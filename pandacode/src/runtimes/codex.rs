//! Codex runtime driven directly through `codex app-server` over stdio.
//!
//! Each turn spawns one app-server process, starts or resumes a thread, runs
//! `turn/start`, and waits for `turn/completed`. Thread state persists as
//! Codex rollout files, so resume works across processes without a daemon.
//!
//! `--detach` re-execs pandacode as a background worker that owns the live
//! app-server for the whole turn. The worker keeps the session record fresh
//! (status/last message/usage), waits in-protocol on `requestUserInput` so
//! `answer` can reply inside the same turn, and honors `interrupt` through a
//! control file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    cli::{
        AnswerCommandArgs, LogsCommandArgs, ModelCommandArgs, PermissionMode, RuntimeBins,
        RuntimeCommand, RuntimeGlobalArgs, SessionCommandArgs, TaskCommandArgs,
    },
    io::{
        BUILTIN_PROMPTS, generated_session, output_json, pandacode_dir, sanitize_name, workspace,
        write_prompt_file,
    },
    session::{self, SessionRecord},
};

use super::codex_appserver::{
    self, AppServerClient, kill_process_group, notification_method, server_request_id,
};

const RUNTIME: &str = "codex";
const DEFAULT_MODEL: &str = "gpt-5.5";
const DEFAULT_EFFORT: &str = "xhigh";
const DEFAULT_TIMEOUT_MS: u64 = 1_200_000;
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_SLICE: Duration = Duration::from_millis(300);

pub fn run(command: RuntimeCommand) -> Result<()> {
    match command {
        RuntimeCommand::Exec(args) => exec(args),
        RuntimeCommand::Resume(args) => resume(args),
        RuntimeCommand::Answer(args) => answer(args),
        RuntimeCommand::Status(args) => status(args),
        RuntimeCommand::Logs(args) => logs(args),
        RuntimeCommand::Artifacts(args) => artifacts(args),
        RuntimeCommand::Model(args) => model(args),
        RuntimeCommand::Models(args) => models(args),
        RuntimeCommand::Interrupt(args) => interrupt(args),
        RuntimeCommand::Stop(args) => stop(args),
        RuntimeCommand::List(args) => list(args),
        RuntimeCommand::Doctor(args) => doctor(args),
    }
}

#[derive(Default)]
struct TurnOutcome {
    state: String,
    agent_messages: Vec<String>,
    usage: Option<Value>,
    questions: Vec<Value>,
    completed: Option<Value>,
    errors: Vec<Value>,
}

fn exec(args: TaskCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let task = crate::io::read_task(
        args.task.as_deref(),
        args.task_file.as_deref(),
        args.stdin.as_deref(),
        Some(&root),
    )?;
    let task = crate::io::apply_prompt_parts(&task, &args.prompt_append, Some(&root))?;
    let session_name = if args.session == "latest" {
        generated_session(RUNTIME)
    } else {
        sanitize_name(&args.session, RUNTIME)
    };
    let model = effective_model(args.model.as_deref(), None);
    let effort = effective_effort(args.effort, None);
    let permission = effective_permission(args.permission, None);
    if args.detach {
        return spawn_detached_worker(
            "exec",
            &root,
            &session_name,
            &task,
            &args,
            &model,
            &effort,
            permission,
        );
    }
    let prompt_file = write_prompt_file(&root, RUNTIME, &session_name, &task)?;
    let log_path = appserver_log_path(&root, &session_name);

    let mut record = SessionRecord::new(RUNTIME, &session_name, "codex-appserver", &root);
    record.model = Some(model.clone());
    record.effort = Some(effort.clone());
    record.permission = Some(permission.as_value().to_string());
    record.artifacts = json!({
        "prompt_file": prompt_file,
        "log_path": log_path,
        "status": "starting",
        "runner_pid": std::process::id(),
        "detached": crate::io::detached_worker(),
        "expected_artifacts": args.expect_artifact,
    });
    session::save(&root, &mut record)?;

    let mut client =
        codex_appserver::spawn_initialized(&args.bins, &root, Some(log_path.clone()))?;
    let start = client.call(
        "thread/start",
        json!({
            "cwd": root.to_string_lossy(),
            "approvalPolicy": approval_policy(permission),
            "sandbox": sandbox_policy(permission),
            "experimentalRawEvents": false,
            "persistExtendedHistory": true,
            "model": model,
        }),
        CALL_TIMEOUT,
    )?;
    record.thread_id = start
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    record.thread_path = start
        .pointer("/result/thread/path")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let thread_model = start
        .pointer("/result/model")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| model.clone());
    let thread_id = record
        .thread_id
        .clone()
        .context("thread/start response missing result.thread.id")?;
    if let Some(objective) = &args.objective {
        let goal = client.call(
            "thread/goal/set",
            json!({ "threadId": thread_id, "objective": objective, "status": "active" }),
            CALL_TIMEOUT,
        )?;
        record.artifacts["objective"] = json!(objective);
        record.artifacts["goal"] = goal["result"].clone();
    }
    record.artifacts["status"] = json!("running");
    session::save(&root, &mut record)?;

    let outcome = run_turn(
        &mut client,
        &root,
        &mut record,
        &thread_id,
        &task,
        &thread_model,
        &effort,
        permission,
        args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
    )?;
    finish_turn(&root, &mut record, &outcome, "exec")
}

