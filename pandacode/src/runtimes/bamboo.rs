use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::{
    agent::{self, RunOptions, RunReport},
    cli::{
        AnswerCommandArgs, BambooGenerationArgs, BambooModelCommandArgs, BambooRuntimeCommand,
        BambooRuntimeGlobalArgs, BambooTaskCommandArgs, Effort, LogsCommandArgs, PermissionMode,
        ProviderOverrides, SessionCommandArgs, TaskCommandArgs,
    },
    client::{ReasoningOptions, RequestParams},
    config::{self, ProviderKind},
    fx,
    io::{
        generated_session, output_json, read_task, sanitize_name, structured_log_tail, tail,
        workspace, write_prompt_file,
    },
    models, prompt, session,
    session::SessionRecord,
};

const RUNTIME: &str = "bamboo";
const DEFAULT_PROVIDER: &str = "deepseek";
const DEFAULT_EFFORT: &str = "high";

pub async fn run(command: BambooRuntimeCommand) -> Result<()> {
    match command {
        BambooRuntimeCommand::Exec(args) => exec(args).await,
        BambooRuntimeCommand::Resume(args) => resume(args, "resume").await,
        BambooRuntimeCommand::Answer(args) => answer(args).await,
        BambooRuntimeCommand::Status(args) => status(args),
        BambooRuntimeCommand::Logs(args) => logs(args),
        BambooRuntimeCommand::Artifacts(args) => artifacts(args),
        BambooRuntimeCommand::Model(args) => model(args),
        BambooRuntimeCommand::Models(args) => models_cmd(args).await,
        BambooRuntimeCommand::Interrupt(args) => interrupt_or_stop(args, "interrupt"),
        BambooRuntimeCommand::Stop(args) => interrupt_or_stop(args, "stop"),
        BambooRuntimeCommand::List(args) => list(args),
        BambooRuntimeCommand::Doctor(args) => doctor(args).await,
    }
}

async fn exec(args: BambooTaskCommandArgs) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    let session_name = if args.common.session == "latest" {
        generated_session(RUNTIME)
    } else {
        sanitize_name(&args.common.session, RUNTIME)
    };
    let record = SessionRecord::new(RUNTIME, &session_name, "bamboo-native", &root);
    run_turn(root, args, record, None, "exec").await
}

async fn resume(args: BambooTaskCommandArgs, action: &str) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    let record = session::load(&root, RUNTIME, &args.common.session)?;
    let resume_target = record.run_id.clone();
    run_turn(root, args, record, resume_target, action).await
}

async fn answer(args: AnswerCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let pending = record
        .artifacts
        .get("pending_user_input")
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Bamboo session {} is not waiting for user input",
                record.session
            )
        })?;
    let answer = match (args.choice, args.text.as_deref()) {
        (Some(choice), None) if choice > 0 => {
            format!(
                "Pending Bamboo question:\n{}\n\nUser selected option {choice}. Continue from that answer and finish the task.",
                serde_json::to_string_pretty(&pending)?
            )
        }
        (None, Some(text)) => {
            format!(
                "Pending Bamboo question:\n{}\n\nUser answer:\n{text}\n\nContinue from that answer and finish the task.",
                serde_json::to_string_pretty(&pending)?
            )
        }
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
    let task_args = BambooTaskCommandArgs {
        common: TaskCommandArgs {
            stdin: None,
            task: Some(answer),
            task_file: None,
            cd: root,
            session: record.session,
            model: None,
            effort: None,
            permission: None,
            timeout_ms: args.timeout_ms,
            json: args.json,
            bins: args.bins,
        },
        provider: None,
        generation: BambooGenerationArgs::default(),
        run: Default::default(),
    };
    resume(task_args, "answer").await
}

