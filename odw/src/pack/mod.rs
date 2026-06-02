pub(crate) struct PackFile {
    pub(crate) path: &'static str,
    pub(crate) content: &'static str,
}

pub(crate) const ODW_JS_RUNNER: &str = include_str!("templates/runtime/odw-js-runner.mjs");

// Execution-graph HTML report generator + its offline render assets (mermaid +
// marked), embedded so `odw report` can render without a network or `odw init`.
pub(crate) const ODW_REPORT_MJS: &str = include_str!("templates/report/odw-report.mjs");
pub(crate) const REPORT_MERMAID_JS: &str = include_str!("templates/report/vendor/mermaid.min.js");
pub(crate) const REPORT_MARKED_JS: &str = include_str!("templates/report/vendor/marked.min.js");

const ODW_TOML: &str = include_str!("templates/odw/odw.toml");
const ODW_README: &str = include_str!("templates/odw/README.md");
const ODW_RUNTIME_CONTRACT: &str = include_str!("templates/odw/framework/runtime-contract.md");
const ODW_WORKFLOW_API_DTS: &str = include_str!("templates/odw/framework/workflow-api.d.ts");

const AGENT_ORCHESTRATOR: &str = include_str!("templates/claude/agents/odw-orchestrator.md");
const AGENT_CODEX_CODER: &str = include_str!("templates/claude/agents/odw-codex-coder.md");
const AGENT_RESEARCHER: &str = include_str!("templates/claude/agents/odw-researcher.md");
const AGENT_SECURITY_REVIEWER: &str =
    include_str!("templates/claude/agents/odw-security-reviewer.md");
const AGENT_TEST_RUNNER: &str = include_str!("templates/claude/agents/odw-test-runner.md");
const AGENT_FAILURE_ANALYST: &str = include_str!("templates/claude/agents/odw-failure-analyst.md");
const AGENT_VERIFIER: &str = include_str!("templates/claude/agents/odw-verifier.md");
const AGENT_SYNTHESIZER: &str = include_str!("templates/claude/agents/odw-synthesizer.md");

// The agent-usable skill lives canonically at the repo root (open-source
// discoverability) and is embedded here so `odw init` installs it for an agent
// to auto-discover under `.claude/skills/`.
const SKILL_ODW: &str = include_str!("../../skills/odw/SKILL.md");

const COMMAND_FLOW: &str = include_str!("templates/claude/commands/odw.md");
const COMMAND_ODW_AUDIT: &str = include_str!("templates/claude/commands/odw-audit.md");
const COMMAND_ODW_SHIP: &str = include_str!("templates/claude/commands/odw-ship.md");
const COMMAND_ODW_FLOW: &str = include_str!("templates/claude/commands/odw-flow.md");

const WORKFLOW_AUTHORING_CONTRACT: &str =
    include_str!("templates/claude/workflows/odw-authoring-contract.md");
const WORKFLOW_AUDIT_JS: &str = include_str!("templates/claude/workflows/odw-audit.js");
const WORKFLOW_SHIP_JS: &str = include_str!("templates/claude/workflows/odw-ship.js");
const WORKFLOW_FLOW_JS: &str = include_str!("templates/claude/workflows/odw-flow.js");

const SETTINGS_EXAMPLE: &str = include_str!("templates/claude/settings.odw.example.json");

const SCHEMA_RESEARCH: &str = include_str!("templates/odw/schemas/research.schema.json");
const SCHEMA_SECURITY_FINDING: &str =
    include_str!("templates/odw/schemas/security-finding.schema.json");
const SCHEMA_CODEX_RESULT: &str = include_str!("templates/odw/schemas/codex-result.schema.json");
const SCHEMA_CODEX_PLAN: &str = include_str!("templates/odw/schemas/codex-plan.schema.json");
const SCHEMA_TEST_RESULT: &str = include_str!("templates/odw/schemas/test-result.schema.json");
const SCHEMA_ERROR_FEEDBACK: &str =
    include_str!("templates/odw/schemas/error-feedback.schema.json");
const SCHEMA_VERIFIER: &str = include_str!("templates/odw/schemas/verifier.schema.json");
const SCHEMA_SYNTHESIS: &str = include_str!("templates/odw/schemas/synthesis.schema.json");
const SCHEMA_TASK_PLAN: &str = include_str!("templates/odw/schemas/task-plan.schema.json");
const SCHEMA_TASK_JOIN: &str = include_str!("templates/odw/schemas/task-join.schema.json");
const SCHEMA_QUALITY_GATE: &str = include_str!("templates/odw/schemas/quality-gate.schema.json");
const SCHEMA_WORKFLOW_MANIFEST: &str =
    include_str!("templates/odw/schemas/workflow-manifest.schema.json");