fn resume(args: TaskCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let task = crate::io::read_task(
        args.task.as_deref(),
        args.task_file.as_deref(),
        args.stdin.as_deref(),
        Some(&root),
    )?;
    let task = crate::io::apply_prompt_parts(&task, &args.prompt_append, Some(&root))?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    let model = effective_model(args.model.as_deref(), record.model.as_deref());
    let effort = effective_effort(args.effort, record.effort.as_deref());
    let permission = effective_permission(args.permission, record.permission.as_deref());
    if args.detach {
        return spawn_detached_worker(
            "resume",
            &root,
            &record.session.clone(),
            &task,
            &args,
            &model,
            &effort,
            permission,
        );
    }
    record.model = Some(model.clone());
    record.effort = Some(effort.clone());
    record.permission = Some(permission.as_value().to_string());
    if !args.expect_artifact.is_empty() {
        record.artifacts["expected_artifacts"] = json!(args.expect_artifact);
    }
    let outcome = resume_turn(&root, &mut record, &args.bins, &task, args.timeout_ms)?;
    finish_turn(&root, &mut record, &outcome, "resume")
}

#[allow(clippy::too_many_arguments)]
fn spawn_detached_worker(
    action: &str,
    root: &Path,
    session: &str,
    task: &str,
    args: &TaskCommandArgs,
    model: &str,
    effort: &str,
    permission: PermissionMode,
) -> Result<()> {
    let task_file = write_prompt_file(root, RUNTIME, &format!("{session}-detach"), task)?;
    // Pre-write a starting record so `pandacode wait`/status see the lane
    // immediately, before the background worker has spawned its app-server.
    let mut pre = SessionRecord::new(RUNTIME, session, "codex-appserver", root);
    pre.model = Some(model.to_string());
    pre.effort = Some(effort.to_string());
    pre.permission = Some(permission.as_value().to_string());
    pre.artifacts = json!({
        "status": "starting",
        "detached": true,
        "expected_artifacts": args.expect_artifact,
    });
    session::save(root, &mut pre)?;
    let out_dir = pandacode_dir(root).join(RUNTIME).join("detached");
    std::fs::create_dir_all(&out_dir)?;
    let result_file = out_dir.join(format!("{session}.json"));
    let stdout = std::fs::File::create(&result_file)?;
    let stderr = stdout.try_clone()?;
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("codex")
        .arg(action)
        .args(["--session", session])
        .args(["--task-file", &task_file.to_string_lossy()])
        .args(["--cd", &root.to_string_lossy()])
        .args(["--model", model])
        .args(["--effort", effort])
        .args(["--permission", permission.as_value()])
        .args(["--codex-bin", &args.bins.codex_bin])
        .args(["--claude-bin", &args.bins.claude_bin])
        .args(["--tmux-bin", &args.bins.tmux_bin])
        .args(["--log-mode", &args.bins.log_mode])
        .arg("--json")
        .env(crate::io::DETACHED_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    if let Some(timeout) = args.timeout_ms {
        command.args(["--timeout-ms", &timeout.to_string()]);
    }
    if let Some(objective) = &args.objective {
        command.args(["--objective", objective]);
    }
    for artifact in &args.expect_artifact {
        command.args(["--expect-artifact", &artifact.to_string_lossy()]);
    }
    if let Some(auth_home) = &args.bins.auth_home {
        command.args(["--auth-home", &auth_home.to_string_lossy()]);
    }
    if let Some(codex_home) = &args.bins.codex_home {
        command.args(["--codex-home", &codex_home.to_string_lossy()]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().context("spawn detached codex worker")?;
    output_json(&json!({
        "ok": true,
        "state": "running",
        "runtime": RUNTIME,
        "action": action,
        "session": session,
        "detached": true,
        "worker_pid": child.id(),
        "result_file": result_file,
        "note": "turn runs in a detached worker; poll `pandacode codex status`, continue with `answer`, abort with `interrupt`, end with `stop`",
    }))
}

fn answer(args: AnswerCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let pending = record.artifacts["pending_questions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let live_wait = record.artifacts["status"] == "waiting_for_user"
        && record.artifacts["answer_file"].is_string()
        && pid_alive(record.artifacts["runner_pid"].as_u64());
    if live_wait
        && let Ok(payload) =
            build_structured_answer(&pending, args.choice, args.text.as_deref())
    {
        return structured_answer(&root, record, payload, args.timeout_ms);
    }
    // Fallback: continue the thread with the answer as a fresh turn.
    let mut record = record;
    let answer_text = match (&args.text, args.choice) {
        (Some(text), None) => text.clone(),
        (None, Some(choice)) => choice_answer_text(&record, choice)?,
        _ => {
            output_json(&json!({
                "ok": false,
                "runtime": RUNTIME,
                "action": "answer",
                "error": "pass exactly one answer source: --choice N or --text TEXT"
            }))?;
            return Ok(());
        }
    };
    let pending_value = record.artifacts["pending_questions"].clone();
    let task = if pending_value.is_null() {
        answer_text
    } else {
        format!("User answer to the pending question(s) {pending_value}:\n{answer_text}")
    };
    record.artifacts["pending_questions"] = Value::Null;
    let outcome = resume_turn(&root, &mut record, &args.bins, &task, args.timeout_ms)?;
    finish_turn_with(&root, &mut record, &outcome, "answer", json!("resume_turn"))
}

fn structured_answer(
    root: &Path,
    record: SessionRecord,
    payload: Value,
    timeout_ms: Option<u64>,
) -> Result<()> {
    let answer_file = PathBuf::from(
        record.artifacts["answer_file"]
            .as_str()
            .context("answer_file missing")?,
    );
    let original_questions = record.artifacts["pending_questions"].clone();
    if let Some(parent) = answer_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = answer_file.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string(&payload)?)?;
    std::fs::rename(&tmp, &answer_file)?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let session_name = record.session.clone();
    loop {
        std::thread::sleep(POLL_SLICE);
        let current = session::load(root, RUNTIME, &session_name)?;
        let status = current.artifacts["status"].as_str().unwrap_or("");
        let questions_changed = current.artifacts["pending_questions"] != original_questions;
        let settled = match status {
            "running" => false,
            "waiting_for_user" => questions_changed,
            _ => true,
        };
        if settled || Instant::now() >= deadline {
            let ok = status == "completed";
            return output_json(&json!({
                "ok": ok,
                "state": current.artifacts["status"],
                "runtime": RUNTIME,
                "action": "answer",
                "answer_mode": "structured",
                "session": current.session,
                "summary": {
                    "last_agent_message": current.artifacts["last_agent_message"],
                    "model": current.model,
                    "effort": current.effort,
                    "usage": current.artifacts["usage"],
                },
                "pending_user_input": current.artifacts["pending_questions"],
                "record": current,
            }));
        }
    }
}

fn build_structured_answer(
    questions: &[Value],
    choice: Option<usize>,
    text: Option<&str>,
) -> Result<Value> {
    if questions.is_empty() {
        bail!("no pending questions recorded");
    }
    let mut answers = serde_json::Map::new();
    for question in questions {
        let id = question
            .get("id")
            .and_then(Value::as_str)
            .context("pending question has no id; falling back to a resume turn")?;
        let value = if let Some(text) = text {
            text.to_string()
        } else {
            let choice = choice.context("pass --choice N or --text TEXT")?;
            if choice == 0 {
                bail!("--choice is 1-based");
            }
            question
                .get("options")
                .and_then(Value::as_array)
                .and_then(|options| options.get(choice - 1))
                .and_then(option_label)
                .with_context(|| format!("choice {choice} is out of range for question {id}"))?
        };
        answers.insert(id.to_string(), json!({ "answers": [value] }));
    }
    Ok(json!({ "answers": answers }))
}

fn option_label(option: &Value) -> Option<String> {
    option
        .as_str()
        .map(ToString::to_string)
        .or_else(|| {
            option
                .get("label")
                .or_else(|| option.get("text"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn resume_turn(
    root: &Path,
    record: &mut SessionRecord,
    bins: &RuntimeBins,
    task: &str,
    timeout_ms: Option<u64>,
) -> Result<TurnOutcome> {
    let thread_id = record
        .thread_id
        .clone()
        .context("session has no Codex thread id; run `pandacode codex exec` first")?;
    let model = record
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let effort = record
        .effort
        .clone()
        .unwrap_or_else(|| DEFAULT_EFFORT.to_string());
    let permission = PermissionMode::from_record(record.permission.as_deref());
    let log_path = appserver_log_path(root, &record.session);
    let prompt_file = write_prompt_file(root, RUNTIME, &record.session, task)?;
    record.artifacts["last_prompt_file"] = json!(prompt_file);
    record.artifacts["runner_pid"] = json!(std::process::id());
    record.artifacts["detached"] = json!(crate::io::detached_worker());

    let mut client =
        codex_appserver::spawn_initialized(bins, root, Some(log_path.clone()))?;
    let resumed = client.call(
        "thread/resume",
        json!({ "threadId": thread_id, "model": model }),
        CALL_TIMEOUT,
    )?;
    if let Some(path) = resumed
        .pointer("/result/thread/path")
        .and_then(Value::as_str)
    {
        record.thread_path = Some(path.to_string());
    }
    let thread_model = resumed
        .pointer("/result/model")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or(model);
    record.artifacts["status"] = json!("running");
    session::save(root, record)?;
    run_turn(
        &mut client,
        root,
        record,
        &thread_id,
        task,
        &thread_model,
        &effort,
        permission,
        timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    client: &mut AppServerClient,
    root: &Path,
    record: &mut SessionRecord,
    thread_id: &str,
    prompt: &str,
    model: &str,
    effort: &str,
    permission: PermissionMode,
    timeout_ms: u64,
) -> Result<TurnOutcome> {
    let request_id = client.send_request(
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt, "text_elements": []}],
            "approvalPolicy": approval_policy(permission),
            "collaborationMode": {
                "mode": "default",
                "settings": {
                    "model": model,
                    "developer_instructions": null,
                    "reasoning_effort": effort,
                },
            },
        }),
    )?;
    let answer_wait = crate::io::detached_worker();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut outcome = TurnOutcome {
        state: "running".to_string(),
        ..TurnOutcome::default()
    };
    loop {
        let message = match client.recv_maybe(POLL_SLICE) {
            Ok(Some(message)) => message,
            Ok(None) => {
                if take_interrupt(root, &record.session) {
                    interrupt_active_turn(client, record, thread_id);
                    outcome.state = "interrupted".to_string();
                    client.kill();
                    return Ok(outcome);
                }
                if Instant::now() >= deadline {
                    client.kill();
                    outcome.state = "timeout".to_string();
                    return Ok(outcome);
                }
                continue;
            }
            Err(error) => {
                client.kill();
                outcome.state = "failed".to_string();
                outcome.errors.push(json!({ "error": error.to_string() }));
                return Ok(outcome);
            }
        };
        if message.get("method").is_none() {
            if message.get("id").and_then(Value::as_u64) == Some(request_id) {
                if let Some(error) = message.get("error") {
                    outcome.state = "failed".to_string();
                    outcome.errors.push(json!({ "error": error }));
                    return Ok(outcome);
                }
                if let Some(turn_id) = message.pointer("/result/turn/id").and_then(Value::as_str) {
                    record.artifacts["turn_id"] = json!(turn_id);
                }
            }
            continue;
        }
        if let Some(id) = server_request_id(&message) {
            let method = message["method"].as_str().unwrap_or("");
            if method == "item/tool/requestUserInput" {
                outcome.questions = message
                    .pointer("/params/questions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if answer_wait {
                    match wait_for_structured_answer(client, root, record, &outcome.questions, deadline)? {
                        AnswerWait::Answered(payload) => {
                            client.send_response(id, payload)?;
                            record.artifacts["pending_questions"] = Value::Null;
                            record.artifacts["answer_file"] = Value::Null;
                            record.artifacts["status"] = json!("running");
                            session::save(root, record)?;
                            outcome.questions.clear();
                            continue;
                        }
                        AnswerWait::Interrupted => {
                            interrupt_active_turn(client, record, thread_id);
                            outcome.state = "interrupted".to_string();
                            client.kill();
                            return Ok(outcome);
                        }
                        AnswerWait::TimedOut => {
                            client.kill();
                            outcome.state = "timeout".to_string();
                            return Ok(outcome);
                        }
                    }
                }
                outcome.state = "waiting_for_user".to_string();
                client.kill();
                return Ok(outcome);
            }
            let _ = client.send_response(id, json!({}));
            continue;
        }
        match notification_method(&message).unwrap_or("") {
            "turn/started" => {
                if let Some(turn_id) = message.pointer("/params/turn/id").and_then(Value::as_str) {
                    record.artifacts["turn_id"] = json!(turn_id);
                }
            }
            "item/completed" => {
                let item = &message["params"]["item"];
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(item_type, "agentMessage" | "agent_message")
                    && let Some(text) = item.get("text").and_then(Value::as_str)
                {
                    outcome.agent_messages.push(text.to_string());
                    record.artifacts["last_agent_message"] = json!(text);
                    let _ = session::save(root, record);
                }
            }
            "thread/tokenUsage/updated" => {
                outcome.usage = Some(message["params"].clone());
                record.artifacts["usage"] = message["params"].clone();
                let _ = session::save(root, record);
            }
            "turn/completed" => {
                outcome.state = "completed".to_string();
                outcome.completed = Some(message["params"].clone());
                return Ok(outcome);
            }
            "error" => {
                outcome.state = "failed".to_string();
                outcome.errors.push(message.clone());
                return Ok(outcome);
            }
            _ => {}
        }
    }
}

enum AnswerWait {
    Answered(Value),
    Interrupted,
    TimedOut,
}

fn wait_for_structured_answer(
    _client: &mut AppServerClient,
    root: &Path,
    record: &mut SessionRecord,
    questions: &[Value],
    deadline: Instant,
) -> Result<AnswerWait> {
    let answer_file = answer_file_path(root, &record.session);
    if let Some(parent) = answer_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&answer_file);
    record.artifacts["status"] = json!("waiting_for_user");
    record.artifacts["pending_questions"] = json!(questions);
    record.artifacts["answer_file"] = json!(answer_file);
    session::save(root, record)?;
    loop {
        if take_interrupt(root, &record.session) {
            return Ok(AnswerWait::Interrupted);
        }
        if Instant::now() >= deadline {
            return Ok(AnswerWait::TimedOut);
        }
        if answer_file.exists() {
            let text = std::fs::read_to_string(&answer_file)?;
            let _ = std::fs::remove_file(&answer_file);
            if let Ok(payload) = serde_json::from_str::<Value>(&text) {
                return Ok(AnswerWait::Answered(payload));
            }
        }
        std::thread::sleep(POLL_SLICE);
    }
}

fn interrupt_active_turn(client: &mut AppServerClient, record: &SessionRecord, thread_id: &str) {
    if let Some(turn_id) = record.artifacts["turn_id"].as_str() {
        let _ = client.call(
            "turn/interrupt",
            json!({ "threadId": thread_id, "turnId": turn_id }),
            Duration::from_secs(10),
        );
    }
}

fn finish_turn(
    root: &Path,
    record: &mut SessionRecord,
    outcome: &TurnOutcome,
    action: &str,
) -> Result<()> {
    finish_turn_with(root, record, outcome, action, Value::Null)
}

fn finish_turn_with(
    root: &Path,
    record: &mut SessionRecord,
    outcome: &TurnOutcome,
    action: &str,
    answer_mode: Value,
) -> Result<()> {
    let last_agent_message = outcome
        .agent_messages
        .last()
        .cloned()
        .or_else(|| record.artifacts["last_agent_message"].as_str().map(ToString::to_string));
    let missing = crate::io::missing_artifacts(root, &record.artifacts["expected_artifacts"]);
    let state = if outcome.state == "completed" && !missing.is_empty() {
        "no_report".to_string()
    } else {
        outcome.state.clone()
    };
    record.artifacts["missing_artifacts"] = json!(missing);
    record.artifacts["status"] = json!(state);
    record.artifacts["last_agent_message"] = json!(last_agent_message);
    if let Some(usage) = &outcome.usage {
        record.artifacts["usage"] = usage.clone();
    }
    record.artifacts["pending_questions"] = if outcome.state == "waiting_for_user" {
        json!(outcome.questions)
    } else {
        Value::Null
    };
    record.artifacts["answer_file"] = Value::Null;
    let _ = std::fs::remove_file(answer_file_path(root, &record.session));
    let _ = std::fs::remove_file(interrupt_file_path(root, &record.session));
    session::save(root, record)?;
    let ok = state == "completed";
    let pending_user_input = if state == "waiting_for_user" {
        json!({ "questions": outcome.questions, "source": "requestUserInput" })
    } else {
        Value::Null
    };
    let mut report = json!({
        "ok": ok,
        "state": state,
        "runtime": RUNTIME,
        "action": action,
        "session": record.session,
        "summary": {
            "last_agent_message": last_agent_message,
            "agent_messages": outcome.agent_messages,
            "model": record.model,
            "effort": record.effort,
            "usage": outcome.usage,
            "completed": outcome.completed,
            "errors": outcome.errors,
        },
        "pending_user_input": pending_user_input,
        "artifacts": record.artifacts,
        "record": record,
    });
    if !answer_mode.is_null() {
        report["answer_mode"] = answer_mode;
    }
    output_json(&report)
}

fn choice_answer_text(record: &SessionRecord, choice: usize) -> Result<String> {
    if choice == 0 {
        bail!("--choice is 1-based");
    }
    let questions = record.artifacts["pending_questions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for question in &questions {
        if let Some(options) = question.get("options").and_then(Value::as_array)
            && let Some(option) = options.get(choice - 1)
            && let Some(text) = option_label(option)
        {
            return Ok(text);
        }
    }
    Ok(format!("Option {choice}"))
}

fn status(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let state = record.artifacts["status"].as_str().unwrap_or("unknown");
    output_json(&json!({
        "ok": state == "completed" || state == "idle",
        "state": state,
        "runtime": RUNTIME,
        "action": "status",
        "session": record.session,
        "summary": {
            "last_agent_message": record.artifacts["last_agent_message"],
            "model": record.model,
            "effort": record.effort,
            "usage": record.artifacts["usage"],
        },
        "pending_user_input": record.artifacts["pending_questions"],
        "artifacts": record.artifacts,
        "record": record,
    }))
}

fn logs(args: LogsCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    if args.visible {
        let thread_id = record
            .thread_id
            .clone()
            .context("session has no Codex thread id yet")?;
        let mut client = codex_appserver::spawn_initialized(&args.bins, &root, None)?;
        let response = client.call(
            "thread/read",
            json!({ "threadId": thread_id, "includeTurns": true }),
            CALL_TIMEOUT,
        )?;
        client.kill();
        let thread = &response["result"]["thread"];
        let mut messages = Vec::new();
        for turn in thread
            .get("turns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let turn_id = turn.get("id").and_then(Value::as_str).unwrap_or("");
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                let text = match item_type {
                    "userMessage" => item
                        .pointer("/content/0/text")
                        .and_then(Value::as_str),
                    "agentMessage" => item.get("text").and_then(Value::as_str),
                    _ => None,
                };
                if let Some(text) = text {
                    messages.push(json!({
                        "turn_id": turn_id,
                        "type": item_type,
                        "text": text,
                    }));
                }
            }
        }
        return output_json(&json!({
            "ok": true,
            "runtime": RUNTIME,
            "action": "logs",
            "session": record.session,
            "view": "thread",
            "thread_id": thread.get("id"),
            "turn_count": thread.get("turns").and_then(Value::as_array).map(Vec::len),
            "messages": messages,
        }));
    }
    let log_path = appserver_log_path(&root, &record.session);
    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    let lines = text.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(args.tail);
    let log_tail = lines[start..].join("\n");
    if args.json {
        output_json(&json!({
            "ok": true,
            "runtime": RUNTIME,
            "action": "logs",
            "session": record.session,
            "state": record.artifacts["status"],
            "log_path": log_path,
            "log_tail": log_tail,
            "summary": {
                "last_agent_message": record.artifacts["last_agent_message"],
            },
        }))
    } else {
        println!("{log_tail}");
        Ok(())
    }
}

fn artifacts(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let session_name = session::resolve_session(&root, RUNTIME, &args.session)?;
    output_json(&session::artifacts(&root, RUNTIME, &session_name)?)
}

fn model(args: ModelCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    record.model = Some(args.model);
    record.effort = args.effort.map(|effort| effort.as_value().to_string());
    session::save(&root, &mut record)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "model",
        "note": "model and effort apply on the next exec/resume/answer turn",
        "record": record
    }))
}

