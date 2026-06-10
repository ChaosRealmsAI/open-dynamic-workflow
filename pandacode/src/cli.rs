use std::{fmt, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::ProviderKind;

#[derive(Debug, Parser)]
#[command(
    name = "pandacode",
    version,
    about = "PandaCode: unified coding-task executor for Codex, Claude Code, and Bamboo runtimes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(hide = true, name = "claude-hook")]
    ClaudeHook(ClaudeHookArgs),
    #[command(
        name = "run",
        alias = "exec",
        about = "Run a coding task with automatic runtime selection"
    )]
    Run(AgentTaskCommandArgs),
    #[command(
        alias = "continue",
        about = "Resume the latest or selected PandaCode session"
    )]
    Resume(AgentTaskCommandArgs),
    #[command(about = "Answer a runtime prompt from the latest or selected session")]
    Answer(AgentAnswerCommandArgs),
    #[command(about = "Read status for the latest or selected session")]
    Status(AgentSessionCommandArgs),
    #[command(about = "Read logs for the latest or selected session")]
    Logs(AgentLogsCommandArgs),
    #[command(about = "Print artifacts for the latest or selected session")]
    Artifacts(AgentSessionCommandArgs),
    #[command(about = "Interrupt the latest or selected session")]
    Interrupt(AgentSessionCommandArgs),
    #[command(about = "Stop the latest or selected session")]
    Stop(AgentSessionCommandArgs),
    #[command(
        about = "Wait until the given sessions settle; succeed only when every session completes and every expected artifact exists"
    )]
    Wait(WaitCommandArgs),
    #[command(
        about = "Reclaim disk: prune PandaCode-owned prompts/logs/events/detached files older than --days (session records and Codex home are never touched)"
    )]
    Gc(GcCommandArgs),
    #[command(about = "Check runtimes and required local binaries")]
    Doctor(GlobalArgs),
    #[command(about = "List known PandaCode sessions for all runtimes")]
    List(GlobalArgs),
    #[command(about = "List models for all runtimes")]
    Models(GlobalArgs),
    #[command(
        subcommand,
        about = "Run tasks directly through codex app-server (stdio JSON-RPC, no daemon)",
        after_help = "Examples:\n  pandacode codex exec --task \"fix the failing tests\" --cd .\n  pandacode codex exec --detach --session build --task-file task.md   # background turn\n  pandacode codex status --session build        # live watch (state + last agent message)\n  pandacode codex answer --session build --choice 2   # answer a pending question\n  pandacode codex interrupt --session build     # abort the active turn\n  pandacode codex exec --auth-home ~/.codex-work --task \"...\"   # another account's auth, clean config\n  pandacode codex logs --session build --visible   # structured thread history\n  pandacode codex doctor                        # health + account + rate limits"
    )]
    Codex(RuntimeCommand),
    #[command(subcommand, about = "Run tasks through Claude Code in tmux")]
    Claude(RuntimeCommand),
    #[command(
        subcommand,
        name = "bamboo",
        about = "Run tasks through Bamboo's provider-native coding agent"
    )]
    Bamboo(Box<BambooRuntimeCommand>),
}

#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    #[command(about = "Execute a coding task and wait for the runtime to pause or complete")]
    Exec(TaskCommandArgs),
    #[command(about = "Resume an existing session with a continuation task")]
    Resume(TaskCommandArgs),
    #[command(about = "Answer a runtime prompt that is waiting for user input")]
    Answer(AnswerCommandArgs),
    #[command(about = "Read current session status")]
    Status(SessionCommandArgs),
    #[command(about = "Read current session logs or visible output")]
    Logs(LogsCommandArgs),
    #[command(about = "Print known local artifact paths for a session")]
    Artifacts(SessionCommandArgs),
    #[command(about = "Set the model/effort used by the next turn for a session")]
    Model(ModelCommandArgs),
    #[command(about = "List available models for this runtime")]
    Models(RuntimeGlobalArgs),
    #[command(about = "Interrupt the active turn without removing the session")]
    Interrupt(SessionCommandArgs),
    #[command(about = "Stop the session and release its runtime process")]
    Stop(SessionCommandArgs),
    #[command(about = "List sessions for this runtime")]
    List(RuntimeGlobalArgs),
    #[command(about = "Check this runtime and required local binaries")]
    Doctor(RuntimeGlobalArgs),
}