async fn run_turn(
    root: PathBuf,
    args: BambooTaskCommandArgs,
    mut record: SessionRecord,
    resume_target: Option<String>,
    action: &str,
) -> Result<()> {
    let raw_task = read_task(
        args.common.task.as_deref(),
        args.common.task_file.as_deref(),
        args.common.stdin.as_deref(),
        Some(&root),
    )?;
    let provider = effective_provider(args.provider.as_deref(), provider_from_record(&record))?;
    let model = effective_model(args.common.model.as_deref(), record.model.as_deref());
    let effort = effective_effort(args.common.effort, record.effort.as_deref());
    let permission = effective_permission(args.common.permission, record.permission.as_deref());
    let generation = effective_generation(&args.generation, &record)?;
    let price_file = args.run.price_file.clone();
    let run_store = bamboo_run_store(&root);
    let resume_context = resume_context(resume_target.as_deref(), &run_store)?;
    let task = compose_run_task(raw_task, resume_context.as_deref());
    let run_id = resume_target
        .as_deref()
        .and_then(resume_run_id)
        .map(|id| format!("{id}-resume-{}", crate::io::now_millis()))
        .unwrap_or_else(default_run_id);
    let run_dir = run_store.join(&run_id);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let event_log = run_dir.join("events.jsonl");
    let prompt_file = write_prompt_file(&root, RUNTIME, &record.session, &task)?;

    let overrides = ProviderOverrides {
        provider: Some(provider),
        base_url: None,
        api_key: None,
        model: model.clone(),
    };
    let config = config::resolve(&overrides, generation.max_tokens)?;
    validate_generation(provider, &config.model, &generation)?;
    let reasoning = reasoning_options(&effort, generation.thinking.as_deref());
    let report = agent::run(RunOptions {
        config: config.clone(),
        system: None,
        task,
        cwd: root.clone(),
        permission,
        max_steps: args.run.max_steps.unwrap_or(60),
        max_input_tokens: args.run.max_input_tokens,
        max_output_tokens: args.run.max_output_tokens,
        max_total_tokens: args.run.max_total_tokens,
        max_cost: args.run.max_cost,
        max_cost_currency: args.run.max_cost_currency.clone(),
        price_file: price_file.clone(),
        fx: fx::resolve_if_needed(price_file.as_ref(), args.run.max_cost).await,
        verify_commands: args.run.verify.clone(),
        auto_verify: args.run.auto_verify,
        shell_timeout_ms: args.run.shell_timeout_ms.unwrap_or(120_000),
        model_timeout_ms: args.run.model_timeout_ms.unwrap_or(180_000),
        run_timeout_ms: args
            .run
            .run_timeout_ms
            .or(args.common.timeout_ms)
            .unwrap_or(1_800_000),
        history_keep_last: args.run.history_keep_last.unwrap_or(48),
        compact_threshold_tokens: args.run.compact_threshold_tokens,
        compact_reserve_tokens: args.run.compact_reserve_tokens.unwrap_or(32_000),
        temperature: generation.temperature,
        params: generation.params.clone(),
        reasoning,
        cache_prefix: prompt::cache_prefix_from_args(
            &args.run.cache_prefix,
            &args.run.cache_prefix_file,
        )?,
        cache_key: args.run.cache_key.clone(),
        cache_retention: args.run.cache_retention.clone(),
        cache_report: true,
        cache_warm: args.run.cache_warm,
        cache_warm_rounds: args.run.cache_warm_rounds,
        emit_events: false,
        event_log: Some(event_log.clone()),
        run_id: Some(run_id),
    })
    .await?;

    write_run_artifacts(&run_dir, &event_log, &report, &config.redacted())?;
    record.run_id = Some(report.run_id.clone());
    record.model = Some(report.model.clone());
    record.effort = Some(effort.clone());
    record.permission = Some(permission.as_value().to_string());
    set_generation(&mut record, &generation);
    record.artifacts = json!({
        "provider": provider,
        "generation": generation.record_json(),
        "prompt_file": prompt_file,
        "event_log": event_log,
        "run_store": run_store,
        "run_dir": run_dir,
        "report": record.run_id.as_ref().map(|id| bamboo_run_store(&root).join(id).join("report.json")),
        "resume_context": record.run_id.as_ref().map(|id| bamboo_run_store(&root).join(id).join("resume-context.md")),
        "pending_user_input": report.pending_user_input.clone()
    });
    session::save(&root, &mut record)?;

    let ok = matches!(report.status.as_str(), "success" | "waiting_for_user");
    let state = bamboo_state(&report.status);
    output_json(&json!({
        "ok": ok,
        "runtime": RUNTIME,
        "action": action,
        "session": record.session,
        "state": state,
        "driver": "bamboo-native",
        "pending_user_input": report.pending_user_input.clone(),
        "summary": report_summary(&report),
        "record": record
    }))?;
    if ok {
        Ok(())
    } else {
        Err(crate::io::JsonAlreadyEmitted.into())
    }
}

fn status(args: SessionCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let Some(run_id) = record.run_id.as_deref() else {
        output_json(&json!({
            "ok": false,
            "runtime": RUNTIME,
            "action": "status",
            "session": record.session,
            "state": "unknown",
            "error": "session has no Bamboo run_id yet",
            "record": record
        }))?;
        return Ok(());
    };
    let show = load_run_show(&bamboo_run_store(&root), run_id)?;
    output_json(&json!({
        "ok": matches!(show.run.status.as_str(), "success" | "waiting_for_user"),
        "runtime": RUNTIME,
        "action": "status",
        "session": record.session,
        "state": bamboo_state(&show.run.status),
        "summary": show,
        "pending_user_input": record.artifacts.get("pending_user_input").cloned().unwrap_or(Value::Null),
        "record": record
    }))
}

