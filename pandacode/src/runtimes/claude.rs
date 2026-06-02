use std::{
    fs,
    io::{self, Read},
    path::Path,
    thread,
    time::Duration,
};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    cli::{
        AnswerCommandArgs, ClaudeHookArgs, LogsCommandArgs, ModelCommandArgs, PermissionMode,
        RuntimeBins, RuntimeCommand, RuntimeGlobalArgs, SessionCommandArgs, TaskCommandArgs,
    },
    io::{
        command_report, generated_session, output_json, pandacode_dir, run_capture, run_status,
        run_stdin, sanitize_name, shell_join, strip_ansi_controls, structured_log_tail, tail,
        workspace, write_prompt_file,
    },
    session::{self, SessionRecord},
};

const RUNTIME: &str = "claude";
const DEFAULT_MODEL: &str = "opus";
const DEFAULT_EFFORT: &str = "max";

struct ClaudeLaunch<'a> {
    tmux_name: &'a str,
    debug_log: &'a Path,
    settings_json: &'a str,
    resume_session_id: Option<&'a str>,
    model: &'a str,
    effort: &'a str,
    permission: PermissionMode,
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

pub fn record_hook(args: ClaudeHookArgs) -> Result<()> {
    if let Some(parent) = args.event_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut payload_text = String::new();
    io::stdin().read_to_string(&mut payload_text)?;
    let payload = if payload_text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(&payload_text).unwrap_or_else(|_| json!(payload_text))
    };
    let record = json!({
        "captured_ms": crate::io::now_millis(),
        "kind": args.kind,
        "payload": payload
    });
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.event_log)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
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
        sanitize_name(&args.session, RUNTIME)
    };
    let model = effective_model(args.model.as_deref(), None);
    let effort = effective_effort(args.effort, None);
    let permission = effective_permission(args.permission, None);
    ensure_started(
        &root,
        &session_name,
        &args,
        None,
        &model,
        &effort,
        permission,
    )?;
    let report: Result<Value> = (|| {
        let prompt_file = write_prompt_file(&root, RUNTIME, &session_name, &task)?;
        let dispatch_task = crate::io::dispatch_task_for_transport(&task, &prompt_file);
        let marker = format!(
            "PANDACODE_DONE_{}_{}",
            crate::io::now_millis(),
            std::process::id()
        );
        let prompt = with_completion_marker(dispatch_task.as_deref().unwrap_or(&task), &marker);
        let turn_started = crate::io::now_millis();
        tmux_send_text(&args.bins.tmux_bin, &session_name, &prompt, true)?;
        let wait = wait_for_turn_completion(
            &args.bins.tmux_bin,
            &session_name,
            &marker,
            Some(&event_log_path(&root, &session_name)),
            turn_started,
            args.timeout_ms.unwrap_or(120_000),
        )?;

        let mut record = SessionRecord::new(RUNTIME, &session_name, "claude-tmux", &root);
        record.tmux_name = Some(session_name.clone());
        fill_claude_session_from_events(&root, &session_name, &mut record);
        record.model = Some(model);
        record.effort = Some(effort);
        record.permission = Some(permission.as_value().to_string());
        record.artifacts = json!({
            "prompt_file": prompt_file,
            "transport": if dispatch_task.is_some() { "file_reference" } else { "direct" },
            "debug_log": debug_log_path(&root, &session_name),
            "event_log": event_log_path(&root, &session_name),
            "tmux_session": session_name
        });
        if wait["status"] == "waiting_for_user" {
            record.artifacts["pending_marker"] = json!(marker);
        }
        session::save(&root, &mut record)?;
        let pending_user_input = wait
            .get("pending_user_input")
            .cloned()
            .unwrap_or(Value::Null);
        let output_tail = wait.get("output_tail").cloned().unwrap_or(Value::Null);
        let last_agent_message = wait
            .get("last_agent_message")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(json!({
            "ok": wait["ok"],
            "state": wait["status"],
            "runtime": RUNTIME,
            "action": "exec",
            "session": record.session,
            "marker": marker,
            "summary": {
                "output_tail": output_tail,
                "last_agent_message": last_agent_message,
                "model": record.model,
                "effort": record.effort
            },
            "pending_user_input": pending_user_input,
            "artifacts": record.artifacts,
            "wait": wait,
            "record": record
        }))
    })();
    let keep_session = report
        .as_ref()
        .is_ok_and(|value| value["state"] == "waiting_for_user");
    if !keep_session {
        let _ = kill_tmux_session_if_exists(&args.bins.tmux_bin, &session_name);
    }
    output_json(&report?)
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
    let tmux = record
        .tmux_name
        .clone()
        .unwrap_or_else(|| sanitize_name(&record.session, RUNTIME));
    let model = effective_model(args.model.as_deref(), record.model.as_deref());
    let effort = effective_effort(args.effort, record.effort.as_deref());
    let permission = effective_permission(args.permission, record.permission.as_deref());
    if tmux_has_session(&args.bins.tmux_bin, &tmux)?
        && args.permission.is_some()
        && permission != PermissionMode::from_record(record.permission.as_deref())
    {
        bail!(
            "Claude permission is established when the tmux session starts; stop the session or use a new session to switch permission"
        );
    }
    let started_for_turn = !tmux_has_session(&args.bins.tmux_bin, &tmux)?;
    if started_for_turn {
        ensure_started(
            &root,
            &tmux,
            &args,
            record.thread_id.as_deref(),
            &model,
            &effort,
            permission,
        )?;
    }
    let prompt_file = write_prompt_file(&root, RUNTIME, &record.session, &task)?;
    let dispatch_task = crate::io::dispatch_task_for_transport(&task, &prompt_file);
    let marker = format!(
        "PANDACODE_DONE_{}_{}",
        crate::io::now_millis(),
        std::process::id()
    );
    let prompt = with_completion_marker(dispatch_task.as_deref().unwrap_or(&task), &marker);
    let turn_started = crate::io::now_millis();
    tmux_send_text(&args.bins.tmux_bin, &tmux, &prompt, true)?;
    let wait = wait_for_turn_completion(
        &args.bins.tmux_bin,
        &tmux,
        &marker,
        Some(&event_log_path(&root, &tmux)),
        turn_started,
        args.timeout_ms.unwrap_or(120_000),
    )?;
    fill_claude_session_from_events(&root, &tmux, &mut record);
    record.model = Some(model);
    record.effort = Some(effort);
    record.permission = Some(permission.as_value().to_string());
    record.artifacts["last_prompt_file"] = json!(prompt_file);
    record.artifacts["last_transport"] = json!(if dispatch_task.is_some() {
        "file_reference"
    } else {
        "direct"
    });
    record.artifacts["event_log"] = json!(event_log_path(&root, &tmux));
    if wait["status"] == "waiting_for_user" {
        record.artifacts["pending_marker"] = json!(marker);
    } else if let Some(object) = record.artifacts.as_object_mut() {
        object.remove("pending_marker");
    }
    session::save(&root, &mut record)?;
    let pending_user_input = wait
        .get("pending_user_input")
        .cloned()
        .unwrap_or(Value::Null);
    let output_tail = wait.get("output_tail").cloned().unwrap_or(Value::Null);
    let last_agent_message = wait
        .get("last_agent_message")
        .cloned()
        .unwrap_or(Value::Null);
    let report = json!({
        "ok": wait["ok"],
        "state": wait["status"],
        "runtime": RUNTIME,
        "action": "resume",
        "session": record.session,
        "marker": marker,
        "summary": {
            "output_tail": output_tail,
            "last_agent_message": last_agent_message,
            "model": record.model,
            "effort": record.effort
        },
        "pending_user_input": pending_user_input,
        "artifacts": record.artifacts,
        "wait": wait,
        "record": record
    });
    if started_for_turn && report["state"] != "waiting_for_user" {
        let _ = kill_tmux_session_if_exists(&args.bins.tmux_bin, &tmux);
    }
    output_json(&report)
}