pub(crate) fn files() -> &'static [PackFile] {
    &[
        PackFile {
            path: ".odw/odw.toml",
            content: ODW_TOML,
        },
        PackFile {
            path: ".odw/README.md",
            content: ODW_README,
        },
        PackFile {
            path: ".odw/framework/runtime-contract.md",
            content: ODW_RUNTIME_CONTRACT,
        },
        PackFile {
            path: ".odw/framework/workflow-api.d.ts",
            content: ODW_WORKFLOW_API_DTS,
        },
        PackFile {
            path: ".claude/agents/odw-orchestrator.md",
            content: AGENT_ORCHESTRATOR,
        },
        PackFile {
            path: ".claude/agents/odw-codex-coder.md",
            content: AGENT_CODEX_CODER,
        },
        PackFile {
            path: ".claude/agents/odw-researcher.md",
            content: AGENT_RESEARCHER,
        },
        PackFile {
            path: ".claude/agents/odw-security-reviewer.md",
            content: AGENT_SECURITY_REVIEWER,
        },
        PackFile {
            path: ".claude/agents/odw-test-runner.md",
            content: AGENT_TEST_RUNNER,
        },
        PackFile {
            path: ".claude/agents/odw-failure-analyst.md",
            content: AGENT_FAILURE_ANALYST,
        },
        PackFile {
            path: ".claude/agents/odw-verifier.md",
            content: AGENT_VERIFIER,
        },
        PackFile {
            path: ".claude/agents/odw-synthesizer.md",
            content: AGENT_SYNTHESIZER,
        },
        PackFile {
            path: ".claude/skills/odw/SKILL.md",
            content: SKILL_ODW,
        },
        PackFile {
            path: ".claude/commands/odw.md",
            content: COMMAND_FLOW,
        },
        PackFile {
            path: ".claude/commands/odw-audit.md",
            content: COMMAND_ODW_AUDIT,
        },
        PackFile {
            path: ".claude/commands/odw-ship.md",
            content: COMMAND_ODW_SHIP,
        },
        PackFile {
            path: ".claude/commands/odw-flow.md",
            content: COMMAND_ODW_FLOW,
        },
        PackFile {
            path: ".claude/workflows/odw-authoring-contract.md",
            content: WORKFLOW_AUTHORING_CONTRACT,
        },
        PackFile {
            path: ".claude/workflows/odw-audit.js",
            content: WORKFLOW_AUDIT_JS,
        },
        PackFile {
            path: ".claude/workflows/odw-ship.js",
            content: WORKFLOW_SHIP_JS,
        },
        PackFile {
            path: ".claude/workflows/odw-flow.js",
            content: WORKFLOW_FLOW_JS,
        },
        PackFile {
            path: ".claude/settings.odw.example.json",
            content: SETTINGS_EXAMPLE,
        },
        PackFile {
            path: ".odw/schemas/research.schema.json",
            content: SCHEMA_RESEARCH,
        },
        PackFile {
            path: ".odw/schemas/security-finding.schema.json",
            content: SCHEMA_SECURITY_FINDING,
        },
        PackFile {
            path: ".odw/schemas/codex-result.schema.json",
            content: SCHEMA_CODEX_RESULT,
        },
        PackFile {
            path: ".odw/schemas/codex-plan.schema.json",
            content: SCHEMA_CODEX_PLAN,
        },
        PackFile {
            path: ".odw/schemas/test-result.schema.json",
            content: SCHEMA_TEST_RESULT,
        },
        PackFile {
            path: ".odw/schemas/error-feedback.schema.json",
            content: SCHEMA_ERROR_FEEDBACK,
        },
        PackFile {
            path: ".odw/schemas/verifier.schema.json",
            content: SCHEMA_VERIFIER,
        },
        PackFile {
            path: ".odw/schemas/synthesis.schema.json",
            content: SCHEMA_SYNTHESIS,
        },
        PackFile {
            path: ".odw/schemas/task-plan.schema.json",
            content: SCHEMA_TASK_PLAN,
        },
        PackFile {
            path: ".odw/schemas/task-join.schema.json",
            content: SCHEMA_TASK_JOIN,
        },
        PackFile {
            path: ".odw/schemas/quality-gate.schema.json",
            content: SCHEMA_QUALITY_GATE,
        },
        PackFile {
            path: ".odw/schemas/workflow-manifest.schema.json",
            content: SCHEMA_WORKFLOW_MANIFEST,
        },
    ]
}

pub(crate) fn contract_text() -> &'static str {
    WORKFLOW_AUTHORING_CONTRACT
}