fn logs(args: LogsCommandArgs) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    let event_log = record
        .artifacts
        .get("event_log")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let capture = event_log
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| tail(&text, args.tail))
        .unwrap_or_else(|| "no Bamboo event log recorded for this session".to_string());
    if args.json {
        output_json(&json!({
            "ok": true,
            "runtime": RUNTIME,
            "action": "logs",
            "session": record.session,
            "tail": args.tail,
            "event_log": event_log,
            "log_tail": structured_log_tail(&capture, args.tail),
            "raw": {
                "log_chars": capture.len()
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

fn model(args: BambooModelCommandArgs) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    let mut record = session::load(&root, RUNTIME, &args.common.session)?;
    record.model = Some(args.common.model);
    record.effort = args
        .common
        .effort
        .map(|effort| effort.as_value().to_string());
    if let Some(provider) = args.provider {
        let provider = parse_provider(&provider)?;
        set_provider(&mut record, provider);
    }
    let generation = effective_generation(&args.generation, &record)?;
    set_generation(&mut record, &generation);
    session::save(&root, &mut record)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "model",
        "note": "Bamboo provider/model/effort are applied on the next resume turn.",
        "record": record
    }))
}

async fn models_cmd(args: BambooRuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    output_json(&models_report(&root, args.provider.as_deref()).await?)
}

fn interrupt_or_stop(args: SessionCommandArgs, action: &str) -> Result<()> {
    let root = workspace(&args.cd)?;
    let record = session::load(&root, RUNTIME, &args.session)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": action,
        "session": record.session,
        "state": "unsupported",
        "summary": {
            "reason": "Bamboo native runtime executes foreground turns in-process; stop it by terminating the active pandacode bamboo process. Detached interrupt/stop is not implemented."
        },
        "record": record
    }))
}

fn list(args: BambooRuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    output_json(&json!({
        "ok": true,
        "runtime": RUNTIME,
        "local": session::list(&root, RUNTIME)?,
        "runs": list_runs(&bamboo_run_store(&root), 50)?
    }))
}

async fn doctor(args: BambooRuntimeGlobalArgs) -> Result<()> {
    let root = workspace(&args.common.cd)?;
    output_json(&doctor_report(&root, &args.common.bins).await?)
}