fn interrupt(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let state = record.artifacts["status"].as_str().unwrap_or("unknown");
    let live = matches!(state, "running" | "waiting_for_user" | "starting")
        && pid_alive(record.artifacts["runner_pid"].as_u64());
    if !live {
        return output_json(&json!({
            "ok": true,
            "runtime": RUNTIME,
            "action": "interrupt",
            "session": record.session,
            "state": state,
            "note": "no live turn to interrupt",
        }));
    }
    let interrupt_file = interrupt_file_path(&root, &record.session);
    if let Some(parent) = interrupt_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&interrupt_file, b"")?;
    let session_name = record.session.clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        std::thread::sleep(POLL_SLICE);
        let current = session::load(&root, RUNTIME, &session_name)?;
        let state = current.artifacts["status"].as_str().unwrap_or("unknown");
        if !matches!(state, "running" | "waiting_for_user" | "starting")
            || Instant::now() >= deadline
        {
            return output_json(&json!({
                "ok": state == "interrupted",
                "runtime": RUNTIME,
                "action": "interrupt",
                "session": current.session,
                "state": state,
            }));
        }
    }
}

fn stop(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    let runner_pid = record.artifacts["runner_pid"].as_u64();
    let detached = record.artifacts["detached"] == json!(true);
    if detached
        && let Some(pid) = runner_pid
        && pid_alive(Some(pid))
    {
        kill_process_group(pid as u32);
        std::thread::sleep(Duration::from_millis(200));
    }
    codex_appserver::reap_orphans(&root);
    record.artifacts["status"] = json!("stopped");
    record.artifacts["pending_questions"] = Value::Null;
    session::save(&root, &mut record)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "stop",
        "session": record.session,
        "state": "stopped",
        "note": "thread state stays resumable on disk",
    }))
}

