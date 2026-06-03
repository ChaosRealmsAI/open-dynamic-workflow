use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::{
    cli::{
        AnswerCommandArgs, LogsCommandArgs, ModelCommandArgs, PermissionMode, RuntimeBins,
        RuntimeCommand, RuntimeGlobalArgs, SessionCommandArgs, TaskCommandArgs,
    },
    io::{
        command_report, generated_session, output_json, pandacode_dir, parse_json_or_null,
        run_capture, structured_log_tail, tail, workspace, write_prompt_file,
    },
    session::{self, SessionRecord, require_run_id},
};
use serde_json::Value;

const RUNTIME: &str = "codex";
const DEFAULT_MODEL: &str = "gpt-5.5";
const DEFAULT_EFFORT: &str = "xhigh";

#[derive(Debug, Clone)]
struct CodexControl {
    log_dir: PathBuf,
    session_socket: PathBuf,
}

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

fn exec(args: TaskCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let task = crate::io::read_task(
        args.task.as_deref(),
        args.task_file.as_deref(),
        args.stdin.as_deref(),
        Some(&root),
    )?;
    let session_name = if args.session == "latest" {
        generated_session(RUNTIME)
    } else {
        crate::io::sanitize_name(&args.session, RUNTIME)
    };
    let model = effective_model(args.model.as_deref(), None);
    let effort = effective_effort(args.effort, None);
    let permission = effective_permission(args.permission, None);
    let prompt_file = write_prompt_file(&root, RUNTIME, &session_name, &task)?;
    let dispatch_prompt = crate::io::dispatch_task_for_transport(&task, &prompt_file);
    let dispatch_prompt_file = if let Some(dispatch_prompt) = dispatch_prompt.as_deref() {
        write_prompt_file(
            &root,
            RUNTIME,
            &format!("{}-dispatch", session_name),
            dispatch_prompt,
        )?
    } else {
        prompt_file.clone()
    };
    let (control, command, output, transport_retries) = run_codex_start_with_retry(
        &root,
        &session_name,
        &args,
        &dispatch_prompt_file,
        &model,
        &effort,
    )?;
    let raw = parse_json_or_null(&output.stdout);
    let mut record = SessionRecord::new(RUNTIME, &session_name, "codexctl-session", &root);
    update_record_ids(&mut record, raw.as_ref());
    record.model = Some(model.clone());
    record.effort = Some(effort.clone());
    record.permission = Some(permission.as_value().to_string());
    record.artifacts = json!({
        "prompt_file": prompt_file,
        "dispatch_prompt_file": dispatch_prompt_file,
        "transport": if dispatch_prompt.is_some() { "file_reference" } else { "direct" },
        "transport_retries": transport_retries,
        "log_dir": control.log_dir.to_string_lossy().to_string(),
        "session_socket": control.session_socket.to_string_lossy().to_string()
    });
    let start_report = command_summary(
        output.ok,
        "start",
        Some(&record.session),
        &command,
        &output,
        raw.as_ref(),
    );
    let (ok, state, final_summary, execute_report) = if output.ok {
        if let Some(run_id) = record.run_id.as_deref() {
            if is_needs_input(raw.as_ref()) {
                set_pending_input(&mut record, Some("start"), raw.as_ref());
                (
                    false,
                    codex_state(raw.as_ref()),
                    codex_output_summary(raw.as_ref()),
                    json!(null),
                )
            } else {
                let execute_command = codex_execute_command(
                    &args.bins,
                    run_id,
                    &dispatch_prompt_file,
                    &control,
                    &args,
                    &model,
                    &effort,
                );
                let execute_output = run_capture(&execute_command, Some(&root))?;
                let execute_raw = parse_json_or_null(&execute_output.stdout);
                update_record_ids(&mut record, execute_raw.as_ref());
                if is_needs_input(execute_raw.as_ref()) {
                    set_pending_input(&mut record, Some("execute"), execute_raw.as_ref());
                } else {
                    set_pending_stage(&mut record, None);
                }
                let summary = codex_output_summary(execute_raw.as_ref().or(raw.as_ref()));
                (
                    execute_output.ok && !is_needs_input(execute_raw.as_ref()),
                    codex_state(execute_raw.as_ref().or(raw.as_ref())),
                    summary,
                    command_summary(
                        execute_output.ok,
                        "execute",
                        Some(&record.session),
                        &execute_command,
                        &execute_output,
                        execute_raw.as_ref(),
                    ),
                )
            }
        } else {
            (
                false,
                "failed".to_string(),
                codex_output_summary(raw.as_ref()),
                json!({
                    "ok": false,
                    "runtime": RUNTIME,
                    "action": "execute",
                    "session": record.session,
                    "error": "codexctl session start did not return run_id"
                }),
            )
        }
    } else {
        (
            false,
            codex_state(raw.as_ref()),
            codex_output_summary(raw.as_ref()),
            json!(null),
        )
    };
    session::save(&root, &mut record)?;

    // Reap the per-session daemon once the session is terminal. Without this
    // each finished node leaves a codexctl daemon + codex child resident; see
    // stop_codex_daemon / session_is_terminal.
    if session_is_terminal(&record, &state) {
        stop_codex_daemon(&args.bins, &control);
    }

    let mut report = json!({
        "ok": ok,
        "runtime": RUNTIME,
        "action": "exec",
        "session": record.session,
        "state": state,
        "start": start_report,
        "execute": execute_report,
        "summary": final_summary
    });
    report["record"] = serde_json::to_value(record)?;
    output_json(&report)
}