pub async fn doctor_report(root: &Path, _bins: &crate::cli::RuntimeBins) -> Result<Value> {
    let provider = effective_provider(None, None)?;
    let partial = config::resolve_partial(
        &ProviderOverrides {
            provider: Some(provider),
            base_url: None,
            api_key: None,
            model: None,
        },
        None,
    )?;
    let api_key_present = !partial.api_key.trim().is_empty();
    let price_file = std::env::var("PANDACODE_BAMBOO_PRICE_FILE")
        .or_else(|_| std::env::var("BAMBOO_PRICE_FILE"))
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            let candidate = crate::io::pandacode_dir(root)
                .join("bamboo")
                .join("pricing.cn.json");
            candidate.is_file().then_some(candidate)
        });
    let model_known = models::builtin_model(partial.provider, &partial.model).is_some();
    let missing = [
        (!api_key_present).then_some("api_key"),
        (!model_known).then_some("known_model"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let warnings = if api_key_present {
        Vec::<String>::new()
    } else {
        vec![
            "Set PANDACODE_BAMBOO_API_KEY, BAMBOO_API_KEY, or a provider-specific key before live runs."
                .to_string(),
        ]
    };
    let catalog = models_report(root, None).await?;
    Ok(json!({
        "ok": api_key_present && model_known,
        "state": if api_key_present && model_known { "available" } else { "configuration_needed" },
        "runtime": RUNTIME,
        "workspace": root,
        "driver": "bamboo-native",
        "requirements": ["provider", "model", "api_key"],
        "missing": missing,
        "active": {
            "provider": partial.provider,
            "model": partial.model,
            "base_url": partial.base_url,
            "api_key_present": api_key_present,
            "model_known": model_known,
            "price_file": price_file.as_ref().map(|path| path.display().to_string()),
            "price_file_present": price_file.as_ref().is_some_and(|path| path.is_file())
        },
        "capabilities": {
            "task_execution": true,
            "resume": true,
            "answer": true,
            "interrupt": false,
            "stop": false,
            "model": true,
            "effort": true,
            "permissions_supported": ["max", "limited"],
            "timeout": true,
            "token_budget": true,
            "cost_budget": true,
            "provider_cache": true,
            "auto_compact": true,
            "verify_commands": true
        },
        "warnings": warnings,
        "live_check": "not_run; doctor does not spend provider quota",
        "models": catalog
    }))
}

pub async fn models_report(root: &Path, provider: Option<&str>) -> Result<Value> {
    let providers = if let Some(provider) = provider {
        vec![parse_provider(provider)?]
    } else {
        models::DOMESTIC_PROVIDERS.to_vec()
    };
    let catalog = if provider.is_some() {
        models::builtin_models(providers[0]).collect::<Vec<_>>()
    } else {
        models::builtin_models_for(&providers).collect::<Vec<_>>()
    };
    Ok(json!({
        "ok": true,
        "runtime": RUNTIME,
        "action": "models",
        "driver": "bamboo-native",
        "capabilities": {
            "model": true,
            "effort": true,
            "permissions_supported": ["max", "limited"],
            "timeout": true,
            "token_budget": true,
            "cost_budget": true,
            "provider_cache": true,
            "auto_compact": true,
            "verify_commands": true
        },
        "workspace": root,
        "raw": {
            "source": "builtin",
            "providers": providers,
            "models": catalog,
            "provider_params": models::provider_param_specs(&providers)
        }
    }))
}

fn bamboo_run_store(root: &Path) -> PathBuf {
    crate::io::pandacode_dir(root).join(RUNTIME).join("runs")
}

fn effective_provider(explicit: Option<&str>, stored: Option<&str>) -> Result<ProviderKind> {
    if let Some(provider) = explicit.or(stored) {
        return parse_provider(provider);
    }
    if let Ok(provider) = std::env::var("PANDACODE_BAMBOO_PROVIDER") {
        return parse_provider(&provider);
    }
    parse_provider(DEFAULT_PROVIDER)
}

fn parse_provider(value: &str) -> Result<ProviderKind> {
    value
        .parse::<ProviderKind>()
        .with_context(|| format!("unknown Bamboo provider {value:?}"))
}

fn effective_model(explicit: Option<&str>, stored: Option<&str>) -> Option<String> {
    explicit.or(stored).map(ToString::to_string)
}

fn effective_effort(explicit: Option<Effort>, stored: Option<&str>) -> String {
    explicit
        .map(|effort| effort.as_value())
        .or(stored)
        .unwrap_or(DEFAULT_EFFORT)
        .to_string()
}

fn effective_permission(explicit: Option<PermissionMode>, stored: Option<&str>) -> PermissionMode {
    explicit.unwrap_or_else(|| PermissionMode::from_record(stored))
}

fn reasoning_options(effort: &str, thinking: Option<&str>) -> Option<ReasoningOptions> {
    if effort == "none" || thinking == Some("disabled") {
        Some(ReasoningOptions {
            thinking_type: "disabled".to_string(),
            reasoning_effort: None,
        })
    } else {
        Some(ReasoningOptions {
            thinking_type: "enabled".to_string(),
            reasoning_effort: Some(effort.to_string()),
        })
    }
}

#[derive(Debug, Clone)]
struct EffectiveGeneration {
    thinking: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    params: RequestParams,
}

impl EffectiveGeneration {
    fn record_json(&self) -> Value {
        json!({
            "thinking": self.thinking,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
            "top_p": self.params.top_p,
            "presence_penalty": self.params.presence_penalty,
            "frequency_penalty": self.params.frequency_penalty,
            "stop": self.params.stop,
            "extra": self.params.extra,
        })
    }
}

fn effective_generation(
    args: &BambooGenerationArgs,
    record: &SessionRecord,
) -> Result<EffectiveGeneration> {
    let mut generation = generation_from_record(record);
    if let Some(thinking) = args.thinking {
        generation.thinking = Some(thinking.as_api_value().to_string());
    }
    if let Some(max_tokens) = args.max_tokens {
        generation.max_tokens = Some(max_tokens);
    }
    if let Some(temperature) = args.temperature {
        generation.temperature = Some(temperature);
    }
    if let Some(top_p) = args.top_p {
        generation.params.top_p = Some(top_p);
    }
    if let Some(presence_penalty) = args.presence_penalty {
        generation.params.presence_penalty = Some(presence_penalty);
    }
    if let Some(frequency_penalty) = args.frequency_penalty {
        generation.params.frequency_penalty = Some(frequency_penalty);
    }
    if !args.stop.is_empty() {
        generation.params.stop = args.stop.clone();
    }
    for param in &args.param {
        let (key, value) = parse_extra_param(param)?;
        generation.params.extra.insert(key, value);
    }
    Ok(generation)
}

fn generation_from_record(record: &SessionRecord) -> EffectiveGeneration {
    let stored = record.artifacts.get("generation");
    let mut params = RequestParams::default();
    if let Some(value) = stored {
        params.top_p = json_f32(value.get("top_p"));
        params.presence_penalty = json_f32(value.get("presence_penalty"));
        params.frequency_penalty = json_f32(value.get("frequency_penalty"));
        params.stop = value
            .get("stop")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        params.extra = value
            .get("extra")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
    }
    EffectiveGeneration {
        thinking: stored
            .and_then(|value| value.get("thinking"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        max_tokens: stored
            .and_then(|value| value.get("max_tokens"))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        temperature: stored.and_then(|value| json_f32(value.get("temperature"))),
        params,
    }
}

fn parse_extra_param(raw: &str) -> Result<(String, Value)> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("--param must use KEY=JSON, got {raw:?}"))?;
    let key = key.trim();
    if key.is_empty() {
        return Err(anyhow!("--param key cannot be empty"));
    }
    let value = serde_json::from_str(value.trim())
        .with_context(|| format!("failed to parse JSON value for --param {key}"))?;
    Ok((key.to_string(), value))
}

fn json_f32(value: Option<&Value>) -> Option<f32> {
    value.and_then(Value::as_f64).map(|value| value as f32)
}

fn validate_generation(
    provider: ProviderKind,
    model: &str,
    generation: &EffectiveGeneration,
) -> Result<()> {
    if provider == ProviderKind::Kimi && model.eq_ignore_ascii_case("kimi-k2.6") {
        validate_fixed_float("temperature", generation.temperature, 0.6)?;
        validate_fixed_float("top_p", generation.params.top_p, 0.95)?;
    }
    Ok(())
}

fn validate_fixed_float(name: &str, value: Option<f32>, allowed: f32) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if (value - allowed).abs() > f32::EPSILON {
        return Err(anyhow!(
            "{name}={value} is not accepted by this model; use {name}={allowed} or omit it"
        ));
    }
    Ok(())
}