fn list(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "list",
        "sessions": session::list(&root, RUNTIME)?,
    }))
}

fn models(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&models_report(&root, &args.bins)?)
}

// Codex app-server protocol surface PandaCode depends on. Surfaced in doctor
// so a codex CLI upgrade that changes the protocol is visible, not silent.
const TESTED_CODEX_VERSION: &str = "0.139.0";

fn doctor(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let mut report = doctor_report(&root, &args.bins)?;
    let version = super::version_report(&args.bins.codex_bin, &["--version"]);
    let installed = version
        .get("stdout")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string());
    report["codex_version"] = json!({
        "installed": installed,
        "tested": TESTED_CODEX_VERSION,
        "note": "PandaCode drives the codex app-server JSON-RPC protocol; if a future codex CLI changes thread/turn methods or event names, turns may stall — re-verify after upgrades.",
    });
    report["appserver"] = appserver_handshake_report(&root, &args.bins);
    output_json(&report)
}

fn appserver_handshake_report(root: &Path, bins: &RuntimeBins) -> Value {
    match codex_appserver::spawn_initialized(bins, root, None) {
        Ok(mut client) => {
            let account = client
                .call("account/read", json!({}), CALL_TIMEOUT)
                .ok()
                .and_then(|response| response.pointer("/result/account").cloned())
                .unwrap_or(Value::Null);
            let rate_limits = client
                .call("account/rateLimits/read", json!({}), CALL_TIMEOUT)
                .ok()
                .and_then(|response| response.pointer("/result/rateLimits").cloned())
                .unwrap_or(Value::Null);
            client.kill();
            json!({ "ok": true, "account": account, "rate_limits": rate_limits })
        }
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    }
}

