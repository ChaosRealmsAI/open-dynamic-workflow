pub mod bamboo;
pub mod claude;
pub mod codex;

use std::{env, str::FromStr};

use anyhow::{Result, anyhow};
use serde_json::json;

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
    matches!(model.as_str(), "haiku" | "sonnet" | "opus") || model.starts_with("claude-")
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
    output_json(&json!({
        "ok": codex.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
            || claude.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
            || bamboo.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "state": "checked",
        "codex": codex,
        "claude": claude,
        "bamboo": bamboo
    }))
}

pub fn list_all(args: GlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    output_json(&json!({
        "ok": true,
        "codex": session::list(&root, "codex")?,
        "claude": session::list(&root, "claude")?,
        "bamboo": session::list(&root, "bamboo")?
    }))
}

pub async fn models_all(args: GlobalArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let codex = codex::models_report(&root, &args.bins)?;
    let claude = claude::models_report(&root, &args.bins)?;
    let bamboo = bamboo::models_report(&root, None).await?;
    output_json(&json!({
        "ok": true,
        "codex": codex,
        "claude": claude,
        "bamboo": bamboo
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
    use super::{ResolvedRuntime, bamboo_provider_for_model, runtime_hint_for_model};
    use crate::config::ProviderKind;

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
}