fn resume(args: TaskCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let task = crate::io::read_task(
        args.task.as_deref(),
        args.task_file.as_deref(),
        args.stdin.as_deref(),
        Some(&root),
    )?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    let model = effective_model(args.model.as_deref(), record.model.as_deref());
    let effort = effective_effort(args.effort, record.effort.as_deref());
    let permission = effective_permission(args.permission, record.permission.as_deref());
    if record.run_id.is_some()
        && args.permission.is_some()
        && permission != PermissionMode::from_record(record.permission.as_deref())
    {
        bail!(
            "Codex permission is established when the run starts; start a new session to switch permission"
        );
    }
    let prompt_file = write_prompt_file(&root, RUNTIME, &record.session, &task)?;
    let dispatch_prompt = crate::io::dispatch_task_for_transport(&task, &prompt_file);
    let dispatch_prompt_file = if let Some(dispatch_prompt) = dispatch_prompt.as_deref() {
        write_prompt_file(
            &root,
            RUNTIME,
            &format!("{}-dispatch", record.session),
            dispatch_prompt,
        )?
    } else {
        prompt_file.clone()
    };
    let control = record_codex_control(&root, &record)?;
    // Resolve a LIVE run for this session, mirroring the daemon-reap rule:
    //  - A parked (needs_input) session was deliberately NOT reaped, so its run
    //    is still live — continue it directly. Resurrecting would fork a fresh
    //    run from the rollout and abandon the parked turn.
    //  - Any other session's daemon was reaped on idle/terminal, so the recorded
    //    run_id is dead. Resurrect the thread from codex's persisted rollout
    //    (thread_id) to get a fresh run_id used consistently for send AND execute.
    //  - With no thread to resurrect, the bare run_id is the last resort.
    let run_id = match (
        pending_stage(&record).is_some(),
        record.run_id.clone(),
        record.thread_id.clone(),
    ) {
        (true, Some(run_id), _) => run_id,
        (_, _, Some(thread_id)) => {
            let resume = codex_resume_command(
                &args.bins, &thread_id, &root, &control, &model, &effort, permission,
            );
            let output = run_capture(&resume, Some(&root))?;
            update_record_ids(&mut record, parse_json_or_null(&output.stdout).as_ref());
            require_run_id(&record)?
        }
        (_, Some(run_id), None) => run_id,
        (_, None, None) => bail!(
            "codex session {} has neither thread_id nor run_id",
            record.session
        ),
    };
    let command = codex_send_command(
        &args.bins,
        &run_id,
        &dispatch_prompt_file,
        &control,
        &args,
        &model,
        &effort,
    );
    let output = run_capture(&command, Some(&root))?;
    let raw = parse_json_or_null(&output.stdout);
    update_record_ids(&mut record, raw.as_ref());
    record.model = Some(model.clone());
    record.effort = Some(effort.clone());
    record.permission = Some(permission.as_value().to_string());
    record.artifacts["last_prompt_file"] = json!(prompt_file);
    record.artifacts["last_dispatch_prompt_file"] = json!(dispatch_prompt_file);
    record.artifacts["last_transport"] = json!(if dispatch_prompt.is_some() {
        "file_reference"
    } else {
        "direct"
    });
    let send_report = command_summary(
        output.ok,
        "send",
        Some(&record.session),
        &command,
        &output,
        raw.as_ref(),
    );
    let (ok, state, final_summary, execute_report) = if output.ok {
        if is_needs_input(raw.as_ref()) {
            set_pending_input(&mut record, Some("send"), raw.as_ref());
            (
                false,
                codex_state(raw.as_ref()),
                codex_output_summary(raw.as_ref()),
                json!(null),
            )
        } else {
            let execute_command = codex_execute_command(
                &args.bins,
                &run_id,
                &dispatch_prompt_file,
                &control,
                &args,
                &model,
                &effort,
            );
            let execute_output = run_capture(&execute_command, Some(&root))?;
            let execute_raw = parse_json_or_null(&execute_output.stdout);
            update_record_ids(&mut record, execute_raw.as_ref());
            if is_needs_input(execute_raw.as_ref()) {
                set_pending_input(&mut record, Some("execute"), execute_raw.as_ref());
            } else {
                set_pending_stage(&mut record, None);
            }
            let summary = codex_output_summary(execute_raw.as_ref().or(raw.as_ref()));
            (
                execute_output.ok && !is_needs_input(execute_raw.as_ref()),
                codex_state(execute_raw.as_ref().or(raw.as_ref())),
                summary,
                command_summary(
                    execute_output.ok,
                    "execute",
                    Some(&record.session),
                    &execute_command,
                    &execute_output,
                    execute_raw.as_ref(),
                ),
            )
        }
    } else {
        (
            false,
            codex_state(raw.as_ref()),
            codex_output_summary(raw.as_ref()),
            json!(null),
        )
    };
    session::save(&root, &mut record)?;
    if session_is_terminal(&record, &state) {
        stop_codex_daemon(&args.bins, &control);
    }
    let mut report = json!({
        "ok": ok,
        "runtime": RUNTIME,
        "action": "resume",
        "session": record.session,
        "state": state,
        "send": send_report,
        "execute": execute_report,
        "summary": final_summary
    });
    report["record"] = serde_json::to_value(record)?;
    output_json(&report)
}