#[derive(Debug, Subcommand)]
pub enum BambooRuntimeCommand {
    #[command(about = "Execute a coding task and wait for the runtime to pause or complete")]
    Exec(BambooTaskCommandArgs),
    #[command(about = "Resume an existing session with a continuation task")]
    Resume(BambooTaskCommandArgs),
    #[command(about = "Answer a runtime prompt that is waiting for user input")]
    Answer(AnswerCommandArgs),
    #[command(about = "Read current session status")]
    Status(SessionCommandArgs),
    #[command(about = "Read current session logs or visible output")]
    Logs(LogsCommandArgs),
    #[command(about = "Print known local artifact paths for a session")]
    Artifacts(SessionCommandArgs),
    #[command(about = "Set the model/effort used by the next turn for a session")]
    Model(BambooModelCommandArgs),
    #[command(about = "List available models for this runtime")]
    Models(BambooRuntimeGlobalArgs),
    #[command(about = "Interrupt the active turn without removing the session")]
    Interrupt(SessionCommandArgs),
    #[command(about = "Stop the session and release its runtime process")]
    Stop(SessionCommandArgs),
    #[command(about = "List sessions for this runtime")]
    List(BambooRuntimeGlobalArgs),
    #[command(about = "Check this runtime and required local binaries")]
    Doctor(BambooRuntimeGlobalArgs),
}

#[derive(Debug, Args, Clone)]
pub struct ClaudeHookArgs {
    #[arg(long)]
    pub event_log: PathBuf,
    #[arg(long)]
    pub kind: String,
}

#[derive(Debug, Args, Clone)]
pub struct AnswerCommandArgs {
    #[arg(long, default_value = "latest", help = "Session id, or latest")]
    pub session: String,
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(
        long,
        conflicts_with = "text",
        help = "Choose a structured option by 1-based index"
    )]
    pub choice: Option<usize>,
    #[arg(
        long,
        conflicts_with = "choice",
        alias = "answer",
        help = "Paste a text answer into the active prompt"
    )]
    pub text: Option<String>,
    #[arg(long, help = "Wait for the runtime to continue after answering")]
    pub wait: bool,
    #[arg(long, help = "Wait timeout in milliseconds")]
    pub timeout_ms: Option<u64>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct GlobalArgs {
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct RuntimeGlobalArgs {
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct WaitCommandArgs {
    #[arg(
        long = "session",
        required = true,
        value_name = "SESSION",
        help = "Session id to wait for; repeat for multiple lanes"
    )]
    pub sessions: Vec<String>,
    #[arg(long, default_value = ".", help = "Workspace directory")]
    pub cd: PathBuf,
    #[arg(
        long,
        default_value_t = 1_800_000,
        help = "Overall wait timeout in milliseconds"
    )]
    pub timeout_ms: u64,
    #[arg(long, default_value_t = 5_000, help = "Poll interval in milliseconds")]
    pub interval_ms: u64,
    #[arg(
        long = "expect-artifact",
        value_name = "PATH",
        help = "File that must exist (relative to --cd) for the wait to succeed; repeat for multiple files"
    )]
    pub expect_artifact: Vec<PathBuf>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Args, Clone)]
pub struct GcCommandArgs {
    #[arg(long, default_value = ".", help = "Workspace directory")]
    pub cd: PathBuf,
    #[arg(
        long,
        default_value_t = 7,
        help = "Delete PandaCode-owned prompt/log/event/detached files older than this many days"
    )]
    pub days: u64,
    #[arg(
        long,
        help = "Report what would be deleted without removing anything"
    )]
    pub dry_run: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Args, Clone)]