fn answer(args: AnswerCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let mut record = session::load(&root, RUNTIME, &args.session)?;
    let tmux = record
        .tmux_name
        .clone()
        .unwrap_or_else(|| sanitize_name(&record.session, RUNTIME));
    if !tmux_has_session(&args.bins.tmux_bin, &tmux)? {
        bail!(
            "Claude tmux session {tmux} is not running; start it with `pandacode claude resume --session {} --task ...`",
            record.session
        );
    }

    let answer_started = crate::io::now_millis();
    match (args.choice, args.text.as_deref()) {
        (Some(choice), None) if choice > 0 => {
            for _ in 1..choice {
                let output = run_status(
                    &tmux_send_keys_command(&args.bins.tmux_bin, &tmux, &["Down".to_string()]),
                    None,
                )?;
                if !output.ok {
                    bail!("tmux Down failed: {}", output.stderr.trim());
                }
                thread::sleep(Duration::from_millis(100));
            }
            let output = run_status(
                &tmux_send_keys_command(&args.bins.tmux_bin, &tmux, &["Enter".to_string()]),
                None,
            )?;
            if !output.ok {
                bail!("tmux Enter failed: {}", output.stderr.trim());
            }
        }
        (None, Some(text)) => {
            tmux_send_text(&args.bins.tmux_bin, &tmux, text, true)?;
        }
        _ => bail!("pass exactly one answer source: --choice N or --text TEXT"),
    }

    let marker = record
        .artifacts
        .get("pending_marker")
        .and_then(|value| value.as_str())
        .map(ToString::to_string);
    let wait = if args.wait {
        thread::sleep(Duration::from_millis(1_000));
        if let Some(marker) = marker.as_deref() {
            Some(wait_for_turn_completion(
                &args.bins.tmux_bin,
                &tmux,
                marker,
                Some(&event_log_path(&root, &tmux)),
                answer_started,
                args.timeout_ms.unwrap_or(120_000),
            )?)
        } else {
            None
        }
    } else {
        None
    };

    fill_claude_session_from_events(&root, &tmux, &mut record);
    if wait
        .as_ref()
        .is_some_and(|value| value["status"] != "waiting_for_user")
        && let Some(object) = record.artifacts.as_object_mut()
    {
        object.remove("pending_marker");
    }
    session::save(&root, &mut record)?;
    let capture = tmux_capture(&args.bins.tmux_bin, &tmux, 120, true, false)?;
    let state = claude_state(&event_log_path(&root, &tmux), Some(&capture), true);
    let output_tail = structured_log_tail(&capture, 80);
    output_json(&json!({
        "ok": wait.as_ref().map(|value| value["ok"].as_bool().unwrap_or(true)).unwrap_or(true),
        "state": wait.as_ref().and_then(|value| value["status"].as_str()).unwrap_or(state.as_str()),
        "runtime": RUNTIME,
        "action": "answer",
        "session": record.session,
        "tmux_session": tmux,
        "summary": {
            "output_tail": output_tail
        },
        "wait": wait,
        "output_tail": output_tail,
        "pending_user_input": pending_user_input(&event_log_path(&root, &tmux)),
        "artifacts": record.artifacts,
        "record": record
    }))
}