fn answer(args: AnswerCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    let run_id = require_run_id(&record)?;
    let control = record_codex_control(&root, &record)?;
    let pending_stage = pending_stage(&record);
    let pending_question_id = pending_question_id(&record);
    let command = codex_answer_command(
        &args.bins,
        &run_id,
        &root,
        &control,
        &args,
        pending_question_id.as_deref(),
    )?;
    let output = run_capture(&command, Some(&root))?;
    let raw = parse_json_or_null(&output.stdout);
    update_record_ids(&mut record, raw.as_ref());
    let answer_report = command_summary(
        output.ok,
        "answer",
        Some(&record.session),
        &command,
        &output,
        raw.as_ref(),
    );

    let mut execute_report = json!(null);
    let mut final_summary = codex_output_summary(raw.as_ref());
    let mut ok = output.ok;
    let mut state = codex_state(raw.as_ref());

    if output.ok && args.wait && is_needs_input(raw.as_ref()) {
        set_pending_input(&mut record, pending_stage.as_deref(), raw.as_ref());
        ok = false;
    } else if output.ok
        && args.wait
        && is_completed(raw.as_ref())
        && matches!(pending_stage.as_deref(), Some("start" | "send"))
    {
        let model = effective_model(None, record.model.as_deref());
        let effort = effective_effort(None, record.effort.as_deref());
        let execute_command = codex_execute_after_answer_command(
            &args.bins, &run_id, &control, &args, &model, &effort,
        );
        let execute_output = run_capture(&execute_command, Some(&root))?;
        let execute_raw = parse_json_or_null(&execute_output.stdout);
        update_record_ids(&mut record, execute_raw.as_ref());
        if is_needs_input(execute_raw.as_ref()) {
            set_pending_input(&mut record, Some("execute"), execute_raw.as_ref());
        } else {
            set_pending_stage(&mut record, None);
        }
        ok = execute_output.ok && !is_needs_input(execute_raw.as_ref());
        state = codex_state(execute_raw.as_ref().or(raw.as_ref()));
        final_summary = codex_output_summary(execute_raw.as_ref().or(raw.as_ref()));
        execute_report = command_summary(
            execute_output.ok,
            "execute",
            Some(&record.session),
            &execute_command,
            &execute_output,
            execute_raw.as_ref(),
        );
    } else if output.ok && !args.wait {
        state = "running".to_string();
        set_pending_stage(&mut record, pending_stage.as_deref());
    } else if output.ok {
        set_pending_stage(&mut record, None);
    }

    session::save(&root, &mut record)?;
    if session_is_terminal(&record, &state) {
        stop_codex_daemon(&args.bins, &control);
    }
    let mut report = json!({
        "ok": ok,
        "runtime": RUNTIME,
        "action": "answer",
        "session": record.session,
        "state": state,
        "answer": answer_report,
        "execute": execute_report,
        "summary": final_summary
    });
    report["record"] = serde_json::to_value(record)?;
    output_json(&report)
}

fn status(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let run_id = require_run_id(&record)?;
    let control = record_codex_control(&root, &record)?;
    let command = codex_read_command(&args.bins, &run_id, &control, false);
    let output = run_capture(&command, Some(&root))?;
    let raw = parse_json_or_null(&output.stdout);
    output_json(&json!({
        "ok": output.ok,
        "runtime": RUNTIME,
        "action": "status",
        "session": record.session,
        "state": codex_state(raw.as_ref()),
        "summary": codex_output_summary(raw.as_ref()),
        "output_tail": structured_log_tail(&output.stdout, 80),
        "command": command_summary(output.ok, "status", Some(&record.session), &command, &output, raw.as_ref()),
        "record": record
    }))
}

fn logs(args: LogsCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let run_id = require_run_id(&record)?;
    let control = record_codex_control(&root, &record)?;
    let command = codex_read_command(&args.bins, &run_id, &control, true);
    let output = run_capture(&command, Some(&root))?;
    if args.json {
        let raw = parse_json_or_null(&output.stdout);
        output_json(&json!({
            "ok": output.ok,
            "runtime": RUNTIME,
            "action": "logs",
            "session": record.session,
            "tail": args.tail,
            "state": codex_state(raw.as_ref()),
            "summary": codex_output_summary(raw.as_ref()),
            "output_tail": structured_log_tail(&output.stdout, args.tail),
            "command": command_summary(output.ok, "logs", Some(&record.session), &command, &output, raw.as_ref()),
            "record": record
        }))
    } else {
        println!("{}", tail(&output.stdout, args.tail));
        if !output.stderr.trim().is_empty() {
            eprintln!("{}", tail(&output.stderr, args.tail));
        }
        Ok(())
    }
}

fn artifacts(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&session::artifacts(&root, RUNTIME, &args.session)?)
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
        "note": "model and effort are applied on the next codex resume/send turn",
        "record": record
    }))
}

fn models(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&models_report(&root, &args.bins)?)
}