pub fn doctor_report(root: &Path, bins: &RuntimeBins) -> Result<Value> {
    let codex = super::version_report(&bins.codex_bin, &["--help"]);
    let codex_ok = codex.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let missing = [(!codex_ok).then_some("codex")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok(json!({
        "ok": codex_ok,
        "state": if codex_ok { "available" } else { "missing_requirements" },
        "runtime": RUNTIME,
        "workspace": root,
        "driver": "codex app-server",
        "requirements": ["codex"],
        "missing": missing,
        "capabilities": {
            "task_execution": true,
            "resume": true,
            "answer": true,
            "detach": true,
            "interrupt": true,
            "stop": true,
            "model": true,
            "effort": true,
            "objective": true,
            "permissions_supported": ["max", "limited"],
            "timeout": true,
            "token_budget": false,
            "cost_budget": false,
            "provider_cache": false,
            "auto_compact": false,
            "verify_commands": false
        },
        "codex": codex,
    }))
}

pub fn models_report(root: &Path, bins: &RuntimeBins) -> Result<Value> {
    let builtin_prompts = BUILTIN_PROMPTS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let fallback_models = json!([
        {"id": DEFAULT_MODEL, "supported_reasoning_efforts": ["low", "medium", "high", "xhigh"], "is_default": true}
    ]);
    let capabilities = json!({
        "model": true,
        "effort": true,
        "permissions_supported": ["max", "limited"],
        "timeout": true,
        "token_budget": false,
        "cost_budget": false,
        "provider_cache": false,
        "auto_compact": false,
        "verify_commands": false
    });
    let mut client = match codex_appserver::spawn_initialized(bins, root, None) {
        Ok(client) => client,
        Err(error) => {
            return Ok(json!({
                "ok": false,
                "runtime": RUNTIME,
                "action": "models",
                "driver": "codex app-server",
                "workspace": root,
                "models": fallback_models,
                "builtin_prompts": builtin_prompts,
                "error": error.to_string(),
                "note": "app-server handshake failed; reporting PandaCode Codex defaults",
                "capabilities": capabilities,
            }));
        }
    };
    let response = client.call("model/list", json!({}), CALL_TIMEOUT);
    client.kill();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return Ok(json!({
                "ok": false,
                "runtime": RUNTIME,
                "action": "models",
                "driver": "codex app-server",
                "workspace": root,
                "models": fallback_models,
                "builtin_prompts": builtin_prompts,
                "error": error.to_string(),
                "note": "model/list failed; reporting PandaCode Codex defaults",
                "capabilities": capabilities,
            }));
        }
    };
    let models = response
        .pointer("/result/data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|model| {
            json!({
                "id": model.get("id").cloned().unwrap_or(Value::Null),
                "model": model.get("model").cloned().unwrap_or(Value::Null),
                "display_name": model.get("displayName").cloned().unwrap_or(Value::Null),
                "is_default": model.get("isDefault").cloned().unwrap_or(Value::Bool(false)),
                "default_reasoning_effort": model.get("defaultReasoningEffort").cloned().unwrap_or(Value::Null),
                "supported_reasoning_efforts": model.get("supportedReasoningEfforts").cloned().unwrap_or(Value::Array(Vec::new())),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "models",
        "driver": "codex app-server",
        "workspace": root,
        "models": models,
        "builtin_prompts": builtin_prompts,
        "capabilities": capabilities,
    }))
}