fn provider_from_record(record: &SessionRecord) -> Option<&str> {
    record.artifacts.get("provider").and_then(Value::as_str)
}

fn set_provider(record: &mut SessionRecord, provider: ProviderKind) {
    if !record.artifacts.is_object() {
        record.artifacts = json!({});
    }
    if let Some(object) = record.artifacts.as_object_mut() {
        object.insert("provider".to_string(), json!(provider));
    }
}

fn set_generation(record: &mut SessionRecord, generation: &EffectiveGeneration) {
    if !record.artifacts.is_object() {
        record.artifacts = json!({});
    }
    if let Some(object) = record.artifacts.as_object_mut() {
        object.insert("generation".to_string(), generation.record_json());
    }
}

fn bamboo_state(status: &str) -> String {
    match status {
        "success" => "completed".to_string(),
        "blocked" => "blocked".to_string(),
        "failed" => "failed".to_string(),
        other => other.to_string(),
    }
}

fn report_summary(report: &RunReport) -> Value {
    json!({
        "run_id": report.run_id,
        "status": report.status,
        "provider": report.provider,
        "model": report.model,
        "summary": report.summary,
        "pending_user_input": report.pending_user_input,
        "changed_files": report.changed_files,
        "verification": report.verification,
        "usage": report.usage,
        "cache": report.cache,
        "estimated_cost": report.estimated_cost,
        "budget": report.budget,
        "context_compaction": report.context_compaction,
        "duration_ms": report.duration_ms
    })
}

#[derive(Debug, serde::Serialize)]
struct RunListItem {
    run_id: String,
    path: String,
    status: String,
    provider: Option<String>,
    model: Option<String>,
    created_ms: u64,
    workspace: Option<String>,
    report: Option<String>,
    events: Option<String>,
    resume_context: Option<String>,
    has_resume_context: bool,
    model_compactions: u64,
    runtime_compactions: u64,
}

#[derive(Debug, serde::Serialize)]
struct RunShowReport {
    run: RunListItem,
    summary: Option<String>,
    pending_user_input: Value,
    changed_files: Value,
    todos: Value,
    verification: Value,
    usage: Value,
    cache: Value,
    estimated_cost: Value,
    context_compaction: Value,
    final_audit: Value,
    artifacts: Value,
    metadata: Value,
}

fn list_runs(run_store: &Path, limit: usize) -> Result<Vec<RunListItem>> {
    if !run_store.is_dir() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in fs::read_dir(run_store)
        .with_context(|| format!("failed to read {}", run_store.display()))?
    {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("run-") {
            continue;
        }
        if let Ok(item) = load_run_item(&path) {
            runs.push(item);
        }
    }
    runs.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| b.run_id.cmp(&a.run_id))
    });
    runs.truncate(limit);
    Ok(runs)
}