fn interrupt(args: SessionCommandArgs) -> Result<()> {
    session_action(args, "interrupt", codex_interrupt_command)
}

fn stop(args: SessionCommandArgs) -> Result<()> {
    session_action(args, "stop", codex_stop_command)
}

fn list(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let command = vec![
        args.bins.codexctl_bin.clone(),
        "session".to_string(),
        "list".to_string(),
        "--threads".to_string(),
        "--log-dir".to_string(),
        codex_legacy_log_dir(&root).to_string_lossy().to_string(),
        "--log-mode".to_string(),
        args.bins.log_mode.clone(),
    ];
    let output = run_capture(&command, Some(&root))?;
    output_json(&json!({
        "ok": output.ok,
        "runtime": RUNTIME,
        "local": session::list(&root, RUNTIME)?,
        "codexctl": command_report(output.ok, RUNTIME, "list", None, &command, &output, parse_json_or_null(&output.stdout))
    }))
}

fn doctor(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&doctor_report(&root, &args.bins)?)
}

pub fn doctor_report(root: &Path, bins: &RuntimeBins) -> Result<serde_json::Value> {
    let codexctl = super::version_report(&bins.codexctl_bin, &["--help"]);
    let codex = super::version_report(&bins.codex_bin, &["--help"]);
    let codexctl_ok = codexctl
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let codex_ok = codex.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let missing = [
        (!codexctl_ok).then_some("codexctl"),
        (!codex_ok).then_some("codex"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    Ok(json!({
        "ok": codexctl_ok && codex_ok,
        "state": if codexctl_ok && codex_ok { "available" } else { "missing_requirements" },
        "runtime": RUNTIME,
        "workspace": root,
        "driver": "codexctl session",
        "requirements": ["codexctl", "codex"],
        "missing": missing,
        "capabilities": {
            "task_execution": true,
            "resume": true,
            "answer": true,
            "interrupt": true,
            "stop": true,
            "model": true,
            "effort": true,
            "permissions_supported": ["max", "limited"],
            "timeout": true,
            "token_budget": false,
            "cost_budget": false,
            "provider_cache": false,
            "auto_compact": false,
            "verify_commands": false
        },
        "codexctl": codexctl,
        "codex": codex
    }))
}

pub fn models_report(root: &Path, bins: &RuntimeBins) -> Result<serde_json::Value> {
    let command = vec![bins.codexctl_bin.clone(), "models".to_string()];
    let output = run_capture(&command, Some(root))?;
    let mut report = command_report(
        output.ok,
        RUNTIME,
        "models",
        None,
        &command,
        &output,
        parse_json_or_null(&output.stdout),
    );
    report["capabilities"] = json!({
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
    Ok(report)
}

fn session_action(
    args: SessionCommandArgs,
    action: &str,
    build: fn(&RuntimeBins, &str, &CodexControl) -> Vec<String>,
) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let run_id = require_run_id(&record)?;
    let control = record_codex_control(&root, &record)?;
    let command = build(&args.bins, &run_id, &control);
    let output = run_capture(&command, Some(&root))?;
    output_json(&command_report(
        output.ok,
        RUNTIME,
        action,
        Some(&record.session),
        &command,
        &output,
        parse_json_or_null(&output.stdout),
    ))
}

fn run_codex_start_with_retry(
    root: &Path,
    session_name: &str,
    args: &TaskCommandArgs,
    prompt_file: &Path,
    model: &str,
    effort: &str,
) -> Result<(CodexControl, Vec<String>, crate::io::CmdOutput, usize)> {
    let mut retries = 0;
    loop {
        let control_session = if retries == 0 {
            session_name.to_string()
        } else {
            format!("{session_name}-retry-{retries}")
        };
        let control = new_codex_control(root, &control_session)?;
        let command =
            codex_start_command(&args.bins, prompt_file, root, &control, args, model, effort);
        let output = run_capture(&command, Some(root))?;
        if output.ok || retries >= 2 || !is_codex_transport_error(&output) {
            return Ok((control, command, output, retries));
        }
        retries += 1;
    }
}

fn is_codex_transport_error(output: &crate::io::CmdOutput) -> bool {
    let text = format!("{}\n{}", output.stderr, output.stdout).to_lowercase();
    [
        "broken pipe",
        "socket is not connected",
        "connection reset",
        "daemon returned invalid json",
        "eof while parsing",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn command_summary(
    ok: bool,
    action: &str,
    session: Option<&str>,
    command: &[String],
    output: &crate::io::CmdOutput,
    raw: Option<&serde_json::Value>,
) -> serde_json::Value {
    json!({
        "ok": ok,
        "runtime": RUNTIME,
        "action": action,
        "session": session,
        "command": command,
        "shell": crate::io::shell_join(command),
        "exit_code": output.exit_code,
        "stdout_tail": tail_chars(&output.stdout, 1_200),
        "stderr_tail": tail_chars(&output.stderr, 1_200),
        "summary": codex_output_summary(raw)
    })
}

fn codex_output_summary(raw: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(raw) = raw else {
        return json!(null);
    };
    json!({
        "ok": raw.get("ok").and_then(|value| value.as_bool()),
        "status": raw.get("status").and_then(|value| value.as_str()),
        "current_phase": raw.get("current_phase").and_then(|value| value.as_str()),
        "run_id": string_field(raw, &["run_id", "runId"]),
        "thread_id": string_field(raw, &["thread_id", "threadId"]),
        "thread_path": string_field(raw, &["thread_path", "threadPath"]),
        "turn_id": string_field(raw, &["turn_id", "turnId"]),
        "log_path": raw.get("log_path").and_then(|value| value.as_str()),
        "last_agent_message": raw.get("last_agent_message").and_then(|value| value.as_str())
            .or_else(|| last_string(raw.get("agent_messages"))),
        "counts": raw.get("counts").cloned().unwrap_or_else(|| json!({
            "agent_messages": array_len(raw.get("agent_messages")),
            "plans": array_len(raw.get("plans")),
            "questions": array_len(raw.get("questions")),
            "errors": array_len(raw.get("errors")),
            "warnings": array_len(raw.get("warnings"))
        })),
        "usage": raw.get("usage").cloned(),
        "errors": raw.get("errors").and_then(|value| value.as_array()).map(|items| items.len()),
        "warnings": raw.get("warnings").and_then(|value| value.as_array()).map(|items| items.len())
    })
}

fn codex_state(raw: Option<&Value>) -> String {
    match codex_status(raw) {
        Some("needs_input") => "waiting_for_user".to_string(),
        Some("completed") => "completed".to_string(),
        Some("running") => "running".to_string(),
        Some("stopped") => "stopped".to_string(),
        Some("failed") => "failed".to_string(),
        Some(status) => status.to_string(),
        None => "unknown".to_string(),
    }
}

fn codex_status(raw: Option<&Value>) -> Option<&str> {
    raw.and_then(|value| value.get("status"))
        .and_then(|value| value.as_str())
}

fn is_needs_input(raw: Option<&Value>) -> bool {
    codex_status(raw) == Some("needs_input")
}

fn is_completed(raw: Option<&Value>) -> bool {
    codex_status(raw) == Some("completed")
}

fn update_record_ids(record: &mut SessionRecord, raw: Option<&Value>) {
    let Some(raw) = raw else {
        return;
    };
    if let Some(run_id) = string_field(raw, &["run_id", "runId"]) {
        record.run_id = Some(run_id.to_string());
    }
    if let Some(thread_id) = string_field(raw, &["thread_id", "threadId"]) {
        record.thread_id = Some(thread_id.to_string());
    }
    if let Some(thread_path) = string_field(raw, &["thread_path", "threadPath"]) {
        record.thread_path = Some(thread_path.to_string());
    }
}

fn pending_stage(record: &SessionRecord) -> Option<String> {
    record
        .artifacts
        .get("pending_stage")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn pending_question_id(record: &SessionRecord) -> Option<String> {
    record
        .artifacts
        .get("pending_question_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn set_pending_stage(record: &mut SessionRecord, stage: Option<&str>) {
    if !record.artifacts.is_object() {
        record.artifacts = json!({});
    }
    if let Some(object) = record.artifacts.as_object_mut() {
        if let Some(stage) = stage {
            object.insert("pending_stage".to_string(), json!(stage));
        } else {
            object.remove("pending_stage");
            object.remove("pending_question_id");
            object.remove("pending_question");
        }
    }
}

fn set_pending_input(record: &mut SessionRecord, stage: Option<&str>, raw: Option<&Value>) {
    set_pending_stage(record, stage);
    if !record.artifacts.is_object() {
        record.artifacts = json!({});
    }
    if let Some(object) = record.artifacts.as_object_mut() {
        object.remove("pending_question_id");
        object.remove("pending_question");
        if let Some(question_id) = first_question_id(raw) {
            object.insert("pending_question_id".to_string(), json!(question_id));
        }
        if let Some(question_text) = first_question_text(raw) {
            object.insert("pending_question".to_string(), json!(question_text));
        }
    }
}

fn first_pending_question_id(
    bins: &RuntimeBins,
    run_id: &str,
    root: &Path,
    control: &CodexControl,
    known_question_id: Option<&str>,
) -> Result<String> {
    if let Some(question_id) = known_question_id.and_then(non_empty_string) {
        return Ok(question_id.to_string());
    }
    let command = codex_read_command(bins, run_id, control, false);
    let output = run_capture(&command, Some(root))?;
    if !output.ok {
        bail!(
            "codexctl session read failed before answering: {}",
            output.stderr.trim()
        );
    }
    let raw = parse_json_or_null(&output.stdout);
    if let Some(question_id) = first_question_id(raw.as_ref()) {
        return Ok(question_id);
    }

    let full_command = codex_read_command(bins, run_id, control, true);
    let full_output = run_capture(&full_command, Some(root))?;
    if !full_output.ok {
        bail!(
            "codexctl session read --full failed before answering: {}",
            full_output.stderr.trim()
        );
    }
    let full_raw = parse_json_or_null(&full_output.stdout);
    first_question_id(full_raw.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot infer Codex question id for --text; pass --choice N or --text '{{...}}'"
        )
    })
}

fn non_empty_string(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn first_question_id(raw: Option<&Value>) -> Option<String> {
    let raw = raw?;
    for key in [
        "pending_question_id",
        "pendingQuestionId",
        "question_id",
        "questionId",
    ] {
        if let Some(value) = raw.get(key).and_then(|value| value.as_str()) {
            return Some(value.to_string());
        }
    }
    let question = first_question(raw)?;
    for key in ["id", "question_id", "questionId", "key", "name"] {
        if let Some(value) = question.get(key).and_then(|value| value.as_str()) {
            return Some(value.to_string());
        }
    }
    question
        .get("question")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn first_question_text(raw: Option<&Value>) -> Option<String> {
    let question = first_question(raw?)?;
    for key in ["question", "prompt", "text", "message", "title"] {
        if let Some(value) = question.get(key).and_then(|value| value.as_str()) {
            return Some(value.to_string());
        }
    }
    None
}

fn first_question(raw: &Value) -> Option<&Value> {
    raw.get("questions").and_then(|value| {
        value
            .as_array()
            .and_then(|questions| questions.first())
            .or_else(|| value.as_object().map(|_| value))
    })
}

fn string_field<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_str()))
}

fn last_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(|value| value.as_array())
        .and_then(|items| items.last())
        .and_then(|value| value.as_str())
}

fn array_len(value: Option<&serde_json::Value>) -> usize {
    value
        .and_then(|value| value.as_array())
        .map(|items| items.len())
        .unwrap_or(0)
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let start = char_count.saturating_sub(max_chars);
    format!("...{}", text.chars().skip(start).collect::<String>())
}

fn codex_start_command(
    bins: &RuntimeBins,
    prompt_file: &Path,
    root: &Path,
    control: &CodexControl,
    args: &TaskCommandArgs,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "start".to_string(),
        "--prompt-file".to_string(),
        prompt_file.to_string_lossy().to_string(),
        "--cwd".to_string(),
        root.to_string_lossy().to_string(),
        "--timeout".to_string(),
        timeout_arg(args.timeout_ms),
    ];
    push_control(&mut command, control, &bins.log_mode);
    push_model_effort(&mut command, model, effort);
    push_permission(&mut command, effective_permission(args.permission, None));
    command
}

fn codex_send_command(
    bins: &RuntimeBins,
    run_id: &str,
    prompt_file: &Path,
    control: &CodexControl,
    args: &TaskCommandArgs,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "send".to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
        "--prompt-file".to_string(),
        prompt_file.to_string_lossy().to_string(),
        "--timeout".to_string(),
        timeout_arg(args.timeout_ms),
    ];
    push_control(&mut command, control, &args.bins.log_mode);
    push_model_effort(&mut command, model, effort);
    command
}

fn codex_execute_command(
    bins: &RuntimeBins,
    run_id: &str,
    prompt_file: &Path,
    control: &CodexControl,
    args: &TaskCommandArgs,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "execute".to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
        "--prompt-file".to_string(),
        prompt_file.to_string_lossy().to_string(),
        "--timeout".to_string(),
        timeout_arg(args.timeout_ms),
    ];
    push_control(&mut command, control, &args.bins.log_mode);
    push_model_effort(&mut command, model, effort);
    command
}

