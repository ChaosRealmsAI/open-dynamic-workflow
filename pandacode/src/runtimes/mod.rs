pub mod bamboo;
pub mod claude;
pub mod codex;
pub(crate) mod codex_appserver;

use std::{env, str::FromStr};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::{
    cli::{
        AgentAnswerCommandArgs, AgentLogsCommandArgs, AgentSessionCommandArgs,
        AgentTaskCommandArgs, BambooRuntimeCommand, BambooTaskCommandArgs, GlobalArgs,
        RuntimeCommand, RuntimeSelector,
    },
    config::ProviderKind,
    io::{output_json, run_capture, workspace},
    models, session,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedRuntime {
    Bamboo,
    Claude,
    Codex,
}

pub async fn run_agent_task(args: AgentTaskCommandArgs, action: &str) -> Result<()> {
    let runtime = resolve_runtime_for_task(&args, action).await?;
    match runtime {
        ResolvedRuntime::Bamboo => {
            let command = bamboo_task_command(action, args)?;
            bamboo::run(command).await
        }
        ResolvedRuntime::Claude => {
            reject_provider_for_delegated_runtime(args.provider.as_deref(), "claude")?;
            let command = match action {
                "exec" => RuntimeCommand::Exec(args.common),
                "resume" => RuntimeCommand::Resume(args.common),
                _ => return Err(anyhow!("unsupported agent task action {action}")),
            };
            claude::run(command)
        }
        ResolvedRuntime::Codex => {
            reject_provider_for_delegated_runtime(args.provider.as_deref(), "codex")?;
            let command = match action {
                "exec" => RuntimeCommand::Exec(args.common),
                "resume" => RuntimeCommand::Resume(args.common),
                _ => return Err(anyhow!("unsupported agent task action {action}")),
            };
            codex::run(command)
        }
    }
}

pub async fn answer_agent(args: AgentAnswerCommandArgs) -> Result<()> {
    let runtime =
        resolve_runtime_for_session(args.runtime, &args.common.cd, &args.common.session).await?;
    match runtime {
        ResolvedRuntime::Bamboo => bamboo::run(BambooRuntimeCommand::Answer(args.common)).await,
        ResolvedRuntime::Claude => claude::run(RuntimeCommand::Answer(args.common)),
        ResolvedRuntime::Codex => codex::run(RuntimeCommand::Answer(args.common)),
    }
}

pub async fn logs_agent(args: AgentLogsCommandArgs) -> Result<()> {
    let runtime =
        resolve_runtime_for_session(args.runtime, &args.common.cd, &args.common.session).await?;
    match runtime {
        ResolvedRuntime::Bamboo => bamboo::run(BambooRuntimeCommand::Logs(args.common)).await,
        ResolvedRuntime::Claude => claude::run(RuntimeCommand::Logs(args.common)),
        ResolvedRuntime::Codex => codex::run(RuntimeCommand::Logs(args.common)),
    }
}

pub async fn session_agent(args: AgentSessionCommandArgs, action: &str) -> Result<()> {
    let runtime =
        resolve_runtime_for_session(args.runtime, &args.common.cd, &args.common.session).await?;
    match (runtime, action) {
        (ResolvedRuntime::Bamboo, "status") => {
            bamboo::run(BambooRuntimeCommand::Status(args.common)).await
        }
        (ResolvedRuntime::Bamboo, "artifacts") => {
            bamboo::run(BambooRuntimeCommand::Artifacts(args.common)).await
        }
        (ResolvedRuntime::Bamboo, "interrupt") => {
            bamboo::run(BambooRuntimeCommand::Interrupt(args.common)).await
        }
        (ResolvedRuntime::Bamboo, "stop") => {
            bamboo::run(BambooRuntimeCommand::Stop(args.common)).await
        }
        (ResolvedRuntime::Claude, "status") => claude::run(RuntimeCommand::Status(args.common)),
        (ResolvedRuntime::Claude, "artifacts") => {
            claude::run(RuntimeCommand::Artifacts(args.common))
        }
        (ResolvedRuntime::Claude, "interrupt") => {
            claude::run(RuntimeCommand::Interrupt(args.common))
        }
        (ResolvedRuntime::Claude, "stop") => claude::run(RuntimeCommand::Stop(args.common)),
        (ResolvedRuntime::Codex, "status") => codex::run(RuntimeCommand::Status(args.common)),
        (ResolvedRuntime::Codex, "artifacts") => codex::run(RuntimeCommand::Artifacts(args.common)),
        (ResolvedRuntime::Codex, "interrupt") => codex::run(RuntimeCommand::Interrupt(args.common)),
        (ResolvedRuntime::Codex, "stop") => codex::run(RuntimeCommand::Stop(args.common)),
        (_, other) => Err(anyhow!("unsupported agent session action {other}")),
    }
}

fn bamboo_task_command(action: &str, args: AgentTaskCommandArgs) -> Result<BambooRuntimeCommand> {
    let provider = args.provider.or_else(|| {
        args.common
            .model
            .as_deref()
            .and_then(bamboo_provider_for_model)
            .map(|provider| provider.to_string())
    });
    let task = BambooTaskCommandArgs {
        common: args.common,
        provider,
        generation: Default::default(),
        run: Default::default(),
    };
    match action {
        "exec" => Ok(BambooRuntimeCommand::Exec(task)),
        "resume" => Ok(BambooRuntimeCommand::Resume(task)),
        _ => Err(anyhow!("unsupported Bamboo agent task action {action}")),
    }
}

async fn resolve_runtime_for_task(
    args: &AgentTaskCommandArgs,
    action: &str,
) -> Result<ResolvedRuntime> {
    if args.provider.is_some() && matches!(args.runtime, RuntimeSelector::Auto) {
        return Ok(ResolvedRuntime::Bamboo);
    }
    if action == "resume" {
        return resolve_runtime_for_session(args.runtime, &args.common.cd, &args.common.session)
            .await;
    }
    if matches!(args.runtime, RuntimeSelector::Auto)
        && let Some(model) = args.common.model.as_deref()
        && let Some(runtime) = runtime_hint_for_model(model)
    {
        return Ok(runtime);
    }
    resolve_runtime(args.runtime, &args.common.cd, &args.common.bins).await
}

async fn resolve_runtime_for_session(
    selector: RuntimeSelector,
    cd: &std::path::Path,
    session_name: &str,
) -> Result<ResolvedRuntime> {
    if selector != RuntimeSelector::Auto {
        return runtime_from_selector(selector);
    }
    if let Some(runtime) = runtime_from_env()? {
        return Ok(runtime);
    }
    let root = workspace(cd)?;
    let runtime = session::resolve_runtime_for_session(&root, session_name)?;
    runtime_from_str(&runtime)
}

async fn resolve_runtime(
    selector: RuntimeSelector,
    cd: &std::path::Path,
    bins: &crate::cli::RuntimeBins,
) -> Result<ResolvedRuntime> {
    if selector != RuntimeSelector::Auto {
        return runtime_from_selector(selector);
    }
    if let Some(runtime) = runtime_from_env()? {
        return Ok(runtime);
    }
    let root = workspace(cd)?;
    let bamboo = bamboo::doctor_report(&root, bins).await?;
    if bamboo
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Ok(ResolvedRuntime::Bamboo);
    }
    let claude = claude::doctor_report(&root, bins)?;
    if claude
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Ok(ResolvedRuntime::Claude);
    }
    let codex = codex::doctor_report(&root, bins)?;
    if codex
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Ok(ResolvedRuntime::Codex);
    }
    Err(anyhow!(
        "no usable PandaCode runtime found; run `pandacode doctor --json` or pass --runtime bamboo|claude|codex"
    ))
}