fn load_run_show(run_store: &Path, target: &str) -> Result<RunShowReport> {
    let dir = resume_dir(target, run_store);
    let run = load_run_item(&dir)?;
    let metadata = read_json_file_if_exists(&dir.join("metadata.json"))?.unwrap_or_default();
    let report = read_json_file(&dir.join("report.json"))?;
    let artifacts = json!({
        "run_dir": dir,
        "report": run.report,
        "events": run.events,
        "resume_context": run.resume_context,
    });
    Ok(RunShowReport {
        summary: json_str(&report, "/summary").map(ToString::to_string),
        pending_user_input: report
            .pointer("/pending_user_input")
            .cloned()
            .unwrap_or(Value::Null),
        changed_files: report
            .pointer("/changed_files")
            .cloned()
            .unwrap_or_else(|| json!([])),
        todos: report
            .pointer("/todos")
            .cloned()
            .unwrap_or_else(|| json!([])),
        verification: report
            .pointer("/verification")
            .cloned()
            .unwrap_or_else(|| json!([])),
        usage: report.pointer("/usage").cloned().unwrap_or_default(),
        cache: report.pointer("/cache").cloned().unwrap_or_default(),
        estimated_cost: report
            .pointer("/estimated_cost")
            .cloned()
            .unwrap_or_default(),
        context_compaction: report
            .pointer("/context_compaction")
            .cloned()
            .unwrap_or_default(),
        final_audit: report.pointer("/final_audit").cloned().unwrap_or_default(),
        run,
        artifacts,
        metadata,
    })
}