pub struct AgentTaskCommandArgs {
    #[command(flatten)]
    pub common: TaskCommandArgs,
    #[arg(
        long,
        value_enum,
        default_value_t = RuntimeSelector::Auto,
        help = "Runtime to use; auto selects from model/provider hints"
    )]
    pub runtime: RuntimeSelector,
    #[arg(
        long,
        help = "Provider hint for Bamboo; selects Bamboo when --runtime auto"
    )]
    pub provider: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct AgentSessionCommandArgs {
    #[command(flatten)]
    pub common: SessionCommandArgs,
    #[arg(long, value_enum, default_value_t = RuntimeSelector::Auto, help = "Runtime to inspect")]
    pub runtime: RuntimeSelector,
}

#[derive(Debug, Args, Clone)]
pub struct AgentLogsCommandArgs {
    #[command(flatten)]
    pub common: LogsCommandArgs,
    #[arg(long, value_enum, default_value_t = RuntimeSelector::Auto, help = "Runtime to inspect")]
    pub runtime: RuntimeSelector,
}

#[derive(Debug, Args, Clone)]
pub struct AgentAnswerCommandArgs {
    #[command(flatten)]
    pub common: AnswerCommandArgs,
    #[arg(long, value_enum, default_value_t = RuntimeSelector::Auto, help = "Runtime to answer")]
    pub runtime: RuntimeSelector,
}

#[derive(Debug, Args, Clone)]
pub struct TaskCommandArgs {
    #[arg(
        value_name = "-",
        help = "Read task from stdin when this positional is '-'"
    )]
    pub stdin: Option<String>,
    #[arg(long, conflicts_with = "task_file", help = "Inline task text")]
    pub task: Option<String>,
    #[arg(long, value_name = "PATH", help = "Read task text from a file")]
    pub task_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "TEXT|builtin:NAME|@FILE|file:PATH|text:TEXT",
        help = "Append a prompt part after the task; repeat for multiple ordered parts. All runtimes resolve builtin:NAME (embedded role prompts), @FILE, file:PATH, and text:TEXT locally"
    )]
    pub prompt_append: Vec<String>,
    #[arg(
        long,
        help = "Codex/Claude: return immediately and run the turn in a detached background worker; observe with status, block with `pandacode wait`, end with stop"
    )]
    pub detach: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Require this file to exist (relative to --cd) after a completed turn, otherwise the state becomes no_report; repeat for multiple files"
    )]
    pub expect_artifact: Vec<PathBuf>,
    #[arg(
        long,
        help = "Codex only: set a thread goal/objective before the turn starts"
    )]
    pub objective: Option<String>,
    #[arg(long, default_value = ".", help = "Workspace directory")]
    pub cd: PathBuf,
    #[arg(long, default_value = "latest", help = "Session id, or latest")]
    pub session: String,
    #[arg(long, help = "Model id for this turn")]
    pub model: Option<String>,
    #[arg(long, help = "Reasoning/effort level when supported")]
    pub effort: Option<Effort>,
    #[arg(
        long,
        value_enum,
        help = "Agent permission mode. New sessions default to max; resume inherits the stored mode unless set"
    )]
    pub permission: Option<PermissionMode>,
    #[arg(long, help = "Wait timeout in milliseconds")]
    pub timeout_ms: Option<u64>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct BambooTaskCommandArgs {
    #[command(flatten)]
    pub common: TaskCommandArgs,
    #[arg(long, help = "Provider to call, for example deepseek")]
    pub provider: Option<String>,
    #[command(flatten)]
    pub generation: BambooGenerationArgs,
    #[command(flatten)]
    pub run: BambooRunArgs,
}