fn codex_resume_command(
    bins: &RuntimeBins,
    thread_id: &str,
    root: &Path,
    control: &CodexControl,
    model: &str,
    effort: &str,
    permission: PermissionMode,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "resume".to_string(),
        "--thread-id".to_string(),
        thread_id.to_string(),
        "--cwd".to_string(),
        root.to_string_lossy().to_string(),
    ];
    push_control(&mut command, control, &bins.log_mode);
    push_model_effort(&mut command, model, effort);
    push_permission(&mut command, permission);
    command
}

fn codex_read_command(
    bins: &RuntimeBins,
    run_id: &str,
    control: &CodexControl,
    full: bool,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "read".to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
    ];
    push_control(&mut command, control, &bins.log_mode);
    if full {
        command.push("--full".to_string());
    }
    command
}

fn codex_answer_command(
    bins: &RuntimeBins,
    run_id: &str,
    root: &Path,
    control: &CodexControl,
    args: &AnswerCommandArgs,
    pending_question_id: Option<&str>,
) -> Result<Vec<String>> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "answer".to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
        "--timeout".to_string(),
        timeout_arg(args.timeout_ms),
    ];
    push_control(&mut command, control, &bins.log_mode);
    match (args.choice, args.text.as_deref()) {
        (Some(choice), None) if choice > 0 => {
            command.extend(["--pick".to_string(), choice.to_string()]);
        }
        (None, Some(text)) => {
            if serde_json::from_str::<Value>(text)
                .map(|value| value.is_object())
                .unwrap_or(false)
            {
                command.extend(["--answers-json".to_string(), text.to_string()]);
            } else {
                let question_id =
                    first_pending_question_id(bins, run_id, root, control, pending_question_id)?;
                command.extend(["--answer".to_string(), format!("{question_id}={text}")]);
            }
        }
        _ => bail!("pass exactly one answer source: --choice N or --text TEXT"),
    }
    if !args.wait {
        command.push("--detach".to_string());
    }
    Ok(command)
}

