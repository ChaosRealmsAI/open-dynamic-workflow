use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;

mod pack;

const ODW_VERSION: &str = env!("CARGO_PKG_VERSION");
const ODW_PROJECT_BIN_PATH: &str = ".odw/bin/odw";
#[derive(Debug, Parser)]
#[command(
    name = "odw",
    version,
    about = "Open Dynamic Workflow: run agent-authored JavaScript workflows; each agent() node is dispatched to PandaCode (codex/claude/bamboo). Start with `odw exec --script <wf.js> --backend mock`.",
    long_about = "Open Dynamic Workflow (odw) runs a JavaScript workflow you write \
(agent / parallel / pipeline / budget / worktree) and dispatches every executor \
node to PandaCode. The CLI is the single entrypoint:\n\n  \
odw init --path .                 # scaffold the pack + agent skill\n  \
odw doctor                       # check pandacode + pack health\n  \
odw exec --script wf.js --backend mock --json    # token-free dry run\n  \
odw exec --script wf.js --backend pandacode      # real run\n\n\
After `odw init`, read .claude/skills/odw/SKILL.md for the full authoring guide."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Install Open Dynamic Workflow into a project .claude directory")]
    Init {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(
            long,
            help = "Overwrite existing generated Open Dynamic Workflow files"
        )]
        force: bool,
        #[arg(long, help = "Print planned writes without changing files")]
        dry_run: bool,
    },
    #[command(about = "Check the pandacode executor, runtimes, and pack health")]
    Doctor {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "claude")]
        claude_bin: String,
        #[arg(long, default_value = "codexctl")]
        codexctl_bin: String,
        #[arg(long, env = "ODW_PANDACODE_BIN", default_value = "pandacode")]
        pandacode_bin: String,
        #[arg(long, help = "Print the full machine-readable doctor report")]
        json: bool,
    },
    #[command(subcommand, about = "List installed or built-in agent types")]
    Agents(AgentsCommand),
    #[command(subcommand, about = "List or remove ODW workflow commands/templates")]
    Workflows(WorkflowsCommand),
    #[command(subcommand, about = "Inspect workflow graph entrypoints")]
    Graphs(GraphsCommand),
    #[command(about = "Validate a project's Open Dynamic Workflow files")]
    Validate {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    #[command(about = "Print the workflow authoring contract for this pack")]
    Contract,
    #[command(about = "Print machine-readable Open Dynamic Workflow capability mapping")]
    Capabilities,
    #[command(about = "Print the Open Dynamic Workflow framework spec")]
    Spec,
    #[command(about = "List Claude Code Dynamic Workflow evidence for this project")]
    Evidence(EvidenceArgs),
    #[command(subcommand, about = "Inspect ODW run journals and live logs")]
    Runs(RunsCommand),
    #[command(about = "Execute an ODW JavaScript workflow script directly")]
    Exec(Box<ExecArgs>),
    #[command(
        about = "Render an HTML execution-graph report (Mermaid) from a workflow",
        long_about = "Mock-run a workflow script (or take an existing run) and render a self-contained HTML report: a Mermaid execution graph coloured by runtime, plus each node's model, prompt, status, tokens, and duration.\n\n  odw report --script wf.js --open       # write JS -> mock dry-run -> graph -> open\n  odw report --run latest --open         # graph an existing (real or mock) run"
    )]
    Report(ReportArgs),
}

#[derive(Debug, clap::Args)]
struct EvidenceArgs {
    #[arg(long, default_value = ".")]
    path: PathBuf,
    #[arg(
        long,
        help = "Override Claude projects directory; defaults to ~/.claude/projects"
    )]
    claude_projects_dir: Option<PathBuf>,
    #[arg(long, help = "Filter to one Claude Code session directory")]
    session_id: Option<String>,
    #[arg(long, help = "Filter to one workflow runId")]
    run_id: Option<String>,
}

#[derive(Debug, clap::Args)]
struct ExecArgs {
    #[arg(long, default_value = ".")]
    path: PathBuf,
    #[arg(long)]
    script: Option<PathBuf>,
    #[arg(long, conflicts_with = "input_file")]
    input: Option<String>,
    #[arg(long)]
    input_file: Option<PathBuf>,
    #[arg(long, help = "Resume a previous direct workflow run id, or latest")]
    resume: Option<String>,
    #[arg(long, default_value = "mock", value_parser = ["mock", "pandacode"])]
    backend: String,
    #[arg(long, default_value = "node")]
    node_bin: String,
    #[arg(long, env = "ODW_PROVIDER")]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value = "low")]
    effort: String,
    #[arg(long, default_value = "120")]
    timeout: String,
    #[arg(long, env = "ODW_CODEXCTL_BIN", default_value = "codexctl")]
    codexctl_bin: String,
    #[arg(long, env = "ODW_PANDACODE_BIN", default_value = "pandacode")]
    pandacode_bin: String,
    #[arg(long, help = "Print only the final workflow result as one JSON object")]
    json: bool,
    #[arg(long, help = "Open the auto-generated HTML execution-graph report")]
    open: bool,
    #[arg(long, help = "Skip the auto-generated HTML execution-graph report")]
    no_report: bool,
    #[arg(long, help = "Print the Node command instead of executing it")]
    dry_run: bool,
}

#[derive(Debug, Subcommand)]
enum RunsCommand {
    List {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    Show {
        #[arg(default_value = "latest")]
        run_id: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 80)]
        tail: usize,
    },
}

#[derive(Debug, Subcommand)]
enum AgentsCommand {
    List {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, help = "Show built-in agent types instead of installed files")]
        built_in: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorkflowsCommand {
    List {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    Remove {
        name: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, help = "Print files that would be removed")]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum GraphsCommand {
    List {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init {
            path,
            force,
            dry_run,
        } => init(&path, force, dry_run),
        Commands::Doctor {
            path,
            claude_bin,
            codexctl_bin,
            pandacode_bin,
            json,
        } => doctor(&path, &claude_bin, &codexctl_bin, &pandacode_bin, json),
        Commands::Agents(command) => match command {
            AgentsCommand::List { path, built_in } => list_agents(&path, built_in),
        },
        Commands::Workflows(command) => match command {
            WorkflowsCommand::List { path } => list_workflows(&path),
            WorkflowsCommand::Remove {
                name,
                path,
                dry_run,
            } => remove_workflow(&path, &name, dry_run),
        },
        Commands::Graphs(command) => match command {
            GraphsCommand::List { path } => graphs_list(&path),
        },
        Commands::Validate { path } => validate(&path),
        Commands::Contract => {
            println!("{}", contract_text());
            Ok(())
        }
        Commands::Capabilities => {
            println!("{}", serde_json::to_string_pretty(&capabilities_json())?);
            Ok(())
        }
        Commands::Spec => {
            println!("{}", serde_json::to_string_pretty(&framework_spec_json())?);
            Ok(())
        }
        Commands::Evidence(args) => evidence(args),
        Commands::Runs(command) => match command {
            RunsCommand::List { path } => runs_list(&path),
            RunsCommand::Show { run_id, path, tail } => runs_show(&path, &run_id, tail),
        },
        Commands::Exec(args) => exec_script(*args),
        Commands::Report(args) => report(args),
    }
}

fn init(root: &Path, force: bool, dry_run: bool) -> Result<()> {
    let actions = init_actions(root, force, dry_run)?;
    for action in actions {
        println!("{action}");
    }
    if !dry_run {
        println!(
            "\nOpen Dynamic Workflow installed. Run `odw exec --script <workflow.js> --input <json>`; Claude Code `/odw` remains available as an optional entrypoint."
        );
    }
    Ok(())
}

fn init_actions(root: &Path, force: bool, dry_run: bool) -> Result<Vec<String>> {
    let root = normalize_root(root)?;
    let files = pack_files();
    for file in files {
        let target = root.join(file.path);
        if target.exists() && !force {
            bail!(
                "{} already exists; rerun with --force to overwrite generated Open Dynamic Workflow files",
                target.display()
            );
        }
    }
    let shim_target = root.join(ODW_PROJECT_BIN_PATH);
    if shim_target.exists() && !force {
        bail!(
            "{} already exists; rerun with --force to overwrite generated Open Dynamic Workflow files",
            shim_target.display()
        );
    }
    let mut actions = Vec::new();
    for file in files {
        let target = root.join(file.path);
        if dry_run {
            actions.push(format!("write {}", target.display()));
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&target, file.content).with_context(|| format!("write {}", target.display()))?;
        actions.push(format!("wrote {}", target.display()));
    }
    if dry_run {
        actions.push(format!("write {}", shim_target.display()));
    } else {
        if let Some(parent) = shim_target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&shim_target, project_odw_shim_content())
            .with_context(|| format!("write {}", shim_target.display()))?;
        make_executable(&shim_target)?;
        actions.push(format!("wrote {}", shim_target.display()));
    }
    Ok(actions)
}

fn project_odw_shim_content() -> String {
    let bootstrap = std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "odw".to_string());
    format!(
        r#"#!/usr/bin/env sh
set -eu

if [ "${{ODW_BIN:-}}" != "" ] && [ -x "${{ODW_BIN}}" ]; then
  exec "${{ODW_BIN}}" "$@"
fi

bootstrap={}
if [ -x "$bootstrap" ]; then
  exec "$bootstrap" "$@"
fi

candidate="$(command -v odw 2>/dev/null || true)"
if [ "$candidate" != "" ]; then
  self_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
  candidate_dir="$(CDPATH= cd -- "$(dirname -- "$candidate")" 2>/dev/null && pwd || true)"
  candidate_name="$(basename -- "$candidate")"
  if [ "$candidate_dir/$candidate_name" != "$self_dir/odw" ]; then
    exec "$candidate" "$@"
  fi
fi

echo "ODW project wrapper could not find an executable odw binary. Set ODW_BIN or install odw on PATH." >&2
exit 127
"#,
        shell_quote(&bootstrap)
    )
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("read permissions {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("set executable bit {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn doctor(
    root: &Path,
    claude_bin: &str,
    codexctl_bin: &str,
    pandacode_bin: &str,
    json_output: bool,
) -> Result<()> {
    let pandacode_bin = &resolved_pandacode_bin(pandacode_bin);
    let report = doctor_report(root, claude_bin, codexctl_bin, pandacode_bin)?;
    let ok = report
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", render_doctor_human(&report));
    }
    if !ok {
        bail!("odw doctor checks failed; see report above");
    }
    Ok(())
}

fn doctor_report(
    root: &Path,
    claude_bin: &str,
    codexctl_bin: &str,
    pandacode_bin: &str,
) -> Result<serde_json::Value> {
    let root = normalize_root(root)?;
    // `pandacode` is the one executor odw actually requires. claude/codexctl are
    // PandaCode's concern (it owns the runtimes + their mechanics), so they are
    // reported for information but do not gate odw's own health.
    // odw's script runtime runs on node; without it no workflow can execute, so
    // it gates health alongside pandacode.
    let node = run_version("node", &["--version"]);
    let pandacode = run_version(pandacode_bin, &["--version"]);
    let claude = run_version(claude_bin, &["--version"]);
    let codexctl = run_version(codexctl_bin, &["--help"]);
    let codex = run_codex_status(codexctl_bin);
    let bamboo_keys = bamboo_key_report();
    let runtimes = run_pandacode_doctor_report(pandacode_bin, &root);
    let project_odw = run_project_odw_status(&root);
    let pack = validate_pack_status(&root);
    Ok(json!({
        "ok": node.ok && pandacode.ok && project_odw.ok && pack.ok,
        "odw_version": ODW_VERSION,
        "project": root,
        "node": node,
        "pandacode": pandacode,
        "runtimes": runtimes,
        "claude": claude,
        "codexctl": codexctl,
        "codex": codex,
        "bamboo_keys": bamboo_keys,
        "project_odw": project_odw,
        "pack": pack,
        "decision": "odw dispatches each node to `pandacode <runtime> exec`; it requires only the pandacode binary. PandaCode owns the codex/claude/bamboo runtimes (and the codexctl/tmux mechanics)."
    }))
}

fn render_doctor_human(report: &serde_json::Value) -> String {
    let node_ok = value_ok(&report["node"]);
    let pandacode_ok = value_ok(&report["pandacode"]);
    let codex_ok = value_ok(&report["codex"]);
    let claude_bin_ok = value_ok(&report["claude"]);
    let claude_runtime_ok = value_ok(&report["runtimes"]["claude"]);
    let claude_ok = claude_bin_ok && claude_runtime_ok;
    let bamboo_ready = bamboo_ready_count(&report["bamboo_keys"]);
    let hard_ok = report.get("ok").and_then(|value| value.as_bool()) == Some(true);

    let mut ready = Vec::new();
    let mut pending = Vec::new();
    push_source(&mut ready, &mut pending, node_ok, "node");
    push_source(&mut ready, &mut pending, pandacode_ok, "pandacode");
    push_source(&mut ready, &mut pending, codex_ok, "codex");
    push_source(&mut ready, &mut pending, claude_ok, "claude");
    if bamboo_ready > 0 {
        ready.push("bamboo");
    } else {
        pending.push("bamboo");
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "odw doctor: {} hard checks; ready: {}; needs setup: {}",
        if hard_ok { "✅" } else { "❌" },
        comma_list(&ready),
        comma_list(&pending)
    ));
    lines.push(String::new());
    lines.push(format!(
        "{} node: {}",
        icon(node_ok),
        if node_ok {
            format!("available ({})", value_summary(&report["node"]))
        } else {
            "not found - install Node.js (the odw script runtime needs it)".to_string()
        }
    ));
    lines.push(format!(
        "{} pandacode: {}",
        icon(pandacode_ok),
        if pandacode_ok {
            format!("available ({})", value_summary(&report["pandacode"]))
        } else {
            format!(
                "missing or not runnable ({}) - install pandacode or set ODW_PANDACODE_BIN/--pandacode-bin",
                value_summary(&report["pandacode"])
            )
        }
    ));
    lines.push(format!(
        "{} codex: {}",
        icon(codex_ok),
        if codex_ok {
            "logged in / quota check passed".to_string()
        } else if value_ok(&report["codexctl"]) {
            format!(
                "codexctl exists, but login/quota check failed ({}) - run `codexctl status`, sign in, or refresh quota",
                value_summary(&report["codex"])
            )
        } else {
            format!(
                "codexctl not runnable ({}) - install codexctl or set --codexctl-bin",
                value_summary(&report["codexctl"])
            )
        }
    ));
    lines.push(format!(
        "{} claude (Cloud): {}",
        icon(claude_ok),
        claude_human_status(report, claude_bin_ok, claude_runtime_ok)
    ));
    lines.push(format!(
        "{} bamboo: {}",
        if bamboo_ready > 0 { "⚠️" } else { "❌" },
        bamboo_human_status(&report["bamboo_keys"])
    ));
    lines.push(String::new());
    lines.push(format!(
        "{} project odw: {}",
        icon(value_ok(&report["project_odw"])),
        project_human_status(&report["project_odw"])
    ));
    lines.push(format!(
        "{} pack: {}",
        icon(value_ok(&report["pack"])),
        pack_human_status(&report["pack"])
    ));
    lines.join("\n")
}