#[derive(Debug, Args, Clone)]
pub struct SessionCommandArgs {
    #[arg(long, default_value = "latest", help = "Session id, or latest")]
    pub session: String,
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct LogsCommandArgs {
    #[arg(long, default_value = "latest", help = "Session id, or latest")]
    pub session: String,
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(long, default_value_t = 100, help = "Number of log lines to show")]
    pub tail: usize,
    #[arg(
        long,
        hide = true,
        help = "Claude only: capture the final visible viewport instead of scrollback tail"
    )]
    pub visible: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct ModelCommandArgs {
    #[arg(long, default_value = "latest", help = "Session id, or latest")]
    pub session: String,
    #[arg(
        long,
        default_value = ".",
        help = "Workspace directory for session state"
    )]
    pub cd: PathBuf,
    #[arg(
        long = "model",
        alias = "set",
        value_name = "MODEL",
        help = "Model id for the next turn"
    )]
    pub model: String,
    #[arg(long, help = "Reasoning/effort level for the next turn when supported")]
    pub effort: Option<Effort>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[command(flatten)]
    pub bins: RuntimeBins,
}

#[derive(Debug, Args, Clone)]
pub struct BambooModelCommandArgs {
    #[command(flatten)]
    pub common: ModelCommandArgs,
    #[arg(long, help = "Provider to apply on the next turn")]
    pub provider: Option<String>,
    #[command(flatten)]
    pub generation: BambooGenerationArgs,
}

#[derive(Debug, Args, Clone)]
pub struct BambooRuntimeGlobalArgs {
    #[command(flatten)]
    pub common: RuntimeGlobalArgs,
    #[arg(long, help = "Provider to inspect, for example deepseek")]
    pub provider: Option<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct BambooGenerationArgs {
    #[arg(
        long,
        help = "Enable or disable provider thinking when the runtime supports it"
    )]
    pub thinking: Option<ThinkingMode>,
    #[arg(
        long,
        value_name = "TOKENS",
        help = "Provider max output tokens when the runtime supports it"
    )]
    pub max_tokens: Option<u32>,
    #[arg(long, help = "Sampling temperature when the runtime supports it")]
    pub temperature: Option<f32>,
    #[arg(long, help = "Top-p sampling when the runtime supports it")]
    pub top_p: Option<f32>,
    #[arg(long, help = "Presence penalty when the runtime supports it")]
    pub presence_penalty: Option<f32>,
    #[arg(long, help = "Frequency penalty when the runtime supports it")]
    pub frequency_penalty: Option<f32>,
    #[arg(long = "stop", value_name = "TEXT", help = "Stop sequence; repeatable")]
    pub stop: Vec<String>,
    #[arg(
        long = "param",
        value_name = "KEY=JSON",
        help = "Merge a provider-specific JSON field into the request body; repeatable"
    )]
    pub param: Vec<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct BambooRunArgs {
    #[arg(
        long,
        help = "Maximum model/tool loop steps when the runtime supports it"
    )]
    pub max_steps: Option<usize>,
    #[arg(
        long,
        help = "Default shell command timeout when the runtime supports it"
    )]
    pub shell_timeout_ms: Option<u64>,
    #[arg(long, help = "Per-model-call timeout when the runtime supports it")]
    pub model_timeout_ms: Option<u64>,
    #[arg(
        long,
        help = "Total run timeout; overrides --timeout-ms when supported"
    )]
    pub run_timeout_ms: Option<u64>,
    #[arg(
        long,
        help = "Keep this many recent dynamic messages before compacting when supported"
    )]
    pub history_keep_last: Option<usize>,
    #[arg(
        long,
        help = "Estimated context-token threshold for auto compact when supported"
    )]
    pub compact_threshold_tokens: Option<u64>,
    #[arg(long, help = "Output headroom reserved during compact when supported")]
    pub compact_reserve_tokens: Option<u64>,
    #[arg(long, help = "Maximum input tokens when the runtime supports it")]
    pub max_input_tokens: Option<u64>,
    #[arg(long, help = "Maximum output tokens when the runtime supports it")]
    pub max_output_tokens: Option<u64>,
    #[arg(long, help = "Maximum total tokens when the runtime supports it")]
    pub max_total_tokens: Option<u64>,
    #[arg(long, help = "Maximum estimated cost when the runtime supports it")]
    pub max_cost: Option<f64>,
    #[arg(
        long,
        default_value = "native",
        help = "Native, USD, CNY, or a converted currency"
    )]
    pub max_cost_currency: String,
    #[arg(
        long,
        value_name = "PATH",
        help = "Cache-aware provider price table when the runtime supports it"
    )]
    pub price_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Verification command to run after finish; repeatable when supported"
    )]
    pub verify: Vec<String>,
    #[arg(
        long,
        help = "Infer common verification commands from the workspace when supported"
    )]
    pub auto_verify: bool,
    #[arg(
        long,
        help = "Perform provider cache warmup before the task when supported"
    )]
    pub cache_warm: bool,
    #[arg(
        long,
        default_value_t = 2,
        help = "Warmup request count when --cache-warm is set"
    )]
    pub cache_warm_rounds: usize,
    #[arg(long, help = "Inline stable cache prefix when supported")]
    pub cache_prefix: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Stable cache prefix file; repeatable when supported"
    )]
    pub cache_prefix_file: Vec<PathBuf>,
    #[arg(long, help = "Explicit provider cache key when supported")]
    pub cache_key: Option<String>,
    #[arg(long, help = "Provider cache retention hint when supported")]
    pub cache_retention: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct RuntimeBins {
    #[arg(long, hide = true, default_value = "codex")]
    pub codex_bin: String,
    #[arg(long, hide = true, default_value = "claude")]
    pub claude_bin: String,
    #[arg(long, hide = true, default_value = "tmux")]
    pub tmux_bin: String,
    #[arg(long, hide = true, default_value = "summary")]
    pub log_mode: String,
    #[arg(
        long,
        alias = "codex-auth-home",
        value_name = "DIR",
        conflicts_with = "codex_home",
        help = "Codex account switch: copy auth material from this Codex home (e.g. ~/.codex-work) into PandaCode's managed clean home; that home's config/AGENTS.md/skills are NOT loaded"
    )]
    pub auth_home: Option<PathBuf>,
    #[arg(
        long,
        value_name = "DIR",
        help = "Use this full Codex home as-is (loads its config, rules, and session storage). Prefer --auth-home for plain account switching"
    )]
    pub codex_home: Option<PathBuf>,
}