fn runtime_from_env() -> Result<Option<ResolvedRuntime>> {
    match env::var("PANDACODE_RUNTIME") {
        Ok(value) if !value.trim().is_empty() => runtime_from_str(&value).map(Some),
        _ => Ok(None),
    }
}

fn runtime_from_selector(selector: RuntimeSelector) -> Result<ResolvedRuntime> {
    match selector {
        RuntimeSelector::Auto => Err(anyhow!("auto runtime selector was not resolved")),
        RuntimeSelector::Bamboo => Ok(ResolvedRuntime::Bamboo),
        RuntimeSelector::Claude => Ok(ResolvedRuntime::Claude),
        RuntimeSelector::Codex => Ok(ResolvedRuntime::Codex),
    }
}

fn runtime_from_str(value: &str) -> Result<ResolvedRuntime> {
    match value.trim().to_ascii_lowercase().as_str() {
        "bamboo" => Ok(ResolvedRuntime::Bamboo),
        "claude" => Ok(ResolvedRuntime::Claude),
        "codex" => Ok(ResolvedRuntime::Codex),
        other => Err(anyhow!("unsupported PandaCode runtime {other:?}")),
    }
}

fn runtime_hint_for_model(model: &str) -> Option<ResolvedRuntime> {
    if bamboo_provider_for_model(model).is_some() {
        return Some(ResolvedRuntime::Bamboo);
    }
    if is_claude_model_hint(model) {
        return Some(ResolvedRuntime::Claude);
    }
    if is_codex_model_hint(model) {
        return Some(ResolvedRuntime::Codex);
    }
    None
}