fn push_source<'a>(ready: &mut Vec<&'a str>, pending: &mut Vec<&'a str>, ok: bool, name: &'a str) {
    if ok {
        ready.push(name);
    } else {
        pending.push(name);
    }
}

fn icon(ok: bool) -> &'static str {
    if ok { "✅" } else { "❌" }
}

fn comma_list(items: &[&str]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn value_ok(value: &serde_json::Value) -> bool {
    value.get("ok").and_then(|field| field.as_bool()) == Some(true)
}

fn value_summary(value: &serde_json::Value) -> String {
    value
        .get("summary")
        .and_then(|field| field.as_str())
        .filter(|summary| !summary.is_empty())
        .unwrap_or("no details")
        .to_string()
}

fn claude_human_status(report: &serde_json::Value, bin_ok: bool, runtime_ok: bool) -> String {
    if !bin_ok {
        return format!(
            "Claude Code not runnable ({}) - install Claude Code or set --claude-bin",
            value_summary(&report["claude"])
        );
    }
    if runtime_ok {
        return format!(
            "Claude Code available; PandaCode runtime available ({})",
            value_summary(&report["claude"])
        );
    }
    format!(
        "Claude Code exists, but PandaCode runtime needs setup ({}) - run Claude Code login/setup and `pandacode doctor --json`",
        value_summary(&report["runtimes"]["claude"])
    )
}

fn bamboo_human_status(value: &serde_json::Value) -> String {
    let Some(map) = value.as_object() else {
        return "no provider key report - set provider API keys or PANDACODE_BAMBOO_API_KEY"
            .to_string();
    };
    BAMBOO_PROVIDERS
        .iter()
        .map(|(provider, _)| {
            let item = &map[*provider];
            let env_name = item
                .get("env")
                .and_then(|field| field.as_str())
                .unwrap_or("provider API key");
            let source = item
                .get("source")
                .and_then(|field| field.as_str())
                .unwrap_or(env_name);
            if value_ok(item) {
                format!("{provider} ✅ ({source})")
            } else {
                format!("{provider} ❌ set {env_name}")
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn bamboo_ready_count(value: &serde_json::Value) -> usize {
    let Some(map) = value.as_object() else {
        return 0;
    };
    map.values().filter(|item| value_ok(item)).count()
}

fn project_human_status(value: &serde_json::Value) -> String {
    if value_ok(value) {
        format!("installed ({})", value_summary(value))
    } else {
        format!("missing ({}) - run `odw init --path .`", value_summary(value))
    }
}

fn pack_human_status(value: &serde_json::Value) -> String {
    if value_ok(value) {
        "valid".to_string()
    } else {
        let missing: Vec<&str> = value
            .get("missing")
            .and_then(|field| field.as_array())
            .map(|items| items.iter().filter_map(|item| item.as_str()).collect())
            .unwrap_or_default();
        // A fresh checkout is missing the whole pack; dumping 50+ paths buries the
        // one thing the user needs to do. Show a short, actionable line instead.
        if missing.len() >= 10 {
            return "not installed - run `odw init --path .`".to_string();
        }
        let detail = if missing.is_empty() {
            "pack files".to_string()
        } else {
            let shown = missing.iter().take(4).copied().collect::<Vec<_>>().join(", ");
            if missing.len() > 4 {
                format!("{shown}, +{} more", missing.len() - 4)
            } else {
                shown
            }
        };
        format!("incomplete ({detail}) - run `odw init --path . --force` if intended")
    }
}

fn run_codex_status(codexctl_bin: &str) -> serde_json::Value {
    let checks: &[&[&str]] = &[&["status"], &["account"], &["quota"]];
    let mut failures = Vec::new();
    for args in checks {
        let status = run_command_status(codexctl_bin, args);
        if status.ok {
            return json!({
                "ok": true,
                "command": command_display(codexctl_bin, args),
                "summary": status.summary
            });
        }
        failures.push(format!(
            "{}: {}",
            command_display(codexctl_bin, args),
            status.summary
        ));
    }
    json!({
        "ok": false,
        "command": codexctl_bin,
        "summary": failures.join("; ")
    })
}

fn run_command_status(command: &str, args: &[&str]) -> ToolStatus {
    match Command::new(command).args(args).output() {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            ToolStatus {
                ok: output.status.success(),
                command: command_display(command, args),
                summary: text.lines().next().unwrap_or("").to_string(),
            }
        }
        Err(error) => ToolStatus {
            ok: false,
            command: command_display(command, args),
            summary: error.to_string(),
        },
    }
}

fn command_display(command: &str, args: &[&str]) -> String {
    std::iter::once(command)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

const BAMBOO_PROVIDERS: &[(&str, &str)] = &[
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("kimi", "KIMI_API_KEY"),
    ("qwen", "QWEN_API_KEY"),
    ("zhipu", "ZHIPU_API_KEY"),
    ("minimax", "MINIMAX_API_KEY"),
    ("xiaomi", "XIAOMI_API_KEY"),
    ("stepfun", "STEPFUN_API_KEY"),
];

fn bamboo_key_report() -> serde_json::Value {
    let generic_env = "PANDACODE_BAMBOO_API_KEY";
    let generic_set = env_is_set(generic_env);
    let mut providers = serde_json::Map::new();
    for (provider, env_name) in BAMBOO_PROVIDERS {
        let provider_set = env_is_set(env_name);
        let source = if provider_set {
            Some(*env_name)
        } else if generic_set {
            Some(generic_env)
        } else {
            None
        };
        providers.insert(
            (*provider).to_string(),
            json!({
                "ok": source.is_some(),
                "provider": provider,
                "env": env_name,
                "generic_env": generic_env,
                "source": source,
                "summary": match source {
                    Some(name) => format!("key present via {name}"),
                    None => format!("set {env_name} or {generic_env}"),
                }
            }),
        );
    }
    serde_json::Value::Object(providers)
}

fn env_is_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn run_project_odw_status(root: &Path) -> ToolStatus {
    let command = root.join(ODW_PROJECT_BIN_PATH);
    if !command.exists() {
        return ToolStatus {
            ok: false,
            command: command.to_string_lossy().to_string(),
            summary: "project-local ODW wrapper is missing".to_string(),
        };
    }
    let command_text = command.to_string_lossy().to_string();
    run_version(&command_text, &["--version"])
}

fn run_pandacode_doctor_report(pandacode_bin: &str, root: &Path) -> serde_json::Value {
    let command = vec![
        pandacode_bin.to_string(),
        "doctor".to_string(),
        "--cd".to_string(),
        root.to_string_lossy().to_string(),
        "--json".to_string(),
    ];
    match Command::new(pandacode_bin)
        .args(["doctor", "--cd"])
        .arg(root)
        .arg("--json")
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if let Ok(report) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
                return report;
            }
            json!({
                "ok": false,
                "command": command,
                "exit_code": output.status.code(),
                "summary": stdout.lines().next().or_else(|| stderr.lines().next()).unwrap_or("pandacode doctor did not return JSON"),
                "stdout_tail": stdout.chars().rev().take(1000).collect::<String>().chars().rev().collect::<String>(),
                "stderr_tail": stderr.chars().rev().take(1000).collect::<String>().chars().rev().collect::<String>()
            })
        }
        Err(error) => json!({
            "ok": false,
            "command": command,
            "summary": error.to_string()
        }),
    }
}

fn list_agents(root: &Path, built_in: bool) -> Result<()> {
    if built_in {
        for agent in built_in_agents() {
            println!("{}\t{}", agent.name, agent.description);
        }
        return Ok(());
    }
    let dir = normalize_root(root)?.join(".claude/agents");
    if !dir.exists() {
        println!("no installed agents at {}", dir.display());
        return Ok(());
    }
    for entry in sorted_files(&dir)? {
        let content = fs::read_to_string(&entry)?;
        let name = frontmatter_value(&content, "name").unwrap_or_else(|| {
            entry
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let description = frontmatter_value(&content, "description").unwrap_or_default();
        println!("{}\t{}", name, description);
    }
    Ok(())
}

fn list_workflows(root: &Path) -> Result<()> {
    let root = normalize_root(root)?;
    let workflow_dir = root.join(".claude/workflows");
    let command_dir = root.join(".claude/commands");
    println!("workflows:");
    if workflow_dir.exists() {
        for file in sorted_files(&workflow_dir)? {
            println!("  {}", relative_display(&root, &file));
        }
    } else {
        println!("  none");
    }
    println!("commands:");
    if command_dir.exists() {
        for file in sorted_files(&command_dir)? {
            println!("  {}", relative_display(&root, &file));
        }
    } else {
        println!("  none");
    }
    Ok(())
}

fn graphs_list(root: &Path) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&graphs_list_report(root)?)?
    );
    Ok(())
}

fn graphs_list_report(root: &Path) -> Result<serde_json::Value> {
    let root = normalize_root(root)?;
    let mut graphs = Vec::new();
    let workflow_dir = root.join(".claude/workflows");
    if workflow_dir.exists() {
        for file in sorted_files(&workflow_dir)? {
            if file.extension().and_then(|value| value.to_str()) == Some("js") {
                graphs.push(workflow_script_graph_summary(&root, &file)?);
            }
        }
    }
    let graph_dir = root.join(".odw/graphs");
    if graph_dir.exists() {
        for file in sorted_files(&graph_dir)? {
            if file.extension().and_then(|value| value.to_str()) == Some("json") {
                graphs.push(graph_manifest_summary(&root, &file)?);
            }
        }
    }
    Ok(json!({
        "root": root,
        "graphs": graphs
    }))
}

fn workflow_script_graph_summary(root: &Path, file: &Path) -> Result<serde_json::Value> {
    let content = fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    Ok(json!({
        "kind": "workflow_script",
        "path": relative_display(root, file),
        "name": extract_js_meta_string(&content, "name").unwrap_or_else(|| {
            file.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("workflow")
                .to_string()
        }),
        "has_meta": content.contains("export const meta"),
        "features": {
            "agent": content.contains("agent("),
            "fanout": content.contains("fanout("),
            "parallel": content.contains("parallel("),
            "pipeline": content.contains("pipeline("),
            "checkpoint": content.contains("checkpoint("),
            "prompt_slots": content.contains("promptSlot("),
            "schema": content.contains("schema:")
        },
        "valid": content.contains("export const meta") && content.contains("agent("),
        "issues": workflow_script_graph_issues(&content)
    }))
}

fn workflow_script_graph_issues(content: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if !content.contains("export const meta") {
        issues.push("missing export const meta".to_string());
    }
    if !content.contains("agent(") {
        issues.push("no agent(...) node calls detected".to_string());
    }
    issues
}

fn graph_manifest_summary(root: &Path, file: &Path) -> Result<serde_json::Value> {
    let content = fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    let parsed = serde_json::from_str::<serde_json::Value>(&content).ok();
    let mut issues = Vec::new();
    let mut name = file
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("graph")
        .to_string();
    if let Some(value) = &parsed {
        if let Some(parsed_name) = json_string(value, "name") {
            name = parsed_name;
        }
        if value.get("start").is_none() {
            issues.push("missing start".to_string());
        }
        if value.get("end").is_none() {
            issues.push("missing end".to_string());
        }
        if !value.get("nodes").is_some_and(|nodes| nodes.is_array()) {
            issues.push("nodes must be an array".to_string());
        }
        if !value.get("edges").is_some_and(|edges| edges.is_array()) {
            issues.push("edges must be an array".to_string());
        }
    } else {
        issues.push("invalid JSON".to_string());
    }
    Ok(json!({
        "kind": "graph_manifest",
        "path": relative_display(root, file),
        "name": name,
        "valid": issues.is_empty(),
        "issues": issues
    }))
}

fn extract_js_meta_string(content: &str, key: &str) -> Option<String> {
    for (prefix, quote) in [
        (format!("{key}: \""), '"'),
        (format!("{key}: '"), '\''),
        (format!("\"{key}\": \""), '"'),
        (format!("'{key}': '"), '\''),
    ] {
        if let Some(start) = content.find(&prefix) {
            let rest = &content[start + prefix.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn remove_workflow(root: &Path, name: &str, dry_run: bool) -> Result<()> {
    for action in remove_workflow_actions(root, name, dry_run)? {
        println!("{action}");
    }
    Ok(())
}

fn remove_workflow_actions(root: &Path, name: &str, dry_run: bool) -> Result<Vec<String>> {
    let root = normalize_root(root)?;
    let normalized = name.trim().trim_start_matches('/').trim_end_matches(".md");
    if normalized.is_empty() {
        bail!("workflow name cannot be empty");
    }
    let candidates = [
        root.join(format!(".claude/workflows/{normalized}.js")),
        root.join(format!(".claude/workflows/odw-{normalized}.js")),
        root.join(format!(".claude/workflows/{normalized}.md")),
        root.join(format!(".claude/workflows/odw-{normalized}.md")),
        root.join(format!(".claude/commands/{normalized}.md")),
        root.join(format!(".claude/commands/odw-{normalized}.md")),
    ];
    let mut found = false;
    let mut actions = Vec::new();
    for path in candidates {
        if !path.exists() {
            continue;
        }
        found = true;
        if dry_run {
            actions.push(format!("remove {}", path.display()));
        } else {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            actions.push(format!("removed {}", path.display()));
        }
    }
    if !found {
        bail!("no workflow or command named {normalized:?} found under .claude");
    }
    Ok(actions)
}

fn validate(root: &Path) -> Result<()> {
    let root = normalize_root(root)?;
    let status = validate_pack_status(&root);
    if !status.ok {
        println!("{}", serde_json::to_string_pretty(&status)?);
        bail!("Open Dynamic Workflow validation failed");
    }
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

fn evidence(args: EvidenceArgs) -> Result<()> {
    let report = workflow_evidence_report(
        &args.path,
        args.claude_projects_dir.as_deref(),
        args.session_id.as_deref(),
        args.run_id.as_deref(),
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn runs_list(root: &Path) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&runs_list_report(root)?)?
    );
    Ok(())
}

fn runs_list_report(root: &Path) -> Result<serde_json::Value> {
    let root = normalize_root(root)?;
    let runs_dir = root.join(".odw/runs");
    let mut runs = Vec::new();
    if runs_dir.exists() {
        for dir in sorted_dirs(&runs_dir)? {
            let record = dir.join("run.json");
            if let Ok(content) = fs::read_to_string(&record)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
            {
                runs.push(value);
            }
        }
    }
    Ok(json!({
        "runs_dir": runs_dir,
        "runs": runs
    }))
}

fn runs_show(root: &Path, run_id: &str, tail: usize) -> Result<()> {
    let report = runs_show_report(root, run_id, tail)?;
    println!("{}", format_runs_show_view(&report));
    Ok(())
}

fn runs_show_report(root: &Path, run_id: &str, tail: usize) -> Result<serde_json::Value> {
    let root = normalize_root(root)?;
    let run_dir = resolve_run_dir(&root, run_id)?;
    let record = fs::read_to_string(run_dir.join("run.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .unwrap_or_else(|| json!({ "run_id": run_id }));
    let events = read_tail_lines(&run_dir.join("events.jsonl"), tail)?;
    let progress = run_progress_report(&run_dir);
    Ok(json!({
        "run": record,
        "events_path": run_dir.join("events.jsonl"),
        "progress": progress,
        "events": events
    }))
}

fn run_progress_report(run_dir: &Path) -> serde_json::Value {
    let state_path = run_dir.join("state.json");
    let Some(state) = fs::read_to_string(&state_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
    else {
        return json!({
            "state_path": state_path,
            "active_agents": [],
            "completed_agents": 0,
            "failed_agents": 0,
            "checkpoints": 0
        });
    };
    let mut active_agents = state
        .get("activeAgents")
        .and_then(|value| value.as_object())
        .map(|agents| agents.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    sort_agent_details(&mut active_agents);
    let mut completed_agent_details = state
        .get("agents")
        .and_then(|value| value.as_object())
        .map(|agents| {
            agents
                .iter()
                .map(|(key, value)| {
                    let mut item = value.clone();
                    if let Some(object) = item.as_object_mut() {
                        object.insert("key".to_string(), json!(key));
                    }
                    item
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    sort_agent_details(&mut completed_agent_details);
    let mut failed_agent_details = state
        .get("failedAgents")
        .and_then(|value| value.as_object())
        .map(|agents| {
            agents
                .iter()
                .map(|(key, value)| {
                    let mut item = value.clone();
                    if let Some(object) = item.as_object_mut() {
                        object.insert("key".to_string(), json!(key));
                    }
                    item
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    sort_agent_details(&mut failed_agent_details);
    let checkpoint_details = state
        .get("checkpoints")
        .and_then(|value| value.as_object())
        .map(|checkpoints| checkpoints.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let completed_agents = state
        .get("agents")
        .and_then(|value| value.as_object())
        .map(|agents| agents.len())
        .unwrap_or(0);
    let failed_agents = state
        .get("failedAgents")
        .and_then(|value| value.as_object())
        .map(|agents| agents.len())
        .unwrap_or(0);
    let checkpoints = state
        .get("checkpoints")
        .and_then(|value| value.as_object())
        .map(|checkpoints| checkpoints.len())
        .unwrap_or(0);
    json!({
        "state_path": state_path,
        "active_agents": active_agents,
        "completed_agent_details": completed_agent_details,
        "completed_agents": completed_agents,
        "failed_agent_details": failed_agent_details,
        "failed_agents": failed_agents,
        "failed_attempts": failed_agents,
        "checkpoint_details": checkpoint_details,
        "checkpoints": checkpoints,
        "workflow": state.get("workflow").cloned().unwrap_or(serde_json::Value::Null),
        "result": state.get("result").cloned().unwrap_or(serde_json::Value::Null)
    })
}

fn sort_agent_details(agents: &mut [serde_json::Value]) {
    agents.sort_by(|left, right| {
        let left_index = left
            .get("index")
            .and_then(|value| value.as_u64())
            .unwrap_or(u64::MAX);
        let right_index = right
            .get("index")
            .and_then(|value| value.as_u64())
            .unwrap_or(u64::MAX);
        let left_ts = json_string(left, "ts").unwrap_or_else(|| "~".to_string());
        let right_ts = json_string(right, "ts").unwrap_or_else(|| "~".to_string());
        left_index
            .cmp(&right_index)
            .then_with(|| left_ts.cmp(&right_ts))
            .then_with(|| json_string(left, "key").cmp(&json_string(right, "key")))
    });
}

fn format_runs_show_view(report: &serde_json::Value) -> String {
    let run = &report["run"];
    let progress = &report["progress"];
    let run_id = json_string(run, "run_id").unwrap_or_else(|| "unknown".to_string());
    let status = json_string(run, "status").unwrap_or_else(|| "unknown".to_string());
    let workflow = json_string(run, "workflow").unwrap_or_else(|| "-".to_string());
    let started = run.get("started_ms").and_then(|value| value.as_u64());
    let finished = run.get("finished_ms").and_then(|value| value.as_u64());
    let duration = match (started, finished) {
        (Some(started), Some(finished)) if finished >= started => {
            format!("{}ms", finished - started)
        }
        _ => "running".to_string(),
    };
    let active = progress
        .get("active_agents")
        .and_then(|value| value.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    let completed = progress
        .get("completed_agents")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let failed_attempts = progress
        .get("failed_attempts")
        .or_else(|| progress.get("failed_agents"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let checkpoints = progress
        .get("checkpoints")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    let mut lines = vec![
        format!("Run {run_id} [{status}]"),
        format!("Workflow: {workflow}"),
        format!("Duration: {duration}"),
        format!(
            "Nodes: active={active} completed={completed} failed_attempts={failed_attempts} checkpoints={checkpoints}"
        ),
        format!("Resume: odw exec --resume {run_id}"),
    ];

    let events = report
        .get("events")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    // Surface the failure cause in the header instead of burying it in the event
    // tail: prefer the last workflow_error event, fall back to a persisted error.
    if status == "failed" || status == "error" {
        let cause = events
            .iter()
            // Journal events are wrapped as { raw: <event>, stream, summary }.
            .map(|event| event.get("raw").unwrap_or(event))
            .rev()
            .find(|event| json_string(event, "type").as_deref() == Some("workflow_error"))
            .and_then(|event| json_string(event, "message"))
            .or_else(|| json_string(&run["error"], "message"))
            .or_else(|| json_string(run, "error"));
        if let Some(cause) = cause {
            let first_line = cause.lines().next().unwrap_or(&cause).trim().to_string();
            if !first_line.is_empty() {
                lines.insert(1, format!("Failure: {first_line}"));
            }
        }
    }

    let active_agents = progress
        .get("active_agents")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if !active_agents.is_empty() {
        lines.push("".to_string());
        lines.push("Active nodes:".to_string());
        for agent in active_agents {
            lines.push(format!("  - {}", format_agent_progress(&agent)));
            if let Some(key) = json_string(&agent, "key")
                && let Some(message) = latest_agent_message_for_key(&events, &key)
            {
                lines.push(format!("    last: {}", truncate(&message, 220)));
            }
        }
    }

    let failed_agents = progress
        .get("failed_agent_details")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if !failed_agents.is_empty() {
        lines.push("".to_string());
        lines.push("Failed attempts:".to_string());
        for agent in failed_agents {
            lines.push(format!("  - {}", format_agent_progress(&agent)));
        }
    }

    let completed_agents = progress
        .get("completed_agent_details")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if !completed_agents.is_empty() {
        lines.push("".to_string());
        lines.push("Completed nodes:".to_string());
        for agent in completed_agents.iter().rev().take(12).rev() {
            lines.push(format!("  - {}", format_agent_progress(agent)));
        }
    }

    if !events.is_empty() {
        lines.push("".to_string());
        lines.push("Recent events:".to_string());
        for event in events.iter().rev().take(16).rev() {
            let summary = json_string(event, "summary")
                .or_else(|| json_string(event, "type"))
                .unwrap_or_else(|| truncate(&event.to_string(), 160));
            lines.push(format!("  - {summary}"));
        }
    }

    lines.join("\n")
}

fn latest_agent_message_for_key(events: &[serde_json::Value], key: &str) -> Option<String> {
    for event in events.iter().rev() {
        let raw = event.get("raw").unwrap_or(event);
        if json_string(raw, "key").as_deref() != Some(key) {
            continue;
        }
        if let Some(message) = json_string(raw, "last_agent_message") {
            return Some(message);
        }
        if let Some(message) = raw
            .get("result")
            .and_then(|value| json_string(value, "last_agent_message"))
        {
            return Some(message);
        }
        if let Some(message) = raw
            .get("result")
            .and_then(|value| value.get("codex"))
            .and_then(|value| json_string(value, "last_agent_message"))
        {
            return Some(message);
        }
    }
    None
}

fn format_agent_progress(agent: &serde_json::Value) -> String {
    let key = json_string(agent, "key").unwrap_or_else(|| "-".to_string());
    let phase = json_string(agent, "phase").unwrap_or_else(|| "-".to_string());
    let label = json_string(agent, "label").unwrap_or_else(|| key.clone());
    let agent_type = json_string(agent, "agentType").unwrap_or_else(|| "-".to_string());
    let attempt = agent
        .get("attempt")
        .and_then(|value| value.as_u64())
        .unwrap_or(1);
    let max_attempts = agent
        .get("maxAttempts")
        .and_then(|value| value.as_u64())
        .unwrap_or(1);
    let state = json_string(agent, "state").unwrap_or_else(|| {
        if agent.get("ok").and_then(|value| value.as_bool()) == Some(false) {
            "failed".to_string()
        } else {
            "done".to_string()
        }
    });
    format!(
        "{phase} / {label} / {agent_type} key={key} state={state} attempt={attempt}/{max_attempts}"
    )
}

fn exec_script(args: ExecArgs) -> Result<()> {
    let root = normalize_root(&args.path)?;
    let resume_run_dir = args
        .resume
        .as_deref()
        .map(|run_id| resolve_run_dir(&root, run_id))
        .transpose()?;
    let resume_record = resume_run_dir
        .as_ref()
        .and_then(|run_dir| fs::read_to_string(run_dir.join("run.json")).ok())
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());

    let script = match (&args.script, &resume_record) {
        (Some(script), _) if script.is_absolute() => script.clone(),
        // A relative --script resolves against --path (so project-internal
        // workflows like .claude/workflows/x.js work), but fall back to the
        // current directory when it is not found there — that is where a user who
        // typed the path expects it, especially when --path points elsewhere.
        (Some(script), _) => {
            let in_root = root.join(script);
            if in_root.exists() {
                in_root
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(script))
                    .ok()
                    .filter(|candidate| candidate.exists())
                    .unwrap_or(in_root)
            }
        }
        (None, Some(record)) => json_string(record, "workflow")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("resumed run does not record a workflow script"))?,
        (None, None) => {
            bail!("odw exec requires --script unless --resume points to a prior exec run")
        }
    };
    if !script.exists() {
        bail!("workflow script does not exist: {}", script.display());
    }
    let input = match (&args.input, &args.input_file) {
        (Some(input), None) => input.clone(),
        (None, Some(file)) => {
            fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?
        }
        (None, None) => resume_run_dir
            .as_ref()
            .and_then(|run_dir| fs::read_to_string(run_dir.join("input.raw")).ok())
            .unwrap_or_default(),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
    };
    let current_exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "odw".to_string());
    let run_id = new_odw_run_id(if args.resume.is_some() {
        "exec-resume"
    } else {
        "exec"
    });
    let run_dir = root.join(".odw/runs").join(&run_id);
    let runner = run_dir.join("odw-js-runner.mjs");
    let state_path = run_dir.join("state.json");
    let resume_state_path = resume_run_dir
        .as_ref()
        .map(|run_dir| run_dir.join("state.json"))
        .filter(|path| path.exists());
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    fs::write(run_dir.join("input.raw"), &input)
        .with_context(|| format!("write {}", run_dir.join("input.raw").display()))?;
    fs::write(&runner, ODW_JS_RUNNER).with_context(|| format!("write {}", runner.display()))?;
    let command = vec![args.node_bin.clone(), runner.to_string_lossy().to_string()];
    if args.dry_run {
        println!("{}", shell_join(&command));
        return Ok(());
    }
    // Captured so the workflow's HTML execution graph can be auto-generated the
    // moment the run finishes — no separate command needed.
    let report_dir = run_dir.clone();
    let node_bin = args.node_bin.clone();
    let want_report = !args.no_report;
    let want_open = args.open;
    let json_only = args.json;
    let result = run_observable_script(
        &root,
        command,
        ScriptRunConfig {
            run_id,
            run_dir,
            runner,
            script,
            input,
            state_path,
            resume_from: args.resume,
            resume_state_path,
            backend: args.backend,
            odw_bin: current_exe,
            codexctl_bin: args.codexctl_bin,
            pandacode_bin: resolved_pandacode_bin(&args.pandacode_bin),
            provider: args.provider,
            model: args.model,
            effort: args.effort,
            timeout: args.timeout,
            json_only,
        },
    );
    // Auto-report after the run (success OR failure — the graph shows where it
    // failed). Best-effort: never let report generation mask the run's result.
    if want_report && report_dir.join("events.jsonl").exists() {
        let out_html = report_dir.join("report.html");
        if write_report(&report_dir, &out_html, &node_bin).is_ok() {
            if !json_only {
                println!("[odw] report: {}", out_html.display());
            }
            if want_open {
                open_path(&out_html);
            }
        }
    }
    result
}

#[derive(Debug, clap::Args)]
struct ReportArgs {
    #[arg(long, default_value = ".")]
    path: PathBuf,
    #[arg(long, help = "Mock-run this workflow script, then graph it")]
    script: Option<PathBuf>,
    #[arg(long, help = "Graph an existing run id, or 'latest'")]
    run: Option<String>,
    #[arg(long, help = "Input JSON for the mock run (with --script)")]
    input: Option<String>,
    #[arg(long, help = "Output HTML path (default: <run_dir>/report.html)")]
    out: Option<PathBuf>,
    #[arg(long, help = "Open the HTML when done")]
    open: bool,
    #[arg(long, default_value = "node")]
    node_bin: String,
}

fn report(args: ReportArgs) -> Result<()> {
    let root = normalize_root(&args.path)?;
    // Resolve the run to graph: a fresh mock dry-run of --script, an explicit
    // --run id, or the latest run.
    let run_dir = if let Some(script) = &args.script {
        mock_run_for_report(&root, script, args.input.as_deref())?
    } else {
        let run_id = args.run.as_deref().unwrap_or("latest");
        resolve_run_dir(&root, run_id)?
    };
    if !run_dir.join("events.jsonl").exists() {
        bail!(
            "no events.jsonl in {} — nothing to graph",
            run_dir.display()
        );
    }

    let out_html = args
        .out
        .clone()
        .unwrap_or_else(|| run_dir.join("report.html"));
    write_report(&run_dir, &out_html, &args.node_bin)?;
    println!("{}", out_html.display());
    if args.open {
        open_path(&out_html);
    }
    Ok(())
}

// Generate the HTML execution-graph report for a run. Shared by `odw report` and
// the auto-report at the end of `odw exec`. Offline render assets are written
// once to <root>/.odw/report-assets/ (not copied per run).
fn write_report(run_dir: &Path, out_html: &Path, node_bin: &str) -> Result<()> {
    if let Some(parent) = out_html.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    // <root>/.odw/report-assets — derived from <root>/.odw/runs/<id>.
    let assets_dir = run_dir
        .parent()
        .and_then(Path::parent)
        .map(|odw| odw.join("report-assets"))
        .unwrap_or_else(|| out_html.parent().map(Path::to_path_buf).unwrap_or_default());
    fs::create_dir_all(&assets_dir).with_context(|| format!("create {}", assets_dir.display()))?;
    let mermaid = assets_dir.join("mermaid.min.js");
    if !mermaid.exists() {
        fs::write(&mermaid, REPORT_MERMAID_JS)?;
    }
    let marked = assets_dir.join("marked.min.js");
    if !marked.exists() {
        fs::write(&marked, REPORT_MARKED_JS)?;
    }
    let generator = assets_dir.join(".odw-report.mjs");
    fs::write(&generator, ODW_REPORT_MJS)?;
    // Capture (not inherit) the generator's stdout so it never pollutes a
    // caller running `odw exec --json` (machine-readable single-line output).
    let out = std::process::Command::new(node_bin)
        .arg(&generator)
        .arg(run_dir)
        .arg(out_html)
        .arg(&assets_dir)
        .output()
        .with_context(|| format!("spawn {node_bin} (is Node installed?)"))?;
    if !out.status.success() {
        bail!(
            "report generator failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

// Resolve the pandacode executable. An explicit --pandacode-bin / ODW_PANDACODE_BIN
// always wins. Otherwise, prefer a `pandacode` built next to this odw binary: the
// Cargo workspace puts both in the same dir whether installed (`cargo install` ->
// ~/.cargo/bin) or just built (`cargo build` -> target/<profile>/). This makes a
// fresh `cargo build && ./target/release/odw …` work with no env vars; falls back
// to a bare `pandacode` (PATH lookup) when no sibling is found.
fn resolved_pandacode_bin(configured: &str) -> String {
    if configured != "pandacode" {
        return configured.to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        let bin_name = if cfg!(windows) { "pandacode.exe" } else { "pandacode" };
        if let Some(sibling) = exe.parent().map(|dir| dir.join(bin_name))
            && sibling.is_file()
        {
            return sibling.to_string_lossy().into_owned();
        }
    }
    configured.to_string()
}

fn open_path(path: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(path).status();
}

// Mock-run a workflow (token-free) so its execution graph can be rendered, and
// return the new run directory.
fn mock_run_for_report(root: &Path, script: &Path, input: Option<&str>) -> Result<PathBuf> {
    let current_exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "odw".to_string());
    let mut cmd = std::process::Command::new(&current_exe);
    cmd.arg("exec")
        .arg("--path")
        .arg(root)
        .arg("--script")
        .arg(script)
        .arg("--backend")
        .arg("mock");
    if let Some(input) = input {
        cmd.arg("--input").arg(input);
    }
    let output = cmd
        .output()
        .with_context(|| "run mock dry-run for report")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = text
        .split_whitespace()
        .find_map(|token| token.strip_prefix("run_id="))
        .ok_or_else(|| anyhow::anyhow!("mock run produced no run_id:\n{}", text))?;
    Ok(root.join(".odw/runs").join(run_id))
}

struct ScriptRunConfig {
    run_id: String,
    run_dir: PathBuf,
    runner: PathBuf,
    script: PathBuf,
    input: String,
    state_path: PathBuf,
    resume_from: Option<String>,
    resume_state_path: Option<PathBuf>,
    backend: String,
    odw_bin: String,
    codexctl_bin: String,
    pandacode_bin: String,
    provider: Option<String>,
    model: Option<String>,
    effort: String,
    timeout: String,
    json_only: bool,
}

fn run_observable_script(root: &Path, command: Vec<String>, config: ScriptRunConfig) -> Result<()> {
    let started_ms = now_millis();
    let events_path = config.run_dir.join("events.jsonl");
    let debug_file = config.run_dir.join("script-debug.log");
    let workflow = config.script.to_string_lossy().to_string();
    fs::write(&debug_file, "").with_context(|| format!("write {}", debug_file.display()))?;
    let mut journal =
        File::create(&events_path).with_context(|| format!("create {}", events_path.display()))?;
    write_run_record(
        root,
        &config.run_dir,
        RunRecordInput {
            run_id: &config.run_id,
            action: "exec",
            workflow: Some(&workflow),
            status: "running",
            started_ms,
            finished_ms: None,
            session_id: None,
            command: &command,
            debug_file: &debug_file,
        },
    )?;
    write_journal_event(
        &mut journal,
        json!({
            "type": "launch",
            "run_id": &config.run_id,
            "action": "exec",
            "script": &workflow,
            "backend": &config.backend,
            "runner": &config.runner,
            "state_path": &config.state_path,
            "resume_from": &config.resume_from,
            "resume_state_path": &config.resume_state_path,
            "command": &command,
            "shell": shell_join(&command)
        }),
    )?;
    if !config.json_only {
        println!("[odw] run_id={}", config.run_id);
        println!("[odw] journal={}", events_path.display());
        println!("[odw] script={}", config.script.display());
        println!("[odw] backend={}", config.backend);
        println!("[odw] {}", shell_join(&command));
    }

    let Some((program, args)) = command.split_first() else {
        bail!("empty script runner command");
    };
    let mut child = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("ODW_SCRIPT_PATH", &config.script)
        .env("ODW_INPUT", &config.input)
        .env("ODW_BACKEND", &config.backend)
        .env("ODW_RUN_ID", &config.run_id)
        .env("ODW_CWD", root)
        .env("ODW_RUN_DIR", &config.run_dir)
        .env("ODW_STATE_PATH", &config.state_path)
        .env(
            "ODW_RESUME_STATE_PATH",
            config
                .resume_state_path
                .as_ref()
                .map(|path| path.as_os_str())
                .unwrap_or_default(),
        )
        .env(
            "ODW_RESUME_FROM",
            config.resume_from.as_deref().unwrap_or_default(),
        )
        .env("ODW_BIN", &config.odw_bin)
        .env("ODW_CODEXCTL_BIN", &config.codexctl_bin)
        .env("ODW_PANDACODE_BIN", &config.pandacode_bin)
        .env("ODW_PROVIDER", config.provider.as_deref().unwrap_or(""))
        .env("ODW_MODEL", config.model.as_deref().unwrap_or(""))
        .env("ODW_EFFORT", &config.effort)
        .env("ODW_TIMEOUT", &config.timeout)
        .spawn()
        .with_context(|| format!("run {program}"))?;

    let (tx, rx) = mpsc::channel::<ProcessLine>();
    if let Some(stdout) = child.stdout.take() {
        spawn_output_reader(stdout, "stdout", tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_output_reader(stderr, "stderr", tx);
    }

    let mut workflow_failed = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                workflow_failed |=
                    handle_script_line(&mut journal, &debug_file, &line, config.json_only)?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }

        if let Some(status) = child.try_wait()? {
            for line in rx.try_iter() {
                workflow_failed |=
                    handle_script_line(&mut journal, &debug_file, &line, config.json_only)?;
            }
            let finished_ms = now_millis();
            let status_text = if status.success() && !workflow_failed {
                "completed"
            } else {
                "failed"
            };
            write_journal_event(
                &mut journal,
                json!({
                    "type": "exit",
                    "status": status_text,
                    "code": status.code()
                }),
            )?;
            write_run_record(
                root,
                &config.run_dir,
                RunRecordInput {
                    run_id: &config.run_id,
                    action: "exec",
                    workflow: Some(&workflow),
                    status: status_text,
                    started_ms,
                    finished_ms: Some(finished_ms),
                    session_id: None,
                    command: &command,
                    debug_file: &debug_file,
                },
            )?;
            if !status.success() {
                bail!(
                    "ODW script exited with status {status}; journal: {}",
                    events_path.display()
                );
            }
            if workflow_failed {
                bail!(
                    "ODW workflow returned ok:false; journal: {}",
                    events_path.display()
                );
            }
            if !config.json_only {
                println!("[odw] completed run_id={}", config.run_id);
                println!("[odw] logs: ./.odw/bin/odw runs show {}", config.run_id);
            }
            return Ok(());
        }
    }
}

fn handle_script_line(
    journal: &mut File,
    debug_file: &Path,
    line: &ProcessLine,
    json_only: bool,
) -> Result<bool> {
    let parsed = serde_json::from_str::<serde_json::Value>(&line.line).ok();
    let workflow_failed = parsed.as_ref().is_some_and(|value| {
        value.get("type").and_then(|t| t.as_str()) == Some("workflow_done")
            && value
                .get("result")
                .and_then(|r| r.get("ok"))
                .and_then(|ok| ok.as_bool())
                == Some(false)
    });
    let summary = parsed
        .as_ref()
        .map(summarize_script_event)
        .unwrap_or_else(|| truncate(&line.line, 240));
    write_journal_event(
        journal,
        json!({
            "type": "script_stream",
            "stream": line.stream,
            "summary": summary,
            "raw": parsed.clone().unwrap_or_else(|| json!(line.line))
        }),
    )?;
    if line.stream == "stderr" {
        let mut debug = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(debug_file)
            .with_context(|| format!("open {}", debug_file.display()))?;
        writeln!(debug, "{}", line.line)?;
        if !summary.trim().is_empty() {
            eprintln!("[script:stderr] {summary}");
        }
    } else if json_only {
        if parsed
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(|value| value.as_str())
            == Some("workflow_done")
        {
            let result = parsed
                .as_ref()
                .and_then(|value| value.get("result"))
                .unwrap_or(&serde_json::Value::Null);
            println!("{}", serde_json::to_string(result)?);
        }
    } else if !summary.trim().is_empty() {
        println!("{summary}");
    }
    Ok(workflow_failed)
}

// Field accessors shared across summarize_script_event arms: a string defaulting
// to "-", and a u64 with an explicit default. Keeps each arm to its format!.
fn ev_str(value: &serde_json::Value, key: &str) -> String {
    json_string(value, key).unwrap_or_else(|| "-".to_string())
}
fn ev_u64(value: &serde_json::Value, key: &str, default: u64) -> u64 {
    value
        .get(key)
        .and_then(|field| field.as_u64())
        .unwrap_or(default)
}

fn summarize_script_event(value: &serde_json::Value) -> String {
    match value.get("type").and_then(|field| field.as_str()) {
        Some("workflow_start") => {
            let name = json_string(value, "name").unwrap_or_else(|| "workflow".to_string());
            let backend = json_string(value, "backend").unwrap_or_else(|| "unknown".to_string());
            format!("[workflow] start {name} backend={backend}")
        }
        Some("workflow_done") => {
            let name = json_string(value, "name").unwrap_or_else(|| "workflow".to_string());
            let result = value
                .get("result")
                .filter(|r| !r.is_null())
                .map(|r| serde_json::to_string(r).unwrap_or_default())
                .filter(|s| !s.is_empty())
                .map(|s| format!("\n[result] {s}"))
                .unwrap_or_default();
            format!("[workflow] done {name}{result}")
        }
        Some("workflow_error") => {
            let message = json_string(value, "message").unwrap_or_else(|| "unknown".to_string());
            format!("[workflow] error {message}")
        }
        Some("phase") => {
            let title = json_string(value, "title").unwrap_or_else(|| "phase".to_string());
            let detail = json_string(value, "detail")
                .map(|detail| format!(" - {detail}"))
                .unwrap_or_default();
            format!("[phase] {title}{detail}")
        }
        Some("agent_start") => {
            let phase = ev_str(value, "phase");
            let label = ev_str(value, "label");
            let agent_type = ev_str(value, "agentType");
            let attempt = ev_u64(value, "attempt", 1);
            let max_attempts = ev_u64(value, "maxAttempts", 1);
            if max_attempts > 1 {
                format!(
                    "[node] start {phase} / {label} / {agent_type} attempt={attempt}/{max_attempts}"
                )
            } else {
                format!("[node] start {phase} / {label} / {agent_type}")
            }
        }
        Some("agent_schema_invalid") => {
            let phase = ev_str(value, "phase");
            let label = ev_str(value, "label");
            let attempt = ev_u64(value, "attempt", 1);
            let max_attempts = ev_u64(value, "maxAttempts", 1);
            format!("[node] schema-mismatch {phase} / {label} attempt={attempt}/{max_attempts}")
        }
        Some("agent_retry") => {
            let phase = ev_str(value, "phase");
            let label = ev_str(value, "label");
            let reason = json_string(value, "reason").unwrap_or_else(|| "retry".to_string());
            let next_attempt = ev_u64(value, "nextAttempt", 1);
            let max_attempts = ev_u64(value, "maxAttempts", 1);
            format!(
                "[node] retry {phase} / {label} reason={reason} next={next_attempt}/{max_attempts}"
            )
        }
        Some("parallel_start") => {
            let phase = ev_str(value, "phase");
            let label = json_string(value, "label").unwrap_or_else(|| "parallel".to_string());
            let count = ev_u64(value, "count", 0);
            let max = ev_u64(value, "max", 0);
            format!("[parallel] start {phase} / {label} count={count} max={max}")
        }
        Some("parallel_done") => {
            let phase = ev_str(value, "phase");
            let label = json_string(value, "label").unwrap_or_else(|| "parallel".to_string());
            let ok = value
                .get("ok")
                .and_then(|field| field.as_bool())
                .unwrap_or(true);
            format!("[parallel] done {phase} / {label} ok={ok}")
        }
        Some("pipeline_start") => {
            let phase = ev_str(value, "phase");
            let count = ev_u64(value, "count", 0);
            let stages = ev_u64(value, "stages", 0);
            let max = ev_u64(value, "max", 0);
            format!("[pipeline] start {phase} items={count} stages={stages} max={max}")
        }
        Some("pipeline_done") => {
            let phase = ev_str(value, "phase");
            let ok = value
                .get("ok")
                .and_then(|field| field.as_bool())
                .unwrap_or(true);
            format!("[pipeline] done {phase} ok={ok}")
        }
        Some("agent_skip") => {
            let phase = ev_str(value, "phase");
            let label = ev_str(value, "label");
            let agent_type = ev_str(value, "agentType");
            format!("[node] skip {phase} / {label} / {agent_type} cached=true")
        }
        Some("agent_done") => {
            let phase = ev_str(value, "phase");
            let label = ev_str(value, "label");
            let agent_type = ev_str(value, "agentType");
            let ok = value
                .get("ok")
                .and_then(|field| field.as_bool())
                .unwrap_or(true);
            format!("[node] done {phase} / {label} / {agent_type} ok={ok}")
        }
        Some("log") => {
            let message = json_string(value, "message").unwrap_or_default();
            format!("[log] {message}")
        }
        Some("checkpoint") => {
            let name = json_string(value, "name").unwrap_or_else(|| "checkpoint".to_string());
            format!("[checkpoint] {name}")
        }
        Some("worktree_start") => {
            let label = ev_str(value, "label");
            format!("[worktree] start {label}")
        }
        Some("worktree_done") => {
            let label = ev_str(value, "label");
            let changed = value
                .get("changed")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            let files = value.get("files").and_then(|f| f.as_u64()).unwrap_or(0);
            format!("[worktree] done {label} changed={changed} files={files}")
        }
        Some("panda_auto_answer") => {
            let runtime = json_string(value, "runtime").unwrap_or_else(|| "-".to_string());
            let round = value.get("round").and_then(|r| r.as_u64()).unwrap_or(0);
            format!("[answer] auto-answer {runtime} needs_input round={round}")
        }
        Some("parallel_item_error") => {
            let label = ev_str(value, "label");
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let message = json_string(value, "message").unwrap_or_default();
            format!("[parallel] item-error {label}[{index}] {message}")
        }
        Some("pipeline_item_error") => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let message = json_string(value, "message").unwrap_or_default();
            format!("[pipeline] item-error [{index}] {message}")
        }
        Some(other) => format!("[event] {other}"),
        None => truncate(&value.to_string(), 240),
    }
}

struct RunRecordInput<'a> {
    run_id: &'a str,
    action: &'a str,
    workflow: Option<&'a str>,
    status: &'a str,
    started_ms: u128,
    finished_ms: Option<u128>,
    session_id: Option<&'a str>,
    command: &'a [String],
    debug_file: &'a Path,
}

#[derive(Debug)]
struct ProcessLine {
    stream: &'static str,
    line: String,
}

fn spawn_output_reader<R>(reader: R, stream: &'static str, tx: mpsc::Sender<ProcessLine>)
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = io::BufReader::new(reader);
        for line in reader.lines().map_while(|line| line.ok()) {
            if tx.send(ProcessLine { stream, line }).is_err() {
                break;
            }
        }
    });
}

fn write_journal_event(journal: &mut File, mut event: serde_json::Value) -> Result<()> {
    if let Some(object) = event.as_object_mut() {
        object.insert("ts_ms".to_string(), json!(now_millis()));
    }
    serde_json::to_writer(&mut *journal, &event)?;
    writeln!(journal)?;
    journal.flush()?;
    Ok(())
}

fn write_run_record(root: &Path, run_dir: &Path, input: RunRecordInput<'_>) -> Result<()> {
    let value = json!({
        "run_id": input.run_id,
        "action": input.action,
        "workflow": input.workflow,
        "status": input.status,
        "started_ms": input.started_ms,
        "finished_ms": input.finished_ms,
        "session_id": input.session_id,
        "command": input.command,
        "shell": shell_join(input.command),
        "run_dir": run_dir,
        "events_path": run_dir.join("events.jsonl"),
        "debug_file": input.debug_file
    });
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_string_pretty(&value)?,
    )?;
    fs::write(
        root.join(".odw/runs/latest.json"),
        serde_json::to_string_pretty(&value)?,
    )?;
    Ok(())
}

fn resolve_run_dir(root: &Path, run_id: &str) -> Result<PathBuf> {
    let runs = root.join(".odw/runs");
    if run_id == "latest" {
        let latest = runs.join("latest.json");
        let content =
            fs::read_to_string(&latest).with_context(|| format!("read {}", latest.display()))?;
        let value = serde_json::from_str::<serde_json::Value>(&content)
            .with_context(|| format!("parse {}", latest.display()))?;
        // Prefer re-deriving from the recorded run_id (a flat, validated name);
        // only honor a recorded run_dir that stays under .odw/runs, so a tampered
        // latest.json cannot redirect reads to an arbitrary path.
        if let Some(id) = json_string(&value, "run_id") {
            return safe_run_dir(&runs, &id);
        }
        if let Some(path) = json_string(&value, "run_dir") {
            let path = PathBuf::from(path);
            if path.starts_with(&runs) {
                return Ok(path);
            }
            bail!("{} run_dir is outside .odw/runs", latest.display());
        }
        bail!("{} does not contain run_id", latest.display());
    }
    safe_run_dir(&runs, run_id)
}

// A run id is a single run-directory name; reject anything that could traverse
// out of .odw/runs (a separator, "..", or the "latest" sentinel).
fn safe_run_dir(runs: &Path, run_id: &str) -> Result<PathBuf> {
    if run_id.is_empty()
        || run_id == "latest"
        || run_id.contains('/')
        || run_id.contains('\\')
        || run_id.contains("..")
    {
        bail!("invalid run id (must be a single run directory name): {run_id}");
    }
    Ok(runs.join(run_id))
}

fn read_tail_lines(path: &Path, tail: usize) -> Result<Vec<serde_json::Value>> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // Bound the work even if a caller passes an absurd --tail.
    let tail = tail.min(1_000_000);
    let mut lines = content.lines().rev().take(tail).collect::<Vec<_>>();
    lines.reverse();
    Ok(lines
        .into_iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|_| json!(line)))
        .collect())
}

fn new_odw_run_id(action: &str) -> String {
    format!("odw-{action}-{}-{}", now_millis(), std::process::id())
}

fn now_millis() -> u128 {
    system_time_millis(SystemTime::now())
}

fn system_time_millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut result = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        result.push_str("...");
    }
    result.replace('\n', "\\n")
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn workflow_evidence_report(
    root: &Path,
    claude_projects_dir: Option<&Path>,
    session_filter: Option<&str>,
    run_filter: Option<&str>,
) -> Result<serde_json::Value> {
    let root = normalize_root(root)?;
    let projects_dir = match claude_projects_dir {
        Some(path) => path.to_path_buf(),
        None => default_claude_projects_dir()?,
    };
    let project_slug = claude_project_slug(&root);
    let project_dir = projects_dir.join(&project_slug);
    let mut artifacts = Vec::new();

    if project_dir.exists() {
        for session_dir in sorted_dirs(&project_dir)? {
            let session_id = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string();
            if let Some(filter) = session_filter
                && session_id != filter
            {
                continue;
            }
            let workflows_dir = session_dir.join("workflows");
            if !workflows_dir.exists() {
                continue;
            }
            for artifact in sorted_files(&workflows_dir)? {
                if artifact.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let content = fs::read_to_string(&artifact)
                    .with_context(|| format!("read {}", artifact.display()))?;
                let value: serde_json::Value = serde_json::from_str(&content)
                    .with_context(|| format!("parse {}", artifact.display()))?;
                let run_id = value
                    .get("runId")
                    .and_then(|field| field.as_str())
                    .unwrap_or("");
                if let Some(filter) = run_filter
                    && run_id != filter
                {
                    continue;
                }
                artifacts.push(workflow_artifact_summary(
                    &project_dir,
                    &session_id,
                    &artifact,
                    &value,
                ));
            }
        }
    }

    Ok(json!({
        "ok": !artifacts.is_empty(),
        "project": root.to_string_lossy(),
        "claude_project_slug": project_slug,
        "claude_project_dir": project_dir.to_string_lossy(),
        "filters": {
            "session_id": session_filter,
            "run_id": run_filter
        },
        "artifacts": artifacts,
        "note": "This reads Claude Code workflow artifacts. It does not control live /workflows state."
    }))
}

fn workflow_artifact_summary(
    project_dir: &Path,
    session_id: &str,
    artifact: &Path,
    value: &serde_json::Value,
) -> serde_json::Value {
    let phases = value
        .get("phases")
        .and_then(|field| field.as_array())
        .map(|items| {
            items
                .iter()
                .map(|phase| {
                    json!({
                        "title": json_string(phase, "title"),
                        "detail": json_string(phase, "detail")
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let agents = value
        .get("workflowProgress")
        .and_then(|field| field.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("type").and_then(|field| field.as_str()) == Some("workflow_agent")
                })
                .map(|agent| {
                    json!({
                        "index": agent.get("index").and_then(|field| field.as_u64()),
                        "label": json_string(agent, "label"),
                        "phase": json_string(agent, "phaseTitle"),
                        "agent_id": json_string(agent, "agentId"),
                        "agent_type": json_string(agent, "agentType"),
                        "model": json_string(agent, "model"),
                        "state": json_string(agent, "state"),
                        "attempt": agent.get("attempt").and_then(|field| field.as_u64()),
                        "tool_calls": agent.get("toolCalls").and_then(|field| field.as_u64()),
                        "tokens": agent.get("tokens").and_then(|field| field.as_u64())
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "path": artifact.to_string_lossy(),
        "relative_path": relative_display(project_dir, artifact),
        "session_id": session_id,
        "run_id": json_string(value, "runId"),
        "workflow_name": json_string(value, "workflowName"),
        "status": json_string(value, "status"),
        "summary": json_string(value, "summary"),
        "timestamp": json_string(value, "timestamp"),
        "script_path": json_string(value, "scriptPath"),
        "default_model": json_string(value, "defaultModel"),
        "agent_count": value.get("agentCount").and_then(|field| field.as_u64()).unwrap_or(agents.len() as u64),
        "total_tokens": value.get("totalTokens").and_then(|field| field.as_u64()),
        "total_tool_calls": value.get("totalToolCalls").and_then(|field| field.as_u64()),
        "duration_ms": value.get("durationMs").and_then(|field| field.as_u64()),
        "phases": phases,
        "agents": agents
    })
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .map(ToString::to_string)
}

fn default_claude_projects_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --claude-projects-dir"))?;
    Ok(home.join(".claude/projects"))
}

fn claude_project_slug(root: &Path) -> String {
    root.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' => '-',
            _ => ch,
        })
        .collect()
}

fn capabilities_json() -> serde_json::Value {
    json!({
        "primary_user": "Agent or CLI caller",
        "runtime": "ODW direct JavaScript runner",
        "optional_integration": "Claude Code Dynamic Workflows",
        "agent_bridge": "PandaCode dispatches each node to its codex/claude/bamboo runtime via single-shot `pandacode <runtime> exec`",
        "lifecycle": {
            "exec": {
                "cli": "odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>",
                "resume": "odw exec --resume <run_id|latest>",
                "note": "Direct script runner. This is the agent-driven path: a caller writes or selects workflow JavaScript, then ODW runs it and records node progress."
            },
            "watch": {
                "cli": "odw runs show <run_id|latest>",
                "claude_code": "/workflows for Claude-launched runs",
                "note": "Direct runs are watched through ODW journals. Claude-launched runs can also use Claude Code's workflow UI."
            },
            "pause_resume": {
                "claude_code": "/workflows then p",
                "cli": "odw exec --resume <run_id|latest>",
                "note": "Direct exec resumes from .odw/runs/<run_id>/state.json and skips completed node ids."
            },
            "stop": {
                "claude_code": "/workflows then x"
            },
            "restart_agent": {
                "claude_code": "/workflows then r"
            },
            "save": {
                "claude_code": "/workflows then s"
            },
            "remove": {
                "cli": "odw workflows remove <name>"
            },
            "evidence": {
                "cli": "odw evidence --path <project>",
                "note": "Reads saved Claude Code workflow artifacts under ~/.claude/projects."
            },
            "observability": {
                "cli": "odw exec streams direct node progress; use odw runs list/show for journals.",
                "files": ".odw/runs/<odw-run-id>/events.jsonl",
                "note": "ODW records workflow_start, phase, node start/done/skip, checkpoint, error, and exit events for direct runs."
            },
            "spec": {
                "cli": "odw spec",
                "note": "Documents the direct workflow script contract, Codex helpers, and compatibility surfaces."
            },
            "error_feedback": {
                "schema": ".odw/schemas/error-feedback.schema.json",
                "note": "Worker failures must be returned as classified, retry-aware feedback."
            }
        },
        "composition": {
            "parallel": "Dynamic Workflow-compatible parallel([() => agent(...), ...]) fan-out/join with max 16 concurrent thunks",
            "fanout": "fanout(items, mapper) dynamically maps structured upstream output into parallel downstream nodes",
            "pipeline": "Dynamic Workflow-compatible pipeline(items, ...stages) streams each item through sequential stages while items fan out",
            "schemas": "Optional .odw/schemas/*.schema.json contracts. No node receives a default schema; workflow code opts in with schema (schemaDescription optional) for runtime validation, schema_mismatch feedback, and same-node retry context injection",
            "observability": ".odw/runs/*.json and .odw/runs/*/events.jsonl",
            "agent_types": built_in_agents().iter().map(|agent| agent.name).collect::<Vec<_>>()
        }
    })
}

fn framework_spec_json() -> serde_json::Value {
    json!({
        "name": "Open Dynamic Workflow",
        "short_name": "ODW",
        "version": ODW_VERSION,
        "compatibility_target": {
            "runtime": "ODW direct JavaScript runner",
            "optional_runtime": "Claude Code Dynamic Workflows",
            "minimum_observed_claude_code_version": "2.1.154",
            "project_subagents": ".claude/agents/*.md",
            "project_commands": ".claude/commands/*.md",
            "project_workflows": ".claude/workflows/*.js",
            "management_surface": "odw runs list/show; Claude Code /workflows for Claude-launched runs"
        },
        "script_contract": {
            "language": "JavaScript module",
            "entrypoint": "Dynamic Workflow-compatible top-level script: export const meta = {...}; phase(...); const result = await agent(...); return result;",
            "metadata": "export const meta = { name, description, phases, agents, schemas, promptSlots }",
            "input": "Workflow input is available as global args; ODW also passes it as input for compatibility wrappers.",
            "log": "log(message) emits workflow-level progress.",
            "phase": "phase(title: string, detail?: string)",
            "prompt_slot": "promptSlot(name: string, context?: object, suggested?: string) reads input.prompts.<name>; suggested text is enabled for mock smoke tests or explicit caller opt-in.",
            "agent": "agent(prompt: string, options?: { id?: string, label?: string, phase?: string, agentType?: string, nodeType?: string, runtime?: 'claude'|'codex'|'bamboo'|string, provider?: string, model?: string, schema?: string|object, schemaDescription?: string, retry?: { maxAttempts?: number } })",
            "agent_bridge": "With --backend pandacode, ordinary agent(...) nodes dispatch to PandaCode runtime='claude', runtime='codex', or runtime='bamboo'. Passing provider selects Bamboo and becomes pandacode bamboo exec --provider <provider>.",
            "codex": "Route Codex through ordinary agent(prompt, { runtime: 'codex' }); agentType is optional metadata.",
            "budget": "budget is exposed as a workflow global for compatibility.",
            "checkpoint": "checkpoint(name: string, value?: unknown) persists resume state.",
            "parallel": "parallel([() => agent(...), ...]) is the primary node fan-out/join API; keep concurrency <= 16.",
            "fanout": "fanout(items, mapper) maps structured upstream output into dynamic parallel child nodes.",
            "pipeline": "pipeline(items, ...stages) runs each item through sequential stages while items fan out.",
            "schema_retry": "No schema is applied by default. When options.schema is set, schemaDescription is optional; direct exec appends the full JSON Schema as a final-response-only contract, validates that final response, emits agent_schema_invalid on mismatch, injects validation context into the same node prompt, and retries up to retry.maxAttempts.",
            "prompt_style": "Workflow scripts declare prompt slots. Real runs inject input.prompts.<slot>; mock runs may use suggested template literals for smoke tests.",
            "error_feedback": "Every node prompt includes a failure contract. A worker that cannot complete returns .odw/schemas/error-feedback.schema.json instead of unstructured prose.",
            "limits": {
                "max_concurrent_agents": 16,
                "max_agents_per_run": 1000,
                "workflow_script_io": "Workflow scripts are sandboxed orchestration code. File, shell, and code-edit work must go through agent(...) executor nodes."
            }
        },
        "lifecycle": {
            "run": "odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>; optional /odw, /odw-audit, /odw-ship, /odw-flow for Claude Code",
            "watch": "odw runs show <run_id|latest>",
            "observe": "odw exec live stream + odw runs list/show journals",
            "pause_resume": "odw exec --resume <run_id|latest>; optional /workflows then p for Claude Code",
            "stop": "stop the invoking process; Claude Code /workflows then x for Claude-launched runs",
            "restart_agent": "direct exec resumes by node id; Claude Code /workflows then r for Claude-launched runs",
            "save": "workflow scripts are normal files; Claude Code /workflows then s for Claude-launched scripts",
            "remove_template": "odw workflows remove <name>",
            "artifact_evidence": "odw evidence",
            "run_journal": ".odw/runs/<run_id>/events.jsonl"
        },
        "agent_types": built_in_agents().iter().map(|agent| json!({
            "name": agent.name,
            "description": agent.description
        })).collect::<Vec<_>>(),
        "extension_points": {
            "new_agent": "Add .claude/agents/<name>.md with Claude Code subagent frontmatter.",
            "new_command": "Add .claude/commands/<name>.md with a prompt and $ARGUMENTS.",
            "new_workflow": "Add .claude/workflows/<name>.js following .odw/framework/workflow-api.d.ts.",
            "new_schema": "Schemas are opt-in. Add .odw/schemas/<name>.schema.json, then reference it from workflow code with agent(..., { schema, schemaDescription })."
        }
    })
}

fn validate_pack_status(root: &Path) -> PackStatus {
    let mut missing = Vec::new();
    let mut invalid_agents = Vec::new();
    let mut invalid_files = Vec::new();
    for file in pack_files() {
        let target = root.join(file.path);
        if !target.exists() {
            missing.push(file.path.to_string());
        }
    }
    let project_bin = root.join(ODW_PROJECT_BIN_PATH);
    if !project_bin.exists() {
        missing.push(ODW_PROJECT_BIN_PATH.to_string());
    }
    #[cfg(unix)]
    if project_bin.exists() {
        use std::os::unix::fs::PermissionsExt;

        match fs::metadata(&project_bin) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 == 0 => {
                invalid_files.push(format!("{ODW_PROJECT_BIN_PATH} is not executable"));
            }
            Err(error) => invalid_files.push(format!(
                "{ODW_PROJECT_BIN_PATH}: failed to read permissions: {error}"
            )),
            _ => {}
        }
    }
    let agent_dir = root.join(".claude/agents");
    if agent_dir.exists() {
        match sorted_files(&agent_dir) {
            Ok(files) => {
                for file in files {
                    match fs::read_to_string(&file) {
                        Ok(content)
                            if frontmatter_value(&content, "name").is_none()
                                || frontmatter_value(&content, "description").is_none() =>
                        {
                            invalid_agents.push(relative_display(root, &file));
                        }
                        Ok(_) => {}
                        Err(error) => invalid_files.push(format!(
                            "{}: failed to read: {error}",
                            relative_display(root, &file)
                        )),
                    }
                }
            }
            Err(error) => invalid_agents.push(error.to_string()),
        }
    }
    for agent in built_in_agents() {
        let file = root.join(format!(".claude/agents/{}.md", agent.name));
        if !file.exists() {
            missing.push(file.to_string_lossy().to_string());
        }
    }
    for required in [
        ".claude/workflows/odw-audit.js",
        ".claude/workflows/odw-ship.js",
        ".claude/workflows/odw-flow.js",
        ".odw/framework/runtime-contract.md",
        ".odw/framework/workflow-api.d.ts",
        ".odw/schemas/codex-plan.schema.json",
        ".odw/schemas/codex-result.schema.json",
        ".odw/schemas/error-feedback.schema.json",
        ".odw/schemas/task-plan.schema.json",
        ".odw/schemas/task-join.schema.json",
        ".odw/schemas/quality-gate.schema.json",
        ".odw/schemas/verifier.schema.json",
        ".odw/schemas/workflow-manifest.schema.json",
    ] {
        let file = root.join(required);
        if !file.exists() {
            missing.push(required.to_string());
        }
    }
    let schema_dir = root.join(".odw/schemas");
    match fs::read_dir(&schema_dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(schema) => {
                        let path = schema.path();
                        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                            match fs::read_to_string(&path) {
                                Ok(content)
                                    if serde_json::from_str::<serde_json::Value>(&content)
                                        .is_err() =>
                                {
                                    invalid_files.push(relative_display(root, &path));
                                }
                                Ok(_) => {}
                                Err(error) => invalid_files.push(format!(
                                    "{}: failed to read: {error}",
                                    relative_display(root, &path)
                                )),
                            }
                        }
                    }
                    Err(error) => invalid_files.push(format!(
                        "{}: failed to read directory entry: {error}",
                        relative_display(root, &schema_dir)
                    )),
                }
            }
        }
        Err(error) => invalid_files.push(format!(
            "{}: failed to read directory: {error}",
            relative_display(root, &schema_dir)
        )),
    }
    let settings = root.join(".claude/settings.odw.example.json");
    if let Ok(content) = fs::read_to_string(&settings)
        && !content.contains("odw exec")
    {
        invalid_files.push(format!(
            "{} missing the odw exec permission example",
            relative_display(root, &settings)
        ));
    }
    PackStatus {
        ok: missing.is_empty() && invalid_agents.is_empty() && invalid_files.is_empty(),
        missing,
        invalid_agents,
        invalid_files,
    }
}

fn normalize_root(root: &Path) -> Result<PathBuf> {
    if root.exists() {
        root.canonicalize()
            .with_context(|| format!("canonicalize {}", root.display()))
    } else {
        Ok(root.to_path_buf())
    }
}

fn sorted_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    dirs.sort();
    Ok(dirs)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn run_version(command: &str, args: &[&str]) -> ToolStatus {
    match Command::new(command).args(args).output() {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            ToolStatus {
                ok: output.status.success(),
                command: command.to_string(),
                summary: text.lines().next().unwrap_or("").to_string(),
            }
        }
        Err(error) => ToolStatus {
            ok: false,
            command: command.to_string(),
            summary: error.to_string(),
        },
    }
}

fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if left.trim() == key {
            return Some(right.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[derive(serde::Serialize)]
struct ToolStatus {
    ok: bool,
    command: String,
    summary: String,
}

#[derive(Debug, serde::Serialize)]
struct PackStatus {
    ok: bool,
    missing: Vec<String>,
    invalid_agents: Vec<String>,
    invalid_files: Vec<String>,
}

struct BuiltInAgent {
    name: &'static str,
    description: &'static str,
}

fn built_in_agents() -> &'static [BuiltInAgent] {
    &[
        BuiltInAgent {
            name: "odw-orchestrator",
            description: "Workflow coordinator that writes ODW JavaScript workflows and routes to allowed ODW agents.",
        },
        BuiltInAgent {
            name: "odw-codex-coder",
            description: "Implementation worker that delegates coding work to the PandaCode Codex executor (single-shot) instead of editing directly.",
        },
        BuiltInAgent {
            name: "odw-researcher",
            description: "Read-only discovery worker for repository inventory and evidence collection.",
        },
        BuiltInAgent {
            name: "odw-security-reviewer",
            description: "Read-only security reviewer that reports only evidence-backed findings.",
        },
        BuiltInAgent {
            name: "odw-test-runner",
            description: "Verification worker that runs scoped test commands and summarizes failures.",
        },
        BuiltInAgent {
            name: "odw-failure-analyst",
            description: "Classifies failed worker/Codex results into retry-aware structured error feedback.",
        },
        BuiltInAgent {
            name: "odw-verifier",
            description: "Adversarial verifier that rejects weak or duplicated worker claims.",
        },
        BuiltInAgent {
            name: "odw-synthesizer",
            description: "Final report worker that produces concise, cited synthesis from verified results.",
        },
    ]
}

fn pack_files() -> &'static [pack::PackFile] {
    pack::files()
}

fn contract_text() -> &'static str {
    pack::contract_text()
}

const ODW_JS_RUNNER: &str = pack::ODW_JS_RUNNER;
const ODW_REPORT_MJS: &str = pack::ODW_REPORT_MJS;
const REPORT_MERMAID_JS: &str = pack::REPORT_MERMAID_JS;
const REPORT_MARKED_JS: &str = pack::REPORT_MARKED_JS;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn init_installs_pack_and_validates() {
        let root = temp_root("init-validates");
        fs::create_dir_all(&root).unwrap();
        init(&root, false, false).unwrap();
        let status = validate_pack_status(&root);
        assert!(status.ok, "{status:?}");
        assert!(root.join(".claude/agents/odw-codex-coder.md").exists());
        assert!(root.join(".claude/workflows/odw-audit.js").exists());
        assert!(root.join(".claude/workflows/odw-flow.js").exists());
        assert!(root.join(ODW_PROJECT_BIN_PATH).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(root.join(ODW_PROJECT_BIN_PATH))
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0);
        }
        assert!(root.join(".odw/framework/workflow-api.d.ts").exists());
        assert!(root.join(".odw/schemas/codex-plan.schema.json").exists());
        assert!(root.join(".odw/schemas/codex-result.schema.json").exists());
        assert!(root.join(".odw/schemas/task-plan.schema.json").exists());
        assert!(root.join(".odw/schemas/task-join.schema.json").exists());
        assert!(root.join(".odw/schemas/quality-gate.schema.json").exists());
        assert!(
            root.join(".odw/schemas/workflow-manifest.schema.json")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validate_pack_status_reports_unreadable_checked_file() {
        let root = temp_root("validate-unreadable");
        fs::create_dir_all(&root).unwrap();
        init(&root, false, false).unwrap();

        let relative = ".odw/schemas/codex-plan.schema.json";
        let schema = root.join(relative);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let original_permissions = fs::metadata(&schema).unwrap().permissions();
            let mut unreadable = original_permissions.clone();
            unreadable.set_mode(0o000);
            fs::set_permissions(&schema, unreadable).unwrap();

            let status = validate_pack_status(&root);

            fs::set_permissions(&schema, original_permissions).unwrap();
            if status
                .invalid_files
                .iter()
                .any(|invalid| invalid.contains(relative))
            {
                fs::remove_dir_all(root).unwrap();
                return;
            }
        }

        fs::write(&schema, "{not json").unwrap();
        let status = validate_pack_status(&root);
        assert!(
            status
                .invalid_files
                .iter()
                .any(|invalid| invalid.contains(relative)),
            "{status:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graphs_list_reports_workflow_scripts_and_manifests() {
        let root = temp_root("graphs-list");
        fs::create_dir_all(&root).unwrap();
        init(&root, false, false).unwrap();

        let report = graphs_list_report(&root).unwrap();
        let graphs = report["graphs"].as_array().unwrap();
        assert!(graphs.iter().any(|graph| graph["kind"] == "workflow_script"
            && graph["name"] == "odw-flow"
            && graph["features"]["fanout"] == true
            && graph["features"]["checkpoint"] == true
            && graph["features"]["schema"] == true
            && graph["valid"] == true));

        fs::create_dir_all(root.join(".odw/graphs")).unwrap();
        fs::write(
            root.join(".odw/graphs/custom.json"),
            r#"{"name":"custom","start":"a","end":"b","nodes":[],"edges":[]}"#,
        )
        .unwrap();
        let report = graphs_list_report(&root).unwrap();
        assert!(
            report["graphs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|graph| graph["kind"] == "graph_manifest"
                    && graph["name"] == "custom"
                    && graph["valid"] == true)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remove_short_workflow_name_deletes_command_and_script() {
        let root = temp_root("remove-workflow");
        fs::create_dir_all(&root).unwrap();
        init(&root, false, false).unwrap();
        remove_workflow(&root, "audit", false).unwrap();
        assert!(!root.join(".claude/workflows/odw-audit.js").exists());
        assert!(!root.join(".claude/commands/odw-audit.md").exists());
        assert!(root.join(".claude/workflows/odw-ship.js").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn frontmatter_value_reads_basic_fields() {
        let content = "---\nname: odw-test\ndescription: Example\n---\nBody";
        assert_eq!(
            frontmatter_value(content, "name").as_deref(),
            Some("odw-test")
        );
        assert_eq!(
            frontmatter_value(content, "description").as_deref(),
            Some("Example")
        );
    }

    #[test]
    fn capabilities_expose_lifecycle_boundaries() {
        let value = capabilities_json();
        assert_eq!(value["primary_user"], "Agent or CLI caller");
        assert_eq!(value["runtime"], "ODW direct JavaScript runner");
        // Assert the stable invariant, not exact prose: the bridge names pandacode
        // and never reintroduces the removed tmux / app-server surfaces.
        let bridge = value["agent_bridge"].as_str().unwrap_or_default();
        assert!(
            bridge.to_lowercase().contains("pandacode"),
            "agent_bridge should mention pandacode: {bridge}"
        );
        assert!(
            !bridge.contains("tmux") && !bridge.contains("app-server"),
            "agent_bridge must not carry removed tmux/app-server terms: {bridge}"
        );
        assert_eq!(
            value["lifecycle"]["exec"]["cli"],
            "odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>"
        );
        assert_eq!(
            value["lifecycle"]["pause_resume"]["note"],
            "Direct exec resumes from .odw/runs/<run_id>/state.json and skips completed node ids."
        );
        assert_eq!(
            value["composition"]["parallel"],
            "Dynamic Workflow-compatible parallel([() => agent(...), ...]) fan-out/join with max 16 concurrent thunks"
        );
    }

    #[test]
    fn spec_exposes_direct_script_contract() {
        let value = framework_spec_json();
        assert_eq!(value["name"], "Open Dynamic Workflow");
        assert_eq!(
            value["compatibility_target"]["runtime"],
            "ODW direct JavaScript runner"
        );
        assert_eq!(
            value["script_contract"]["phase"],
            "phase(title: string, detail?: string)"
        );
        assert_eq!(
            value["script_contract"]["codex"],
            "Route Codex through ordinary agent(prompt, { runtime: 'codex' }); agentType is optional metadata."
        );
        assert_eq!(
            value["script_contract"]["limits"]["max_concurrent_agents"],
            16
        );
    }

    #[test]
    fn direct_runner_exposes_observable_resume_helpers() {
        assert!(ODW_JS_RUNNER.contains("globalThis.pandacode"));
        assert!(ODW_JS_RUNNER.contains("runPandaCode"));
        assert!(ODW_JS_RUNNER.contains("globalThis.parallel"));
        assert!(ODW_JS_RUNNER.contains("globalThis.fanout"));
        assert!(ODW_JS_RUNNER.contains("globalThis.pipeline"));
        assert!(ODW_JS_RUNNER.contains("globalThis.log"));
        assert!(ODW_JS_RUNNER.contains("globalThis.promptSlot"));
        assert!(ODW_JS_RUNNER.contains("globalThis.args"));
        assert!(ODW_JS_RUNNER.contains("appendSchemaContract"));
        assert!(ODW_JS_RUNNER.contains("ODW final response contract"));
        assert!(ODW_JS_RUNNER.contains("resolveSchemaDescription"));
        // Built-in Workflow parity surface (guards against runtime regressions).
        assert!(ODW_JS_RUNNER.contains("getMaxConcurrency"));
        assert!(ODW_JS_RUNNER.contains("scriptDeterminismGuards"));
        assert!(ODW_JS_RUNNER.contains("createWorktree"));
        assert!(ODW_JS_RUNNER.contains("captureWorktreeChanges"));
        assert!(ODW_JS_RUNNER.contains("globalThis.workflow"));
        assert!(ODW_JS_RUNNER.contains("extractJsonObjectStrings"));
        assert!(ODW_JS_RUNNER.contains("agentCacheKey"));
        assert!(ODW_JS_RUNNER.contains("stableStringify"));
        assert!(ODW_JS_RUNNER.contains("extracted.prelude"));
        assert!(ODW_JS_RUNNER.contains("vm.createContext"));
        assert!(ODW_JS_RUNNER.contains("assertWorkflowSourceSafe"));
        assert!(ODW_JS_RUNNER.contains("validateNodeResult"));
        assert!(ODW_JS_RUNNER.contains("schemaMismatchResult"));
        assert!(ODW_JS_RUNNER.contains("extractStructuredCodexOutput"));
        assert!(ODW_JS_RUNNER.contains("parallel_start"));
        assert!(ODW_JS_RUNNER.contains("pipeline_start"));
        assert!(ODW_JS_RUNNER.contains("agent_schema_invalid"));
        assert!(ODW_JS_RUNNER.contains("globalThis.checkpoint"));
        assert!(ODW_JS_RUNNER.contains("agent_skip"));
        assert!(ODW_JS_RUNNER.contains("ODW_STATE_PATH"));
        assert!(ODW_JS_RUNNER.contains("activeAgents"));
    }

    #[test]
    fn run_journals_are_listed_shown_and_resumable() {
        let root = temp_root("run-journal");
        let run_dir = root.join(".odw/runs/odw-run-test");
        fs::create_dir_all(&run_dir).unwrap();
        let record = json!({
            "run_id": "odw-run-test",
            "status": "completed",
            "session_id": "session-abc",
            "run_dir": run_dir
        });
        fs::write(
            root.join(".odw/runs/latest.json"),
            serde_json::to_string_pretty(&record).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(".odw/runs/odw-run-test/run.json"),
            serde_json::to_string_pretty(&record).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(".odw/runs/odw-run-test/events.jsonl"),
            [
                r#"{"type":"launch"}"#,
                r#"{"raw":{"type":"codex_poll","key":"active-node","last_agent_message":"Older status"},"summary":"[event] codex_poll"}"#,
                r#"{"raw":{"type":"codex_poll","key":"active-node","last_agent_message":"Latest active status"},"summary":"[event] codex_poll"}"#,
                r#"{"type":"exit","status":"completed"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            root.join(".odw/runs/odw-run-test/state.json"),
            serde_json::to_string_pretty(&json!({
                "workflow": "test-flow",
                "activeAgents": {
                    "active-node": {
                        "key": "active-node",
                        "phase": "Implement",
                        "label": "active task",
                        "agentType": "odw-codex-coder",
                        "state": "running",
                        "attempt": 1,
                        "maxAttempts": 2
                    }
                },
                "agents": {},
                "failedAgents": {},
                "checkpoints": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let list = runs_list_report(&root).unwrap();
        assert_eq!(list["runs"].as_array().unwrap().len(), 1);
        let shown = runs_show_report(&root, "latest", 4).unwrap();
        assert_eq!(shown["events"].as_array().unwrap().len(), 4);
        assert_eq!(shown["progress"]["completed_agents"], 0);
        assert_eq!(
            shown["progress"]["active_agents"].as_array().unwrap().len(),
            1
        );
        assert!(format_runs_show_view(&shown).contains("last: Latest active status"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_agent_details_sort_by_index_then_key() {
        let mut agents = vec![
            json!({"key": "node-c", "index": 3}),
            json!({"key": "old-node-late", "ts": "2026-06-02T00:00:03Z"}),
            json!({"key": "old-node-early", "ts": "2026-06-02T00:00:01Z"}),
            json!({"key": "node-a", "index": 1}),
            json!({"key": "node-b", "index": 2}),
            json!({"key": "node-no-index"}),
        ];
        sort_agent_details(&mut agents);
        let keys = agents
            .iter()
            .map(|agent| json_string(agent, "key").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "node-a",
                "node-b",
                "node-c",
                "old-node-early",
                "old-node-late",
                "node-no-index"
            ]
        );
    }

    #[test]
    fn direct_exec_allows_schema_less_flexible_nodes() {
        if std::process::Command::new("node")
            .arg("--version")
            .status()
            .is_err()
        {
            return;
        }

        let root = temp_root("schema-less-exec");
        fs::create_dir_all(&root).unwrap();
        let script = root.join("flexible-workflow.js");
        fs::write(
            &script,
            r#"export const meta = { name: "flexible-workflow" };

phase("Custom", "No default schema or fixed node type");
const result = await agent("Do any custom node work.", {
  id: "freeform-node",
  label: "freeform node",
  phase: "Custom"
});
checkpoint("after-freeform", result);
return result;
"#,
        )
        .unwrap();

        exec_script(ExecArgs {
            path: root.clone(),
            script: Some(script),
            input: Some(r#"{"goal":"flexible"}"#.to_string()),
            input_file: None,
            resume: None,
            backend: "mock".to_string(),
            node_bin: "node".to_string(),
            provider: None,
            model: None,
            effort: "low".to_string(),
            timeout: "120".to_string(),
            codexctl_bin: "codexctl".to_string(),
            pandacode_bin: "pandacode".to_string(),
            json: false,
            dry_run: false,
            open: false,
            no_report: true,
        })
        .unwrap();

        let latest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join(".odw/runs/latest.json")).unwrap())
                .unwrap();
        let state_path =
            std::path::PathBuf::from(latest["run_dir"].as_str().unwrap()).join("state.json");
        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(state_path).unwrap()).unwrap();
        let node = &state["agents"]["freeform-node"];
        assert_eq!(node["schema"], serde_json::Value::Null);
        assert_eq!(node["agentType"], serde_json::Value::Null);
        assert_eq!(node["result"]["backend"], "mock");
        assert!(
            !node["result"]["prompt_preview"]
                .as_str()
                .unwrap()
                .contains("ODW final response contract")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_exec_blocks_workflow_imports() {
        if std::process::Command::new("node")
            .arg("--version")
            .status()
            .is_err()
        {
            return;
        }

        let root = temp_root("sandbox-exec");
        fs::create_dir_all(&root).unwrap();
        let script = root.join("unsafe-workflow.js");
        fs::write(
            &script,
            r#"import fs from "node:fs";

export const meta = { name: "unsafe-workflow" };
return { ok: true };
"#,
        )
        .unwrap();

        let result = exec_script(ExecArgs {
            path: root.clone(),
            script: Some(script),
            input: Some(r#"{"goal":"blocked"}"#.to_string()),
            input_file: None,
            resume: None,
            backend: "mock".to_string(),
            node_bin: "node".to_string(),
            provider: None,
            model: None,
            effort: "low".to_string(),
            timeout: "120".to_string(),
            codexctl_bin: "codexctl".to_string(),
            pandacode_bin: "pandacode".to_string(),
            json: false,
            dry_run: false,
            open: false,
            no_report: true,
        });
        assert!(result.is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_project_slug_matches_claude_code_layout() {
        assert_eq!(
            claude_project_slug(Path::new("/home/user/workspace/odw")),
            "-home-user-workspace-odw"
        );
    }

    #[test]
    fn evidence_reads_claude_workflow_artifacts() {
        let root = temp_root("evidence-project");
        let claude_projects = temp_root("evidence-claude");
        fs::create_dir_all(&root).unwrap();
        let slug = claude_project_slug(&root.canonicalize().unwrap());
        let workflow_dir = claude_projects
            .join(slug)
            .join("session-123")
            .join("workflows");
        fs::create_dir_all(&workflow_dir).unwrap();
        fs::write(
            workflow_dir.join("wf_test.json"),
            r#"{
              "runId": "wf_test",
              "workflowName": "odw-smoke",
              "status": "completed",
              "agentCount": 2,
              "phases": [{"title": "Research", "detail": "read files"}],
              "workflowProgress": [
                {
                  "type": "workflow_agent",
                  "index": 1,
                  "label": "research",
                  "phaseTitle": "Research",
                  "agentId": "agent-1",
                  "agentType": "odw-researcher",
                  "model": "claude-sonnet",
                  "state": "done",
                  "attempt": 1,
                  "toolCalls": 0,
                  "tokens": 10
                }
              ]
            }"#,
        )
        .unwrap();

        let report =
            workflow_evidence_report(&root, Some(&claude_projects), None, Some("wf_test")).unwrap();
        assert_eq!(report["ok"], true);
        assert_eq!(report["artifacts"][0]["run_id"], "wf_test");
        assert_eq!(
            report["artifacts"][0]["agents"][0]["agent_type"],
            "odw-researcher"
        );

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(claude_projects).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("odw-{name}-{stamp}"))
    }
}