fn appserver_log_path(root: &Path, session: &str) -> PathBuf {
    pandacode_dir(root)
        .join(RUNTIME)
        .join("logs")
        .join(format!("{session}.jsonl"))
}

fn control_dir(root: &Path) -> PathBuf {
    pandacode_dir(root).join(RUNTIME).join("control")
}

fn answer_file_path(root: &Path, session: &str) -> PathBuf {
    control_dir(root).join(format!("{session}.answer.json"))
}

fn interrupt_file_path(root: &Path, session: &str) -> PathBuf {
    control_dir(root).join(format!("{session}.interrupt"))
}

fn take_interrupt(root: &Path, session: &str) -> bool {
    let path = interrupt_file_path(root, session);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
        true
    } else {
        false
    }
}

fn pid_alive(pid: Option<u64>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn approval_policy(_permission: PermissionMode) -> &'static str {
    "never"
}

fn sandbox_policy(permission: PermissionMode) -> &'static str {
    match permission {
        PermissionMode::Max => "danger-full-access",
        PermissionMode::Limited => "workspace-write",
    }
}

fn effective_model(explicit: Option<&str>, stored: Option<&str>) -> String {
    explicit.or(stored).unwrap_or(DEFAULT_MODEL).to_string()
}

fn effective_effort(explicit: Option<crate::cli::Effort>, stored: Option<&str>) -> String {
    explicit
        .map(|effort| effort.as_value())
        .or(stored)
        .unwrap_or(DEFAULT_EFFORT)
        .to_string()
}