fn codex_execute_after_answer_command(
    bins: &RuntimeBins,
    run_id: &str,
    control: &CodexControl,
    args: &AnswerCommandArgs,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        "execute".to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
        "--timeout".to_string(),
        timeout_arg(args.timeout_ms),
    ];
    push_control(&mut command, control, &bins.log_mode);
    push_model_effort(&mut command, model, effort);
    command
}

fn codex_interrupt_command(
    bins: &RuntimeBins,
    run_id: &str,
    control: &CodexControl,
) -> Vec<String> {
    codex_run_id_command(bins, "interrupt", run_id, control)
}

fn codex_stop_command(bins: &RuntimeBins, run_id: &str, control: &CodexControl) -> Vec<String> {
    codex_run_id_command(bins, "stop", run_id, control)
}

fn codex_run_id_command(
    bins: &RuntimeBins,
    action: &str,
    run_id: &str,
    control: &CodexControl,
) -> Vec<String> {
    let mut command = vec![
        bins.codexctl_bin.clone(),
        "session".to_string(),
        action.to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
    ];
    push_control(&mut command, control, &bins.log_mode);
    command
}

fn push_model_effort(command: &mut Vec<String>, model: &str, effort: &str) {
    command.extend(["--model".to_string(), model.to_string()]);
    command.extend(["--effort".to_string(), effort.to_string()]);
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

fn push_permission(command: &mut Vec<String>, permission: PermissionMode) {
    match permission {
        PermissionMode::Max => command.push("--dangerously-full-access".to_string()),
        PermissionMode::Limited => command.extend([
            "--sandbox".to_string(),
            "workspace-write".to_string(),
            "--approval-policy".to_string(),
            "never".to_string(),
        ]),
    }
}

fn timeout_arg(timeout_ms: Option<u64>) -> String {
    timeout_ms
        .map(|value| value.div_ceil(1000).to_string())
        .unwrap_or_else(|| "unlimited".to_string())
}

fn push_control(command: &mut Vec<String>, control: &CodexControl, log_mode: &str) {
    command.extend([
        "--log-dir".to_string(),
        control.log_dir.to_string_lossy().to_string(),
    ]);
    command.extend(["--log-mode".to_string(), log_mode.to_string()]);
    command.extend([
        "--session-socket".to_string(),
        control.session_socket.to_string_lossy().to_string(),
    ]);
}

/// Best-effort stop the per-session codexctl daemon once a session reaches a
/// terminal state. codexctl lazily spawns one `daemon serve` per session socket
/// and never stops it on its own, so each finished node otherwise leaves a
/// daemon (and its codex child) resident until codexctl's idle-timeout reaps it.
/// Skipped while a session is pending (needs_input), since answer/resume reuse
/// the same socket.
fn stop_daemon_command(bins: &RuntimeBins, control: &CodexControl) -> Vec<String> {
    vec![
        bins.codexctl_bin.clone(),
        "--session-socket".to_string(),
        control.session_socket.to_string_lossy().to_string(),
        "daemon".to_string(),
        "stop".to_string(),
    ]
}

fn stop_codex_daemon(bins: &RuntimeBins, control: &CodexControl) {
    let _ = run_capture(&stop_daemon_command(bins, control), None);
}

/// A session is safe to tear down (stop its daemon) only when it has reached a
/// terminal state — not awaiting input (`pending_stage`) and not still running
/// asynchronously (`state == "running"`, e.g. an `answer --no-wait`). Stopping
/// otherwise would orphan a resume/answer continuation or a detached run.
fn session_is_terminal(record: &SessionRecord, state: &str) -> bool {
    pending_stage(record).is_none() && state != "running"
}

fn new_codex_control(root: &Path, session: &str) -> Result<CodexControl> {
    let dir = codex_control_dir(root, session);
    let log_dir = dir.join("logs");
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
    let session_socket = codex_session_socket(root, session);
    if let Some(parent) = session_socket.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(CodexControl {
        log_dir,
        session_socket,
    })
}

fn record_codex_control(root: &Path, record: &SessionRecord) -> Result<CodexControl> {
    let mut control = new_codex_control(root, &record.session)?;
    if let Some(log_dir) = record
        .artifacts
        .get("log_dir")
        .and_then(|value| value.as_str())
    {
        control.log_dir = PathBuf::from(log_dir);
        fs::create_dir_all(&control.log_dir)
            .with_context(|| format!("create {}", control.log_dir.display()))?;
    }
    if let Some(session_socket) = record
        .artifacts
        .get("session_socket")
        .and_then(|value| value.as_str())
    {
        control.session_socket = PathBuf::from(session_socket);
        if let Some(parent) = control.session_socket.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
    }
    Ok(control)
}

fn codex_control_dir(root: &Path, session: &str) -> PathBuf {
    pandacode_dir(root)
        .join("codex")
        .join("runs")
        .join(crate::io::sanitize_name(session, RUNTIME))
}

fn codex_session_socket(root: &Path, session: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    session.hash(&mut hasher);
    std::env::temp_dir()
        .join("pandacode-codex")
        .join(format!("{:016x}.sock", hasher.finish()))
}

fn codex_legacy_log_dir(root: &Path) -> PathBuf {
    pandacode_dir(root).join("codex").join("logs")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::cli::{AnswerCommandArgs, Effort, PermissionMode, RuntimeBins};

    use super::*;

    fn bins() -> RuntimeBins {
        RuntimeBins {
            codexctl_bin: "codexctl".to_string(),
            codex_bin: "codex".to_string(),
            claude_bin: "claude".to_string(),
            tmux_bin: "tmux".to_string(),
            log_mode: "summary".to_string(),
        }
    }

    #[test]
    fn builds_codex_start_with_appserver_session() {
        let args = TaskCommandArgs {
            stdin: None,
            task: Some("fix".to_string()),
            task_file: None,
            cd: PathBuf::from("/repo"),
            session: "latest".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some(Effort::Xhigh),
            permission: None,
            timeout_ms: Some(120_000),
            json: false,
            bins: bins(),
        };
        let control = CodexControl {
            log_dir: PathBuf::from("/repo/.pandacode/codex/runs/s1/logs"),
            session_socket: PathBuf::from("/tmp/pandacode-codex/s1.sock"),
        };
        let command = codex_start_command(
            &args.bins,
            Path::new("/tmp/task.md"),
            Path::new("/repo"),
            &control,
            &args,
            "gpt-5.5",
            "xhigh",
        );
        assert!(command.windows(2).any(|pair| pair == ["session", "start"]));
        assert!(command.contains(&"--session-socket".to_string()));
        assert!(command.contains(&"/tmp/pandacode-codex/s1.sock".to_string()));
        assert!(command.contains(&"--dangerously-full-access".to_string()));
        assert!(command.contains(&"gpt-5.5".to_string()));
        assert!(command.contains(&"xhigh".to_string()));
    }

    #[test]
    fn builds_daemon_stop_command() {
        let control = CodexControl {
            log_dir: PathBuf::from("/repo/.pandacode/codex/runs/s1/logs"),
            session_socket: PathBuf::from("/tmp/pandacode-codex/s1.sock"),
        };
        let command = stop_daemon_command(&bins(), &control);
        assert!(command.windows(2).any(|pair| pair == ["daemon", "stop"]));
        assert!(command.contains(&"--session-socket".to_string()));
        assert!(command.contains(&"/tmp/pandacode-codex/s1.sock".to_string()));
    }

    #[test]
    fn builds_codex_start_with_limited_permission() {
        let args = TaskCommandArgs {
            stdin: None,
            task: Some("fix".to_string()),
            task_file: None,
            cd: PathBuf::from("/repo"),
            session: "latest".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some(Effort::Xhigh),
            permission: Some(PermissionMode::Limited),
            timeout_ms: Some(120_000),
            json: false,
            bins: bins(),
        };
        let control = CodexControl {
            log_dir: PathBuf::from("/repo/.pandacode/codex/runs/s1/logs"),
            session_socket: PathBuf::from("/tmp/pandacode-codex/s1.sock"),
        };
        let command = codex_start_command(
            &args.bins,
            Path::new("/tmp/task.md"),
            Path::new("/repo"),
            &control,
            &args,
            "gpt-5.5",
            "xhigh",
        );
        assert!(!command.contains(&"--dangerously-full-access".to_string()));
        assert!(
            command
                .windows(2)
                .any(|pair| pair == ["--sandbox", "workspace-write"])
        );
        assert!(
            command
                .windows(2)
                .any(|pair| pair == ["--approval-policy", "never"])
        );
    }

    #[test]
    fn builds_codex_answer_with_choice() {
        let args = AnswerCommandArgs {
            session: "latest".to_string(),
            cd: PathBuf::from("/repo"),
            choice: Some(2),
            text: None,
            wait: true,
            timeout_ms: Some(30_000),
            json: false,
            bins: bins(),
        };
        let control = CodexControl {
            log_dir: PathBuf::from("/repo/.pandacode/codex/runs/s1/logs"),
            session_socket: PathBuf::from("/tmp/pandacode-codex/s1.sock"),
        };
        let command = codex_answer_command(
            &args.bins,
            "run_1",
            Path::new("/repo"),
            &control,
            &args,
            None,
        )
        .unwrap();
        assert!(command.windows(2).any(|pair| pair == ["session", "answer"]));
        assert!(command.contains(&"--run-id".to_string()));
        assert!(command.contains(&"run_1".to_string()));
        assert!(command.contains(&"--session-socket".to_string()));
        assert!(command.contains(&"--pick".to_string()));
        assert!(command.contains(&"2".to_string()));
        assert!(!command.contains(&"--detach".to_string()));
    }

    #[test]
    fn builds_codex_text_answer_from_recorded_pending_question() {
        let args = AnswerCommandArgs {
            session: "latest".to_string(),
            cd: PathBuf::from("/repo"),
            choice: None,
            text: Some("main".to_string()),
            wait: true,
            timeout_ms: Some(30_000),
            json: false,
            bins: bins(),
        };
        let control = CodexControl {
            log_dir: PathBuf::from("/repo/.pandacode/codex/runs/s1/logs"),
            session_socket: PathBuf::from("/tmp/pandacode-codex/s1.sock"),
        };
        let command = codex_answer_command(
            &args.bins,
            "run_1",
            Path::new("/repo"),
            &control,
            &args,
            Some("question-1"),
        )
        .unwrap();
        assert!(command.contains(&"--answer".to_string()));
        assert!(command.contains(&"question-1=main".to_string()));
    }

    #[test]
    fn detects_codex_transport_errors() {
        let output = crate::io::CmdOutput {
            ok: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "Error: daemon returned invalid JSON\nEOF while parsing".to_string(),
        };
        assert!(is_codex_transport_error(&output));
    }
}