fn status(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let tmux = record.tmux_name.as_deref().unwrap_or(&record.session);
    let alive = tmux_has_session(&args.bins.tmux_bin, tmux)?;
    let capture = if alive {
        Some(tmux_capture(&args.bins.tmux_bin, tmux, 80, true, false)?)
    } else {
        None
    };
    let visible = if alive {
        Some(tmux_capture(&args.bins.tmux_bin, tmux, 80, true, true)?)
    } else {
        None
    };
    let event_log = event_log_path(&root, tmux);
    let state = claude_state(&event_log, visible.as_deref(), alive);
    output_json(&json!({
        "ok": true,
        "state": state,
        "runtime": RUNTIME,
        "action": "status",
        "session": record.session,
        "alive": alive,
        "output_tail": capture.as_deref().map(|text| structured_log_tail(text, 80)),
        "visible_tail": visible.as_deref().map(|text| structured_log_tail(text, 80)),
        "raw": {
            "capture_chars": capture.as_ref().map(|text| text.len()).unwrap_or(0),
            "visible_chars": visible.as_ref().map(|text| text.len()).unwrap_or(0),
            "capture_redacted": true
        },
        "pending_user_input": pending_user_input(&event_log),
        "last_notification": last_notification(&event_log),
        "record": record
    }))
}

fn logs(args: LogsCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let tmux = record.tmux_name.as_deref().unwrap_or(&record.session);
    let capture = tmux_capture(&args.bins.tmux_bin, tmux, args.tail, true, args.visible)?;
    if args.json {
        output_json(&json!({
            "ok": true,
            "runtime": RUNTIME,
            "action": "logs",
            "session": record.session,
            "visible": args.visible,
            "tail": args.tail,
            "output_tail": structured_log_tail(&capture, args.tail),
            "raw": {
                "capture_chars": capture.len(),
                "capture_redacted": true
            }
        }))
    } else {
        println!("{capture}");
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
        "note": "Claude model and effort are applied on the next restart/resume turn; hot-switching an active TUI is not assumed in v1.",
        "record": record
    }))
}

fn models(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&models_report(&root, &args.bins)?)
}

fn interrupt(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let tmux = record.tmux_name.as_deref().unwrap_or(&record.session);
    let command = tmux_send_keys_command(&args.bins.tmux_bin, tmux, &["Escape".to_string()]);
    let output = run_capture(&command, Some(&root))?;
    output_json(&command_report(
        output.ok,
        RUNTIME,
        "interrupt",
        Some(&record.session),
        &command,
        &output,
        None,
    ))
}

fn stop(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let tmux = record.tmux_name.as_deref().unwrap_or(&record.session);
    let command = vec![
        args.bins.tmux_bin.clone(),
        "kill-session".to_string(),
        "-t".to_string(),
        tmux.to_string(),
    ];
    let output = run_capture(&command, Some(&root))?;
    output_json(&command_report(
        output.ok,
        RUNTIME,
        "stop",
        Some(&record.session),
        &command,
        &output,
        None,
    ))
}

fn list(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let command = vec![
        args.bins.tmux_bin.clone(),
        "list-sessions".to_string(),
        "-F".to_string(),
        "#{session_name}".to_string(),
    ];
    let output = run_capture(&command, Some(&root))?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "local": session::list(&root, RUNTIME)?,
        "tmux": command_report(output.ok, RUNTIME, "list", None, &command, &output, None)
    }))
}

fn doctor(args: RuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&doctor_report(&root, &args.bins)?)
}