fn effective_permission(explicit: Option<PermissionMode>, stored: Option<&str>) -> PermissionMode {
    explicit.unwrap_or_else(|| PermissionMode::from_record(stored))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_maps_to_sandbox_and_approval() {
        assert_eq!(sandbox_policy(PermissionMode::Max), "danger-full-access");
        assert_eq!(sandbox_policy(PermissionMode::Limited), "workspace-write");
        assert_eq!(approval_policy(PermissionMode::Max), "never");
    }

    #[test]
    fn effective_values_fall_back_to_defaults() {
        assert_eq!(effective_model(None, None), DEFAULT_MODEL);
        assert_eq!(effective_model(Some("gpt-6"), None), "gpt-6");
        assert_eq!(effective_model(None, Some("gpt-6")), "gpt-6");
        assert_eq!(effective_effort(None, None), DEFAULT_EFFORT);
        assert_eq!(effective_effort(None, Some("low")), "low");
    }

    #[test]
    fn choice_answers_resolve_from_recorded_questions() {
        let mut record = SessionRecord::new(RUNTIME, "s1", "codex-appserver", Path::new("/tmp"));
        record.artifacts = json!({
            "pending_questions": [
                {"question": "Continue?", "options": [{"label": "keep going"}, {"label": "stop here"}]}
            ]
        });
        assert_eq!(choice_answer_text(&record, 2).unwrap(), "stop here");
        assert_eq!(choice_answer_text(&record, 9).unwrap(), "Option 9");
        let mut empty = SessionRecord::new(RUNTIME, "s2", "codex-appserver", Path::new("/tmp"));
        empty.artifacts = json!({});
        assert_eq!(choice_answer_text(&empty, 1).unwrap(), "Option 1");
        assert!(choice_answer_text(&empty, 0).is_err());
    }

    #[test]
    fn structured_answers_use_question_ids_and_option_labels() {
        let questions = vec![json!({
            "id": "q1",
            "question": "Continue?",
            "options": [{"label": "keep going"}, {"label": "stop here"}],
        })];
        let by_choice = build_structured_answer(&questions, Some(2), None).unwrap();
        assert_eq!(by_choice["answers"]["q1"]["answers"][0], "stop here");
        let by_text = build_structured_answer(&questions, None, Some("do it")).unwrap();
        assert_eq!(by_text["answers"]["q1"]["answers"][0], "do it");
        assert!(build_structured_answer(&questions, Some(9), None).is_err());
        let no_id = vec![json!({"question": "Continue?"})];
        assert!(build_structured_answer(&no_id, None, Some("x")).is_err());
        assert!(build_structured_answer(&[], None, Some("x")).is_err());
    }
}