fn load_run_item(run_dir: &Path) -> Result<RunListItem> {
    let metadata = read_json_file_if_exists(&run_dir.join("metadata.json"))?.unwrap_or_default();
    let report = read_json_file_if_exists(&run_dir.join("report.json"))?.unwrap_or_default();
    if metadata.is_null() && report.is_null() {
        return Err(anyhow!(
            "run artifact missing metadata.json and report.json: {}",
            run_dir.display()
        ));
    }
    let run_id = json_str(&metadata, "/run_id")
        .or_else(|| json_str(&report, "/run_id"))
        .or_else(|| run_dir.file_name().and_then(|name| name.to_str()))
        .unwrap_or("unknown")
        .to_string();
    let status = json_str(&metadata, "/status")
        .or_else(|| json_str(&report, "/status"))
        .unwrap_or("unknown")
        .to_string();
    let report_path = existing_path_string(run_dir.join("report.json"));
    let events_path = json_str(&metadata, "/events")
        .map(ToString::to_string)
        .or_else(|| existing_path_string(run_dir.join("events.jsonl")));
    let resume_context = json_str(&metadata, "/resume_context")
        .map(ToString::to_string)
        .or_else(|| existing_path_string(run_dir.join("resume-context.md")));
    Ok(RunListItem {
        run_id,
        path: run_dir.display().to_string(),
        status,
        provider: json_str(&report, "/provider")
            .or_else(|| json_str(&metadata, "/config/provider"))
            .map(ToString::to_string),
        model: json_str(&report, "/model")
            .or_else(|| json_str(&metadata, "/config/model"))
            .map(ToString::to_string),
        created_ms: json_u64_at(&metadata, "created_ms")
            .or_else(|| file_modified_ms(&run_dir.join("report.json")))
            .unwrap_or(0),
        workspace: json_str(&metadata, "/workspace")
            .or_else(|| json_str(&report, "/stable_context/cwd"))
            .map(ToString::to_string),
        report: report_path,
        events: events_path,
        has_resume_context: resume_context
            .as_ref()
            .map(|path| Path::new(path).is_file())
            .unwrap_or(false),
        resume_context,
        model_compactions: report
            .pointer("/context_compaction/model_compactions")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        runtime_compactions: report
            .pointer("/context_compaction/runtime_compactions")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn write_run_artifacts(
    run_dir: &Path,
    event_log: &Path,
    report: &RunReport,
    config: &Value,
) -> Result<()> {
    fs::create_dir_all(run_dir)
        .with_context(|| format!("failed to create {}", run_dir.display()))?;
    let report_path = run_dir.join("report.json");
    fs::write(&report_path, serde_json::to_string_pretty(report)?)
        .with_context(|| format!("failed to write {}", report_path.display()))?;
    let report_value = serde_json::to_value(report)?;
    let events_tail = fs::read_to_string(event_log)
        .ok()
        .map(|events| compact_event_tail(&events, 80))
        .unwrap_or_else(|| "[no event log found]".to_string());
    let resume_context = build_resume_context(run_dir, &report_value, &events_tail);
    let resume_path = run_dir.join("resume-context.md");
    fs::write(&resume_path, resume_context)
        .with_context(|| format!("failed to write {}", resume_path.display()))?;
    let latest_path = run_dir
        .parent()
        .ok_or_else(|| anyhow!("run directory has no parent: {}", run_dir.display()))?
        .join("latest");
    fs::write(&latest_path, &report.run_id)
        .with_context(|| format!("failed to write {}", latest_path.display()))?;
    let metadata = json!({
        "run_id": report.run_id,
        "status": report.status,
        "created_ms": crate::io::now_millis(),
        "workspace": report.stable_context.cwd,
        "report": report_path.display().to_string(),
        "events": event_log.display().to_string(),
        "resume_context": resume_path.display().to_string(),
        "config": config,
    });
    fs::write(
        run_dir.join("metadata.json"),
        serde_json::to_string_pretty(&metadata)?,
    )?;
    Ok(())
}

fn resume_run_id(value: &str) -> Option<String> {
    let path = Path::new(value);
    if path.components().count() > 1 {
        return path.file_name()?.to_str().map(str::to_string);
    }
    Some(value.to_string())
}

fn resume_dir(value: &str, run_store: &Path) -> PathBuf {
    if value == "latest"
        && let Ok(run_id) = fs::read_to_string(run_store.join("latest"))
    {
        return run_store.join(run_id.trim());
    }
    let path = PathBuf::from(value);
    if path.is_file()
        && let Ok(run_id) = fs::read_to_string(&path)
        && let Some(parent) = path.parent()
    {
        return parent.join(run_id.trim());
    }
    if path.is_dir() || path.join("report.json").is_file() {
        path
    } else {
        run_store.join(value)
    }
}

fn resume_context(value: Option<&str>, run_store: &Path) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let dir = resume_dir(value, run_store);
    let resume_path = dir.join("resume-context.md");
    if resume_path.is_file() {
        return fs::read_to_string(&resume_path)
            .map(Some)
            .with_context(|| format!("failed to read resume context {}", resume_path.display()));
    }
    let report_path = dir.join("report.json");
    let report = fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read resume report {}", report_path.display()))?;
    let report_json = serde_json::from_str(&report)
        .with_context(|| format!("failed to parse resume report {}", report_path.display()))?;
    let events_tail = fs::read_to_string(dir.join("events.jsonl"))
        .ok()
        .map(|events| compact_event_tail(&events, 40))
        .unwrap_or_else(|| "[no previous event log found]".to_string());
    Ok(Some(build_resume_context(&dir, &report_json, &events_tail)))
}

fn compose_run_task(task: String, resume_context: Option<&str>) -> String {
    match resume_context {
        Some(context) => format!(
            "Resume the previous Bamboo run. Use the previous report and event tail as context, inspect the current workspace state, continue the task, avoid repeating completed work, and verify before finish.\n\n<<<BAMBOO_RESUME_CONTEXT>>>\n{}\n<<<BAMBOO_RESUME_CONTEXT_END>>>\n\nNew user instruction:\n{}",
            context,
            if task.trim().is_empty() {
                "Continue from the previous run."
            } else {
                task.trim()
            }
        ),
        None => task,
    }
}

fn build_resume_context(run_dir: &Path, report: &Value, compact_event_tail: &str) -> String {
    let mut output = String::new();
    output.push_str("# Bamboo Resume Context\n\n");
    output.push_str(&format!("Previous run directory: {}\n", run_dir.display()));
    output.push_str(&format!(
        "Previous status: {}\n",
        json_str(report, "/status").unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "Previous provider/model: {}/{}\n",
        json_str(report, "/provider").unwrap_or("unknown"),
        json_str(report, "/model").unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "Previous workspace: {}\n\n",
        json_str(report, "/stable_context/cwd").unwrap_or("unknown")
    ));
    output.push_str("## Compact Working Memory\n\n");
    output.push_str(
        latest_compact_summary(report)
            .unwrap_or("[No model compact summary was recorded in the previous run.]"),
    );
    output.push_str("\n\n## Previous Final State\n\n");
    if let Some(summary) = json_str(report, "/summary").filter(|summary| !summary.trim().is_empty())
    {
        output.push_str("summary:\n");
        output.push_str(summary.trim());
        output.push_str("\n\n");
    }
    append_string_array(
        &mut output,
        "changed_files",
        report.pointer("/changed_files"),
    );
    append_verification(&mut output, report.pointer("/verification"));
    append_usage_summary(&mut output, report);
    output.push_str("\n## Recent Event Tail\n\n");
    output.push_str(compact_event_tail);
    output.push('\n');
    output
}

fn latest_compact_summary(report: &Value) -> Option<&str> {
    report
        .pointer("/context_compaction/summaries")?
        .as_array()?
        .last()?
        .get("summary")?
        .as_str()
        .filter(|summary| !summary.trim().is_empty())
}

fn append_string_array(output: &mut String, label: &str, value: Option<&Value>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    output.push_str(label);
    output.push_str(":\n");
    for item in items.iter().filter_map(Value::as_str) {
        output.push_str("- ");
        output.push_str(item);
        output.push('\n');
    }
    output.push('\n');
}

fn append_verification(output: &mut String, value: Option<&Value>) {
    let Some(records) = value.and_then(Value::as_array) else {
        return;
    };
    if records.is_empty() {
        return;
    }
    output.push_str("verification:\n");
    for record in records {
        let status = if record
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "ok"
        } else {
            "failed"
        };
        output.push_str(&format!(
            "- {}: {}\n",
            status,
            json_str_at(record, "command").unwrap_or("")
        ));
    }
    output.push('\n');
}

fn append_usage_summary(output: &mut String, report: &Value) {
    let Some(usage) = report.pointer("/usage") else {
        return;
    };
    output.push_str(&format!(
        "usage: calls={} input_tokens={} output_tokens={} total_tokens={} cache_hit_tokens={} cache_miss_tokens={}\n",
        json_u64_at(usage, "calls").unwrap_or(0),
        json_u64_at(usage, "input_tokens").unwrap_or(0),
        json_u64_at(usage, "output_tokens").unwrap_or(0),
        json_u64_at(usage, "total_tokens").unwrap_or(0),
        json_u64_at(usage, "cache_hit_tokens").unwrap_or(0),
        json_u64_at(usage, "cache_miss_tokens").unwrap_or(0),
    ));
}

fn compact_event_tail(events: &str, max_events: usize) -> String {
    let mut summaries = Vec::new();
    for line in events.lines().rev().take(max_events * 4) {
        if summaries.len() >= max_events {
            break;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        summaries.push(format_event_for_resume(&event));
    }
    summaries.reverse();
    if summaries.is_empty() {
        "[no parseable previous events found]".to_string()
    } else {
        summaries.join("\n")
    }
}

fn format_event_for_resume(event: &Value) -> String {
    let event_type = json_str(event, "/type").unwrap_or("unknown");
    let data = event.get("data").unwrap_or(&Value::Null);
    match event_type {
        "model.completed" => format!(
            "- model.completed step={} input={} output={} total={} cache_hit={} cache_miss={}",
            json_u64_at(data, "step").unwrap_or(0),
            data.pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            data.pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            data.pointer("/usage/total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            data.pointer("/usage/cache_hit_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            data.pointer("/usage/cache_miss_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        "tool.finished" => {
            let result = data.get("result").unwrap_or(&Value::Null);
            format!(
                "- tool.finished step={} action={} ok={} path={} command={} exit_code={} message={}",
                json_u64_at(data, "step").unwrap_or(0),
                json_str_at(result, "action").unwrap_or("unknown"),
                result
                    .get("ok")
                    .and_then(Value::as_bool)
                    .map(|ok| ok.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                json_str_at(result, "path").unwrap_or(""),
                json_str_at(result, "command").unwrap_or(""),
                result
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .map(|code| code.to_string())
                    .unwrap_or_default(),
                truncate_for_resume(json_str_at(result, "message").unwrap_or(""), 240)
            )
        }
        "run.finished" => format!(
            "- run.finished status={} changed_files={}",
            json_str_at(data, "status").unwrap_or("unknown"),
            data.get("changed_files")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0),
        ),
        other => format!(
            "- {} step={} duration_ms={}",
            other,
            json_u64_at(data, "step").unwrap_or(0),
            json_u64_at(data, "duration_ms").unwrap_or(0)
        ),
    }
}

fn read_json_file(path: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_json_file_if_exists(path: &Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    read_json_file(path).map(Some)
}

fn existing_path_string(path: PathBuf) -> Option<String> {
    path.is_file().then(|| path.display().to_string())
}

fn file_modified_ms(path: &Path) -> Option<u64> {
    let modified = path.metadata().ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    u64::try_from(millis).ok()
}

fn default_run_id() -> String {
    format!("run-{}-{}", std::process::id(), crate::io::now_millis())
}

fn json_str<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer)?.as_str()
}

fn json_str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn json_u64_at(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}

fn truncate_for_resume(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output.replace('\n', " ");
        }
        output.push(ch);
    }
    output.replace('\n', " ")
}