pub fn doctor_report(root: &Path, bins: &RuntimeBins) -> Result<serde_json::Value> {
    let claude = super::version_report(&bins.claude_bin, &["--help"]);
    let tmux = super::version_report(&bins.tmux_bin, &["-V"]);
    let claude_ok = claude.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let tmux_ok = tmux.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let missing = [
        (!claude_ok).then_some("claude"),
        (!tmux_ok).then_some("tmux"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    Ok(json!({
        "ok": claude_ok && tmux_ok,
        "state": if claude_ok && tmux_ok { "available" } else { "missing_requirements" },
        "runtime": RUNTIME,
        "workspace": root,
        "driver": "tmux",
        "requirements": ["claude", "tmux"],
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
        "claude": claude,
        "tmux": tmux,
        "forbidden": "claude -p / stream-json / json-schema path is intentionally not used"
    }))
}

pub fn models_report(root: &Path, bins: &RuntimeBins) -> Result<serde_json::Value> {
    let command = vec![bins.claude_bin.clone(), "--help".to_string()];
    let output = run_capture(&command, Some(root))?;
    Ok(json!({
        "ok": output.ok,
        "runtime": RUNTIME,
        "action": "models",
        "capabilities": {
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
        "known_aliases": ["haiku", "sonnet", "opus"],
        "note": "Claude accepts aliases or full model ids through --model; use Claude Code UI for account-specific availability.",
        "help_tail": tail(&output.stdout, 40),
        "stderr_tail": tail(&output.stderr, 20)
    }))
}

fn ensure_started(
    root: &Path,
    tmux_name: &str,
    args: &TaskCommandArgs,
    resume_session_id: Option<&str>,
    model: &str,
    effort: &str,
    permission: PermissionMode,
) -> Result<()> {
    if tmux_has_session(&args.bins.tmux_bin, tmux_name)? {
        return Ok(());
    }
    let debug_log = debug_log_path(root, tmux_name);
    if let Some(parent) = debug_log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let settings = install_hook_recorder(root, tmux_name)?;
    let launch = ClaudeLaunch {
        tmux_name,
        debug_log: &debug_log,
        settings_json: &settings,
        resume_session_id,
        model,
        effort,
        permission,
    };
    let claude_command = claude_command(&args.bins, &launch);
    let tmux_command = vec![
        args.bins.tmux_bin.clone(),
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        tmux_name.to_string(),
        "-c".to_string(),
        root.to_string_lossy().to_string(),
        shell_join(&claude_command),
    ];
    let output = run_capture(&tmux_command, Some(root))?;
    if !output.ok {
        bail!("tmux start failed: {}", output.stderr.trim());
    }
    if let Err(error) = wait_for_input_prompt(&args.bins.tmux_bin, tmux_name, 30_000) {
        let _ = kill_tmux_session_if_exists(&args.bins.tmux_bin, tmux_name);
        return Err(error);
    }
    Ok(())
}

fn claude_command(bins: &RuntimeBins, launch: &ClaudeLaunch<'_>) -> Vec<String> {
    let mut command = vec![
        bins.claude_bin.clone(),
        "--name".to_string(),
        launch.tmux_name.to_string(),
        "--brief".to_string(),
        "--debug-file".to_string(),
        launch.debug_log.to_string_lossy().to_string(),
        "--setting-sources".to_string(),
        "local".to_string(),
        "--settings".to_string(),
        launch.settings_json.to_string(),
        "--strict-mcp-config".to_string(),
        "--mcp-config".to_string(),
        "{\"mcpServers\":{}}".to_string(),
    ];
    push_permission(&mut command, launch.permission);
    if let Some(session_id) = launch.resume_session_id {
        command.extend(["--resume".to_string(), session_id.to_string()]);
    }
    command.extend(["--model".to_string(), launch.model.to_string()]);
    command.extend(["--effort".to_string(), launch.effort.to_string()]);
    command
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
        PermissionMode::Max => command.push("--dangerously-skip-permissions".to_string()),
        PermissionMode::Limited => {
            command.extend(["--permission-mode".to_string(), "acceptEdits".to_string()])
        }
    }
}

fn install_hook_recorder(root: &Path, session: &str) -> Result<String> {
    if let Some(parent) = event_log_path(root, session).parent() {
        fs::create_dir_all(parent)?;
    }
    let event_log = event_log_path(root, session);
    let recorder = std::env::current_exe()?;
    let command = |kind: &str| -> String {
        shell_join(&[
            recorder.to_string_lossy().to_string(),
            "claude-hook".to_string(),
            "--event-log".to_string(),
            event_log.to_string_lossy().to_string(),
            "--kind".to_string(),
            kind.to_string(),
        ])
    };
    let settings_json = json!({
        "hooks": {
            "UserPromptSubmit": [{
                "matcher": "",
                "hooks": [{ "type": "command", "command": command("UserPromptSubmit") }]
            }],
            "PreToolUse": [{
                "matcher": "AskUserQuestion",
                "hooks": [{ "type": "command", "command": command("PreToolUse:AskUserQuestion") }]
            }, {
                "matcher": "SendUserMessage",
                "hooks": [{ "type": "command", "command": command("PreToolUse:SendUserMessage") }]
            }],
            "PostToolUse": [{
                "matcher": "AskUserQuestion",
                "hooks": [{ "type": "command", "command": command("PostToolUse:AskUserQuestion") }]
            }, {
                "matcher": "SendUserMessage",
                "hooks": [{ "type": "command", "command": command("PostToolUse:SendUserMessage") }]
            }],
            "Notification": [{
                "matcher": "",
                "hooks": [{ "type": "command", "command": command("Notification") }]
            }],
            "Stop": [{
                "matcher": "",
                "hooks": [{ "type": "command", "command": command("Stop") }]
            }]
        }
    });
    Ok(serde_json::to_string(&settings_json)?)
}

fn event_log_path(root: &Path, session: &str) -> std::path::PathBuf {
    pandacode_dir(root)
        .join("claude")
        .join("events")
        .join(format!("{session}.jsonl"))
}

fn read_hook_events(path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn pending_user_input(path: &Path) -> Option<Value> {
    let events = read_hook_events(path);
    pending_user_input_from_events(&events)
}

fn pending_user_input_from_events(events: &[Value]) -> Option<Value> {
    let mut last_ask_pre = None;
    let mut last_ask_post = None;
    let mut last_notification = None;
    let mut last_stop = None;

    for (index, event) in events.iter().enumerate() {
        let kind = event.get("kind").and_then(|value| value.as_str());
        let payload = event.get("payload").unwrap_or(&Value::Null);
        match kind {
            Some("PreToolUse:AskUserQuestion") => last_ask_pre = Some((index, event.clone())),
            Some("PostToolUse:AskUserQuestion") => last_ask_post = Some(index),
            Some("Notification") => last_notification = Some((index, event.clone())),
            Some("Stop") => last_stop = Some(index),
            _ => {
                if payload
                    .get("hook_event_name")
                    .and_then(|value| value.as_str())
                    == Some("Stop")
                {
                    last_stop = Some(index);
                }
            }
        }
    }

    if let Some((index, event)) = last_ask_pre
        && last_ask_post.is_none_or(|post| post < index)
    {
        return Some(json!({
            "kind": "ask_user_question",
            "event": event
        }));
    }

    if let Some((index, event)) = last_notification {
        let notification_type = event
            .pointer("/payload/notification_type")
            .and_then(|value| value.as_str());
        if matches!(notification_type, Some("permission_prompt"))
            && last_stop.is_none_or(|stop| stop < index)
        {
            return Some(json!({
                "kind": notification_type.unwrap_or("notification"),
                "event": event
            }));
        }
    }

    None
}

fn last_notification(path: &Path) -> Option<Value> {
    read_hook_events(path)
        .into_iter()
        .rev()
        .find(|event| event.get("kind").and_then(|value| value.as_str()) == Some("Notification"))
}

fn claude_state(path: &Path, visible: Option<&str>, alive: bool) -> String {
    if !alive {
        return "stopped".to_string();
    }
    if pending_user_input(path).is_some() {
        return "waiting_for_user".to_string();
    }
    let visible = visible.unwrap_or_default();
    if is_workspace_trust_prompt(visible) {
        return "waiting_for_user".to_string();
    }
    let looks_interruptible = visible.contains("esc to interrupt")
        || visible.contains("ctrl-c to interrupt")
        || visible.contains("interrupt");
    if looks_interruptible
        && !visible.contains("\u{276f}")
        && !visible.contains("Claude is waiting")
    {
        return "running".to_string();
    }
    if is_input_ready_prompt(visible) {
        return "idle".to_string();
    }
    "running".to_string()
}

fn fill_claude_session_from_events(root: &Path, tmux: &str, record: &mut SessionRecord) {
    for event in read_hook_events(&event_log_path(root, tmux)).iter().rev() {
        let payload = event.get("payload").unwrap_or(&Value::Null);
        if record.thread_id.is_none()
            && let Some(session_id) = payload.get("session_id").and_then(|value| value.as_str())
        {
            record.thread_id = Some(session_id.to_string());
        }
        if record.thread_path.is_none()
            && let Some(path) = payload
                .get("transcript_path")
                .and_then(|value| value.as_str())
        {
            record.thread_path = Some(path.to_string());
        }
        if record.thread_id.is_some() && record.thread_path.is_some() {
            break;
        }
    }
}

fn with_completion_marker(task: &str, marker: &str) -> String {
    format!(
        "{task}\n\nCompletion protocol:\nWhen this turn is complete, end your final response with this exact marker on its own line:\n{marker}"
    )
}

fn wait_for_turn_completion(
    tmux_bin: &str,
    session: &str,
    marker: &str,
    event_log: Option<&Path>,
    turn_started: u128,
    timeout_ms: u64,
) -> Result<serde_json::Value> {
    let started = turn_started;
    let mut saw_submission = false;
    loop {
        let capture = match tmux_capture(tmux_bin, session, 2_000, true, false) {
            Ok(capture) => capture,
            Err(error) => {
                return Ok(json!({
                    "ok": false,
                    "status": "failed",
                    "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                    "error": error.to_string()
                }));
            }
        };
        if marker_line_count(&capture, marker) >= 2 {
            return Ok(json!({
                "ok": true,
                "status": "completed",
                "completion_source": "visible_marker",
                "marker_seen": true,
                "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                "output_tail": structured_log_tail(&capture, 120)
            }));
        }
        let events = event_log
            .map(read_hook_events)
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event_captured_at_or_after(event, started))
            .collect::<Vec<_>>();
        if has_user_prompt_submit(&events) {
            saw_submission = true;
        }
        if let Some(pending) = pending_user_input_from_events(&events) {
            return Ok(json!({
                "ok": false,
                "status": "waiting_for_user",
                "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                "pending_user_input": pending,
                "output_tail": structured_log_tail(&capture, 120)
            }));
        }
        if let Some(stop) = last_stop_event(&events) {
            let last_agent_message = stop
                .pointer("/payload/last_assistant_message")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            return Ok(json!({
                "ok": true,
                "status": "completed",
                "completion_source": if last_agent_message.contains(marker) { "stop_hook_marker" } else { "stop_hook" },
                "marker_seen": last_agent_message.contains(marker),
                "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                "last_agent_message": last_agent_message,
                "output_tail": structured_log_tail(&capture, 120)
            }));
        }
        if !saw_submission
            && is_input_ready_prompt(&capture)
            && crate::io::now_millis().saturating_sub(started) >= 8_000
        {
            return Ok(json!({
                "ok": false,
                "status": "failed",
                "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                "error": "Claude prompt was not submitted",
                "output_tail": structured_log_tail(&capture, 120)
            }));
        }
        if crate::io::now_millis().saturating_sub(started) >= timeout_ms as u128 {
            return Ok(json!({
                "ok": false,
                "status": "timeout",
                "elapsed_ms": crate::io::now_millis().saturating_sub(started),
                "error": "timeout waiting for Claude completion marker",
                "output_tail": structured_log_tail(&capture, 120)
            }));
        }
        thread::sleep(Duration::from_millis(1_000));
    }
}

fn marker_line_count(capture: &str, marker: &str) -> usize {
    capture.lines().filter(|line| line.trim() == marker).count()
}

fn wait_for_input_prompt(tmux_bin: &str, session: &str, timeout_ms: u64) -> Result<()> {
    let started = crate::io::now_millis();
    let mut trust_enter_sent = false;
    loop {
        let capture = tmux_capture(tmux_bin, session, 80, true, true).unwrap_or_default();
        if is_workspace_trust_prompt(&capture) && !trust_enter_sent {
            let output = run_status(
                &tmux_send_keys_command(tmux_bin, session, &["Enter".to_string()]),
                None,
            )?;
            if !output.ok {
                bail!("tmux trust confirmation failed: {}", output.stderr.trim());
            }
            trust_enter_sent = true;
            thread::sleep(Duration::from_millis(1_000));
            continue;
        }
        if is_input_ready_prompt(&capture) {
            return Ok(());
        }
        if crate::io::now_millis().saturating_sub(started) >= timeout_ms as u128 {
            bail!("Claude did not become input-ready before timeout:\n{capture}");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn is_workspace_trust_prompt(capture: &str) -> bool {
    capture.contains("Quick safety check")
        || capture.contains("Yes, I trust this folder")
        || (capture.contains("Enter to confirm") && capture.contains("Esc to cancel"))
}

fn is_input_ready_prompt(capture: &str) -> bool {
    if is_workspace_trust_prompt(capture) {
        return false;
    }
    capture.lines().rev().take(20).any(|line| {
        let line = normalize_tmux_line(line);
        line == "\u{276f}" || line.starts_with("\u{276f} Try ")
    })
}

fn normalize_tmux_line(line: &str) -> String {
    line.replace('\u{a0}', " ").trim().to_string()
}

fn event_captured_at_or_after(event: &Value, min_captured_ms: u128) -> bool {
    event
        .get("captured_ms")
        .and_then(|value| value.as_u64())
        .is_none_or(|captured| captured as u128 >= min_captured_ms)
}

fn has_user_prompt_submit(events: &[Value]) -> bool {
    events.iter().any(|event| {
        event.get("kind").and_then(|value| value.as_str()) == Some("UserPromptSubmit")
            || event
                .pointer("/payload/hook_event_name")
                .and_then(|value| value.as_str())
                == Some("UserPromptSubmit")
    })
}

fn last_stop_event(events: &[Value]) -> Option<&Value> {
    events.iter().rev().find(|event| {
        event.get("kind").and_then(|value| value.as_str()) == Some("Stop")
            || event
                .pointer("/payload/hook_event_name")
                .and_then(|value| value.as_str())
                == Some("Stop")
    })
}

fn tmux_send_text(tmux_bin: &str, session: &str, text: &str, enter: bool) -> Result<()> {
    let load = vec![
        tmux_bin.to_string(),
        "load-buffer".to_string(),
        "-t".to_string(),
        session.to_string(),
        "-".to_string(),
    ];
    let output = run_stdin(&load, text.as_bytes(), None)?;
    if !output.ok {
        bail!("tmux load-buffer failed: {}", output.stderr.trim());
    }
    let paste = vec![
        tmux_bin.to_string(),
        "paste-buffer".to_string(),
        "-t".to_string(),
        session.to_string(),
        "-p".to_string(),
    ];
    let output = run_status(&paste, None)?;
    if !output.ok {
        bail!("tmux paste-buffer failed: {}", output.stderr.trim());
    }
    if enter {
        thread::sleep(Duration::from_millis(300));
        let output = run_status(
            &tmux_send_keys_command(tmux_bin, session, &["Enter".to_string()]),
            None,
        )?;
        if !output.ok {
            bail!("tmux enter failed: {}", output.stderr.trim());
        }
        thread::sleep(Duration::from_millis(700));
        let capture = tmux_capture(tmux_bin, session, 80, true, true).unwrap_or_default();
        if pasted_text_still_in_prompt(&capture) {
            let output = run_status(
                &tmux_send_keys_command(tmux_bin, session, &["Enter".to_string()]),
                None,
            )?;
            if !output.ok {
                bail!("tmux retry enter failed: {}", output.stderr.trim());
            }
        }
    }
    Ok(())
}

fn pasted_text_still_in_prompt(capture: &str) -> bool {
    capture
        .lines()
        .rev()
        .take(12)
        .any(|line| line.contains("\u{276f}") && line.contains("[Pasted text"))
}

fn tmux_send_keys_command(tmux_bin: &str, session: &str, keys: &[String]) -> Vec<String> {
    let mut command = vec![
        tmux_bin.to_string(),
        "send-keys".to_string(),
        "-t".to_string(),
        session.to_string(),
    ];
    command.extend(keys.iter().cloned());
    command
}

fn tmux_has_session(tmux_bin: &str, session: &str) -> Result<bool> {
    let output = run_capture(
        &[
            tmux_bin.to_string(),
            "has-session".to_string(),
            "-t".to_string(),
            session.to_string(),
        ],
        None,
    )?;
    Ok(output.ok)
}

fn kill_tmux_session_if_exists(tmux_bin: &str, session: &str) -> Result<()> {
    if !tmux_has_session(tmux_bin, session)? {
        return Ok(());
    }
    let output = run_capture(
        &[
            tmux_bin.to_string(),
            "kill-session".to_string(),
            "-t".to_string(),
            session.to_string(),
        ],
        None,
    )?;
    if !output.ok {
        bail!("tmux kill-session failed: {}", output.stderr.trim());
    }
    Ok(())
}

fn tmux_capture(
    tmux_bin: &str,
    session: &str,
    tail_lines: usize,
    strip_ansi: bool,
    visible: bool,
) -> Result<String> {
    let mut command = vec![
        tmux_bin.to_string(),
        "capture-pane".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        session.to_string(),
    ];
    if !visible {
        command.extend(["-S".to_string(), format!("-{}", tail_lines.max(1))]);
    }
    let output = run_capture(&command, None)?;
    if !output.ok {
        bail!("tmux capture failed: {}", output.stderr.trim());
    }
    if strip_ansi {
        Ok(strip_ansi_controls(&output.stdout))
    } else {
        Ok(output.stdout)
    }
}

fn debug_log_path(root: &Path, session: &str) -> std::path::PathBuf {
    pandacode_dir(root)
        .join("claude")
        .join("logs")
        .join(format!("{session}.debug.log"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::cli::{Effort, PermissionMode, RuntimeBins};

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
    fn builds_claude_command_without_print_mode() {
        let args = TaskCommandArgs {
            stdin: None,
            task: Some("fix".to_string()),
            task_file: None,
            cd: PathBuf::from("/repo"),
            session: "latest".to_string(),
            model: Some("sonnet".to_string()),
            effort: Some(Effort::High),
            permission: None,
            timeout_ms: None,
            json: false,
            bins: bins(),
        };
        let launch = ClaudeLaunch {
            tmux_name: "main",
            debug_log: Path::new("/tmp/debug.log"),
            settings_json: "{\"hooks\":{}}",
            resume_session_id: Some("claude-session-id"),
            model: "opus",
            effort: "max",
            permission: PermissionMode::Max,
        };
        let command = claude_command(&args.bins, &launch);
        assert!(command.contains(&"--name".to_string()));
        assert!(command.contains(&"opus".to_string()));
        assert!(command.contains(&"max".to_string()));
        assert!(command.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(!command.contains(&"--permission-mode".to_string()));
        assert!(!command.contains(&"--allowedTools".to_string()));
        assert!(command.contains(&"--brief".to_string()));
        assert!(command.contains(&"--settings".to_string()));
        assert!(command.contains(&"--setting-sources".to_string()));
        assert!(command.contains(&"local".to_string()));
        assert!(command.contains(&"--strict-mcp-config".to_string()));
        assert!(command.contains(&"--mcp-config".to_string()));
        assert!(command.contains(&"{\"mcpServers\":{}}".to_string()));
        assert!(command.contains(&"--resume".to_string()));
        assert!(command.contains(&"claude-session-id".to_string()));
        assert!(!command.contains(&"-p".to_string()));
        assert!(!command.contains(&"--output-format".to_string()));
        assert!(!command.contains(&"--json-schema".to_string()));
    }

    #[test]
    fn builds_claude_command_with_limited_permission() {
        let args = TaskCommandArgs {
            stdin: None,
            task: Some("fix".to_string()),
            task_file: None,
            cd: PathBuf::from("/repo"),
            session: "latest".to_string(),
            model: Some("sonnet".to_string()),
            effort: Some(Effort::High),
            permission: Some(PermissionMode::Limited),
            timeout_ms: None,
            json: false,
            bins: bins(),
        };
        let launch = ClaudeLaunch {
            tmux_name: "main",
            debug_log: Path::new("/tmp/debug.log"),
            settings_json: "{\"hooks\":{}}",
            resume_session_id: None,
            model: "opus",
            effort: "max",
            permission: PermissionMode::Limited,
        };
        let command = claude_command(&args.bins, &launch);
        assert!(!command.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(
            command
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "acceptEdits"])
        );
        assert!(command.contains(&"--strict-mcp-config".to_string()));
        assert!(command.contains(&"{\"mcpServers\":{}}".to_string()));
    }

    #[test]
    fn hook_settings_are_inline_and_call_installed_binary() {
        let root = std::env::temp_dir().join(format!(
            "pandacode-claude-inline-hooks-{}",
            crate::io::now_millis()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let settings = install_hook_recorder(&root, "main").unwrap();
        let settings_json = serde_json::from_str::<Value>(&settings).unwrap();
        let stop_command = settings_json
            .pointer("/hooks/Stop/0/hooks/0/command")
            .and_then(|value| value.as_str())
            .unwrap();

        assert!(settings.trim_start().starts_with('{'));
        assert!(stop_command.contains("claude-hook"));
        assert!(stop_command.contains("--event-log"));
        assert!(stop_command.contains(".pandacode/claude/events/main.jsonl"));
        assert!(
            stop_command.contains(
                &std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            )
        );
        assert!(!root.join(".pandacode/claude/hooks").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marker_prompt_is_exact() {
        let prompt = with_completion_marker("do work", "DONE");
        assert!(prompt.contains("do work"));
        assert!(prompt.ends_with("DONE"));
        assert_eq!(marker_line_count(&prompt, "DONE"), 1);
        assert_eq!(marker_line_count(&format!("{prompt}\nDONE\n"), "DONE"), 2);
    }

    #[test]
    fn detects_unsubmitted_collapsed_paste_prompt() {
        assert!(pasted_text_still_in_prompt(
            "────────────────\n❯ [Pasted text #1 +4 lines]\n────────────────"
        ));
        assert!(!pasted_text_still_in_prompt(
            "⏺ done\nPANDACODE_DONE\n────────────────\n❯ "
        ));
    }

    #[test]
    fn trust_prompt_is_not_input_ready() {
        let trust = "Quick safety check\n❯ 1. Yes, I trust this folder\n  2. No, exit\nEnter to confirm · Esc to cancel";
        assert!(is_workspace_trust_prompt(trust));
        assert!(!is_input_ready_prompt(trust));
        assert!(is_input_ready_prompt(
            "────────────────\n❯ Try \"fix lint errors\"\n────────────────"
        ));
        assert!(is_input_ready_prompt(
            "────────────────\n❯ \n────────────────"
        ));
    }

    #[test]
    fn stop_hook_events_carry_completion_messages() {
        let events = vec![
            json!({
                "captured_ms": 100,
                "kind": "UserPromptSubmit",
                "payload": {"hook_event_name": "UserPromptSubmit"}
            }),
            json!({
                "captured_ms": 101,
                "kind": "Stop",
                "payload": {
                    "hook_event_name": "Stop",
                    "last_assistant_message": "done\nPANDACODE_DONE_1_2"
                }
            }),
        ];
        assert!(has_user_prompt_submit(&events));
        assert_eq!(
            last_stop_event(&events)
                .and_then(|event| event.pointer("/payload/last_assistant_message"))
                .and_then(|value| value.as_str()),
            Some("done\nPANDACODE_DONE_1_2")
        );
    }

    #[test]
    fn pending_input_tracks_blocking_ask_not_idle() {
        let root = std::env::temp_dir().join(format!(
            "pandacode-claude-events-{}",
            crate::io::now_millis()
        ));
        std::fs::create_dir_all(root.join(".pandacode/claude/events")).unwrap();
        let path = event_log_path(&root, "main");

        std::fs::write(
            &path,
            r#"{"kind":"PreToolUse:AskUserQuestion","payload":{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion"}}
"#,
        )
        .unwrap();
        assert_eq!(
            pending_user_input(&path).unwrap()["kind"],
            "ask_user_question"
        );
        assert_eq!(
            claude_state(&path, Some("question"), true),
            "waiting_for_user"
        );

        std::fs::write(
            &path,
            r#"{"kind":"PreToolUse:AskUserQuestion","payload":{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion"}}
{"kind":"PostToolUse:AskUserQuestion","payload":{"hook_event_name":"PostToolUse","tool_name":"AskUserQuestion"}}
{"kind":"Stop","payload":{"hook_event_name":"Stop"}}
{"kind":"Notification","payload":{"hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"}}
"#,
        )
        .unwrap();
        assert!(pending_user_input(&path).is_none());
        assert_eq!(
            claude_state(&path, Some("────────────────\n❯ "), true),
            "idle"
        );

        std::fs::write(
            &path,
            r#"{"captured_ms":100,"kind":"Notification","payload":{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission"}}
"#,
        )
        .unwrap();
        assert!(pending_user_input(&path).is_some());
        let newer_events = read_hook_events(&path)
            .into_iter()
            .filter(|event| event_captured_at_or_after(event, 101))
            .collect::<Vec<_>>();
        assert!(pending_user_input_from_events(&newer_events).is_none());

        std::fs::remove_dir_all(root).unwrap();
    }
}