impl Default for RuntimeBins {
    fn default() -> Self {
        Self {
            codex_bin: "codex".to_string(),
            claude_bin: "claude".to_string(),
            tmux_bin: "tmux".to_string(),
            log_mode: "summary".to_string(),
            auth_home: None,
            codex_home: None,
        }
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct ProviderOverrides {
    #[arg(long, global = true, value_enum)]
    pub provider: Option<ProviderKind>,
    #[arg(long, global = true, value_name = "URL")]
    pub base_url: Option<String>,
    #[arg(long, global = true, value_name = "KEY")]
    pub api_key: Option<String>,
    #[arg(long, global = true, value_name = "MODEL")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Raw,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ThinkingMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RuntimeSelector {
    Auto,
    Bamboo,
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PermissionMode {
    Max,
    Limited,
}

impl PermissionMode {
    pub fn as_value(self) -> &'static str {
        match self {
            PermissionMode::Max => "max",
            PermissionMode::Limited => "limited",
        }
    }

    pub fn from_record(value: Option<&str>) -> Self {
        match value {
            Some("limited") => PermissionMode::Limited,
            _ => PermissionMode::Max,
        }
    }
}

impl fmt::Display for RuntimeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeSelector::Auto => write!(f, "auto"),
            RuntimeSelector::Bamboo => write!(f, "bamboo"),
            RuntimeSelector::Claude => write!(f, "claude"),
            RuntimeSelector::Codex => write!(f, "codex"),
        }
    }
}

impl ThinkingMode {
    pub fn as_api_value(self) -> &'static str {
        match self {
            ThinkingMode::Enabled => "enabled",
            ThinkingMode::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    Max,
    Xhigh,
}

#[allow(dead_code)]
impl ReasoningEffort {
    pub fn as_api_value(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Max => "max",
            ReasoningEffort::Xhigh => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_value(self) -> &'static str {
        match self {
            Effort::None => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}