fn bamboo_provider_for_model(model: &str) -> Option<ProviderKind> {
    models::provider_for_model(model).or_else(|| {
        let provider = ProviderKind::from_str(model).ok()?;
        models::DOMESTIC_PROVIDERS
            .contains(&provider)
            .then_some(provider)
    })
}

fn is_claude_model_hint(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    matches!(model.as_str(), "haiku" | "sonnet" | "opus" | "fable") || model.starts_with("claude-")
}

fn is_codex_model_hint(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

fn reject_provider_for_delegated_runtime(provider: Option<&str>, _runtime: &str) -> Result<()> {
    if let Some(provider) = provider {
        let _ = ProviderKind::from_str(provider)?;
        return Err(anyhow!(
            "--provider is only supported by Bamboo; pass --runtime bamboo or use `pandacode bamboo exec ...`"
        ));
    }
    Ok(())
}

pub async fn doctor(args: GlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let codex = codex::doctor_report(&root, &args.bins)?;
    let claude = claude::doctor_report(&root, &args.bins)?;
    let bamboo = bamboo::doctor_report(&root, &args.bins).await?;
    let report = json!({
        "ok": codex.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
            || claude.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
            || bamboo.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "state": "checked",
        "codex": codex,
        "claude": claude,
        "bamboo": bamboo
    });
    if args.json {
        output_json(&report)
    } else {
        println!("{}", format_doctor_summary(&report));
        Ok(())
    }
}

pub fn list_all(args: GlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let report = json!({
        "ok": true,
        "codex": session::list(&root, "codex")?,
        "claude": session::list(&root, "claude")?,
        "bamboo": session::list(&root, "bamboo")?
    });
    if args.json {
        output_json(&report)
    } else {
        println!("{}", format_session_list_summary(&report));
        Ok(())
    }
}

pub async fn models_all(args: GlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let codex = codex::models_report(&root, &args.bins)?;
    let claude = claude::models_report(&root, &args.bins)?;
    let bamboo = bamboo::models_report(&root, None).await?;
    let report = json!({
        "ok": true,
        "codex": codex,
        "claude": claude,
        "bamboo": bamboo
    });
    if args.json {
        output_json(&report)
    } else {
        println!("{}", format_models_summary(&report));
        Ok(())
    }
}

fn format_doctor_summary(report: &Value) -> String {
    let status = if report_bool(report, "ok") {
        "usable"
    } else {
        "needs setup"
    };
    let mut lines = vec![format!("PandaCode doctor: {status}")];
    for runtime in ["bamboo", "claude", "codex"] {
        let runtime_report = &report[runtime];
        let runtime_status = if report_bool(runtime_report, "ok") {
            "available".to_string()
        } else {
            report_str(runtime_report, "state")
                .unwrap_or("unavailable")
                .to_string()
        };
        let mut details = vec![format!("  - {runtime}: {runtime_status}")];
        if let Some(driver) = report_str(runtime_report, "driver") {
            details.push(format!("driver={driver}"));
        }
        if let Some(active) = format_bamboo_active(runtime_report) {
            details.push(format!("active={active}"));
        }
        let missing = string_array(&runtime_report["missing"]);
        if !missing.is_empty() {
            details.push(format!("missing={}", missing.join(",")));
        }
        lines.push(details.join(" "));
    }
    lines.push("JSON: pandacode doctor --json".to_string());
    lines.join("\n")
}

fn format_session_list_summary(report: &Value) -> String {
    let mut lines = vec!["PandaCode sessions".to_string()];
    for runtime in ["bamboo", "claude", "codex"] {
        let sessions = report[runtime].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let mut line = format!("  - {runtime}: {}", sessions.len());
        if let Some(latest) = sessions.first() {
            if let Some(session) = report_str(latest, "session") {
                line.push_str(&format!(" latest={session}"));
            }
            if let Some(model) = report_str(latest, "model") {
                line.push_str(&format!(" model={model}"));
            }
            if let Some(run_id) = report_str(latest, "run_id") {
                line.push_str(&format!(" run={run_id}"));
            }
        }
        lines.push(line);
    }
    lines.push("JSON: pandacode list --json".to_string());
    lines.join("\n")
}

fn format_models_summary(report: &Value) -> String {
    let mut lines = vec!["PandaCode models".to_string()];
    lines.push(format_model_runtime_summary("bamboo", &report["bamboo"]));
    lines.push(format_model_runtime_summary("claude", &report["claude"]));
    lines.push(format_model_runtime_summary("codex", &report["codex"]));
    lines.push("JSON: pandacode models --json".to_string());
    lines.join("\n")
}

fn format_model_runtime_summary(runtime: &str, report: &Value) -> String {
    let mut parts = vec![format!(
        "  - {runtime}: {}",
        if report_bool(report, "ok") {
            "ok"
        } else {
            "unavailable"
        }
    )];
    match runtime {
        "bamboo" => {
            let models = report["raw"]["models"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            parts.push(format!("models={}", models.len()));
            let defaults = models
                .iter()
                .filter(|model| report_bool(model, "is_default"))
                .filter_map(format_provider_model)
                .collect::<Vec<_>>();
            if !defaults.is_empty() {
                parts.push(format!("defaults={}", join_limited(defaults, 4)));
            }
        }
        "claude" => {
            let aliases = string_array(&report["known_aliases"]);
            if !aliases.is_empty() {
                parts.push(format!("aliases={}", aliases.join(", ")));
            }
        }
        "codex" => {
            let models = report["raw"]["models"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            parts.push(format!("models={}", models.len()));
            let defaults = models
                .iter()
                .filter(|model| report_bool(model, "is_default"))
                .filter_map(format_model_id)
                .collect::<Vec<_>>();
            if !defaults.is_empty() {
                parts.push(format!("default={}", join_limited(defaults, 1)));
            }
        }
        _ => {}
    }
    parts.join(" ")
}

fn format_bamboo_active(report: &Value) -> Option<String> {
    let active = &report["active"];
    let provider = report_str(active, "provider")?;
    let model = report_str(active, "model")?;
    Some(format!("{provider}/{model}"))
}

fn format_provider_model(model: &Value) -> Option<String> {
    let provider = report_str(model, "provider")?;
    let id = report_str(model, "id")
        .or_else(|| report_str(model, "model"))
        .unwrap_or("unknown");
    Some(format!("{provider}/{id}"))
}

fn format_model_id(model: &Value) -> Option<String> {
    report_str(model, "id")
        .or_else(|| report_str(model, "model"))
        .map(ToString::to_string)
}

fn report_bool(report: &Value, key: &str) -> bool {
    report.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn report_str<'a>(report: &'a Value, key: &str) -> Option<&'a str> {
    report.get(key).and_then(Value::as_str)
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn join_limited(values: Vec<String>, limit: usize) -> String {
    if values.len() <= limit {
        return values.join(", ");
    }
    let hidden = values.len() - limit;
    let mut visible = values.into_iter().take(limit).collect::<Vec<_>>();
    visible.push(format!("+{hidden}"));
    visible.join(", ")
}

pub fn wait_sessions(args: crate::cli::WaitCommandArgs) -> Result<()> {
    let root = crate::io::workspace(&args.cd)?;
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_millis(args.timeout_ms);
    // Grace window for a freshly-launched lane to write its first record.
    // After it elapses, a session still missing a record is treated as a
    // never-started lane (typo'd name / launch failed) and fails fast instead
    // of silently waiting out the full timeout.
    let grace = std::time::Duration::from_millis(args.interval_ms.saturating_mul(3).max(10_000));
    fn terminal(state: &str) -> bool {
        matches!(
            state,
            "completed" | "failed" | "timeout" | "stopped" | "interrupted" | "no_report" | "blocked"
        )
    }
    loop {
        let mut lanes = serde_json::Map::new();
        let mut all_settled = true;
        let mut any_waiting = false;
        let mut pending_names = Vec::new();
        for name in &args.sessions {
            let (runtime, state) = match session::resolve_runtime_for_session(&root, name) {
                Ok(runtime) => match session::load(&root, &runtime, name) {
                    Ok(record) => {
                        let state = record.artifacts["status"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string();
                        (runtime, state)
                    }
                    Err(_) => (runtime, "pending".to_string()),
                },
                Err(_) => ("unknown".to_string(), "pending".to_string()),
            };
            if state == "pending" {
                pending_names.push(name.clone());
            }
            if state == "waiting_for_user" {
                any_waiting = true;
            } else if !terminal(&state) {
                all_settled = false;
            }
            lanes.insert(
                name.clone(),
                serde_json::json!({ "runtime": runtime, "state": state }),
            );
        }
        // Fast-fail: a lane that never produced a record after the grace window.
        if !pending_names.is_empty() && started.elapsed() >= grace {
            crate::io::output_json(&serde_json::json!({
                "ok": false,
                "action": "wait",
                "state": "missing_session",
                "sessions": lanes,
                "missing_sessions": pending_names,
                "error": "session(s) never started a turn; check the session name(s)",
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }))?;
            return Err(crate::io::JsonAlreadyEmitted.into());
        }
        let timed_out = std::time::Instant::now() >= deadline;
        if all_settled || any_waiting || timed_out {
            let missing = args
                .expect_artifact
                .iter()
                .filter(|path| !root.join(path).exists())
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            let all_completed = lanes.values().all(|lane| lane["state"] == "completed");
            let ok = all_completed && missing.is_empty() && !any_waiting;
            let state = if ok {
                "completed"
            } else if any_waiting {
                "waiting_for_user"
            } else if timed_out && !all_settled {
                "timeout"
            } else if all_completed && !missing.is_empty() {
                "no_report"
            } else {
                "failed"
            };
            crate::io::output_json(&serde_json::json!({
                "ok": ok,
                "action": "wait",
                "state": state,
                "sessions": lanes,
                "missing_artifacts": missing,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }))?;
            if ok {
                return Ok(());
            }
            return Err(crate::io::JsonAlreadyEmitted.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(args.interval_ms));
    }
}

pub fn gc_sessions(args: crate::cli::GcCommandArgs) -> Result<()> {
    let root = crate::io::workspace(&args.cd)?;
    let base = crate::io::pandacode_dir(&root);
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(args.days.saturating_mul(86_400)))
        .unwrap_or(std::time::UNIX_EPOCH);
    // Only PandaCode-owned transient outputs. Never touch `sessions/` (state),
    // `codex/codex-home` (thread history), or `codex/appserver-pids`.
    let prunable = ["prompts", "logs", "events", "detached"];
    let mut removed = Vec::new();
    let mut bytes_freed: u64 = 0;
    for runtime in ["codex", "claude", "bamboo"] {
        for sub in prunable {
            let dir = base.join(runtime).join(sub);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let old = meta
                    .modified()
                    .map(|m| m < cutoff)
                    .unwrap_or(false);
                if !old {
                    continue;
                }
                bytes_freed += meta.len();
                removed.push(path.to_string_lossy().to_string());
                if !args.dry_run {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    crate::io::output_json(&serde_json::json!({
        "ok": true,
        "action": "gc",
        "dry_run": args.dry_run,
        "days": args.days,
        "removed_count": removed.len(),
        "bytes_freed": bytes_freed,
        "removed": removed,
    }))
}

fn version_report(program: &str, args: &[&str]) -> serde_json::Value {
    let command = std::iter::once(program.to_string())
        .chain(args.iter().map(|arg| arg.to_string()))
        .collect::<Vec<_>>();
    match run_capture(&command, None) {
        Ok(output) => json!({
            "ok": output.ok,
            "command": command,
            "exit_code": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr
        }),
        Err(error) => json!({
            "ok": false,
            "command": command,
            "error": error.to_string()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedRuntime, bamboo_provider_for_model, format_doctor_summary, format_models_summary,
        format_session_list_summary, runtime_hint_for_model,
    };
    use crate::config::ProviderKind;
    use serde_json::json;

    #[test]
    fn model_hints_route_to_matching_runtime() {
        assert_eq!(
            runtime_hint_for_model("kimi-k2.6"),
            Some(ResolvedRuntime::Bamboo)
        );
        assert_eq!(
            runtime_hint_for_model("deepseek-v4-pro"),
            Some(ResolvedRuntime::Bamboo)
        );
        assert_eq!(
            runtime_hint_for_model("opus"),
            Some(ResolvedRuntime::Claude)
        );
        assert_eq!(
            runtime_hint_for_model("fable"),
            Some(ResolvedRuntime::Claude)
        );
        assert_eq!(
            runtime_hint_for_model("claude-sonnet-4-5"),
            Some(ResolvedRuntime::Claude)
        );
        assert_eq!(
            runtime_hint_for_model("gpt-5.5"),
            Some(ResolvedRuntime::Codex)
        );
        assert_eq!(runtime_hint_for_model("unknown-model"), None);
    }

    #[test]
    fn bamboo_provider_is_inferred_from_model_id() {
        assert_eq!(
            bamboo_provider_for_model("kimi-k2.6"),
            Some(ProviderKind::Kimi)
        );
        assert_eq!(
            bamboo_provider_for_model("MiniMax-M3"),
            Some(ProviderKind::Minimax)
        );
        assert_eq!(bamboo_provider_for_model("opus"), None);
    }

    #[test]
    fn doctor_summary_highlights_runtime_state() {
        let summary = format_doctor_summary(&json!({
            "ok": true,
            "bamboo": {
                "ok": false,
                "state": "configuration_needed",
                "driver": "bamboo-native",
                "missing": ["api_key"],
                "active": {
                    "provider": "deepseek",
                    "model": "deepseek-v4-pro"
                }
            },
            "claude": {
                "ok": true,
                "state": "available",
                "driver": "tmux",
                "missing": []
            },
            "codex": {
                "ok": true,
                "state": "available",
                "driver": "codex app-server",
                "missing": []
            }
        }));

        assert!(summary.contains("PandaCode doctor: usable"));
        assert!(summary.contains("bamboo: configuration_needed"));
        assert!(summary.contains("active=deepseek/deepseek-v4-pro"));
        assert!(summary.contains("missing=api_key"));
        assert!(summary.contains("claude: available"));
        assert!(summary.contains("codex: available"));
        assert!(summary.contains("JSON: pandacode doctor --json"));
        assert!(!summary.contains('{'));
    }

    #[test]
    fn models_summary_compacts_runtime_catalogs() {
        let summary = format_models_summary(&json!({
            "ok": true,
            "bamboo": {
                "ok": true,
                "raw": {
                    "models": [
                        {
                            "provider": "deepseek",
                            "id": "deepseek-v4-pro",
                            "is_default": true
                        },
                        {
                            "provider": "kimi",
                            "id": "kimi-k2.6",
                            "is_default": true
                        }
                    ]
                }
            },
            "claude": {
                "ok": true,
                "known_aliases": ["haiku", "sonnet", "opus", "fable"]
            },
            "codex": {
                "ok": true,
                "raw": {
                    "models": [
                        {
                            "id": "gpt-5.5",
                            "is_default": true
                        },
                        {
                            "id": "gpt-5.4",
                            "is_default": false
                        }
                    ]
                }
            }
        }));

        assert!(summary.contains("bamboo: ok models=2"));
        assert!(summary.contains("defaults=deepseek/deepseek-v4-pro, kimi/kimi-k2.6"));
        assert!(summary.contains("claude: ok aliases=haiku, sonnet, opus, fable"));
        assert!(summary.contains("codex: ok models=2 default=gpt-5.5"));
        assert!(summary.contains("JSON: pandacode models --json"));
        assert!(!summary.contains('{'));
    }

    #[test]
    fn session_list_summary_counts_and_shows_latest() {
        let summary = format_session_list_summary(&json!({
            "ok": true,
            "bamboo": [],
            "claude": [],
            "codex": [
                {
                    "session": "latest",
                    "model": "gpt-5.5",
                    "run_id": "run_123"
                }
            ]
        }));

        assert!(summary.contains("bamboo: 0"));
        assert!(summary.contains("claude: 0"));
        assert!(summary.contains("codex: 1 latest=latest model=gpt-5.5 run=run_123"));
        assert!(summary.contains("JSON: pandacode list --json"));
        assert!(!summary.contains('{'));
    }
}
