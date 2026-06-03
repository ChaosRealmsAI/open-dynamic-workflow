# PandaCode

PandaCode is an independent CLI for running coding tasks through multiple agent
runtimes with one command shape. The first version supports:

- `pandacode codex ...`: Codex through `codexctl session` app-server/control-plane
  commands.
- `pandacode claude ...`: Claude Code through a real `tmux` session.
- `pandacode bamboo ...`: Bamboo through its provider-native
  read/search/edit/write/bash coding loop for domestic OpenAI-compatible
  providers such as DeepSeek, Xiaomi/MiMo, Kimi, Zhipu, MiniMax, Qwen, and
  Stepfun.

PandaCode is not Open Dynamic Workflow. It is the lower-level executor that a
workflow system can call.

By default, task execution uses the strongest production profile:

- Claude: `opus` with `max` effort.
- Codex: `gpt-5.5` with `xhigh` effort, based on local `codexctl models`
  support.
- Bamboo: `deepseek` provider with Bamboo's provider default model and
  `high` reasoning effort. Use `--provider`, `--model`, and `--effort` to choose
  another domestic model.
- Permission mode: `max` by default. Use `--permission limited` for a lower-risk
  workspace-write mode.
- User settings and MCP: isolated off. Claude runs with inline PandaCode-owned
  hook settings and an empty strict MCP config.

## Quick Start For Agents

Use the top-level commands first. They are intentionally small and choose a
runtime automatically:

```bash
pandacode run --cd <workspace> --session <task-id> --task-file task.md --json
pandacode status --cd <workspace> --session <task-id> --json
pandacode resume --cd <workspace> --session <task-id> --task "continue and verify" --json
pandacode logs --cd <workspace> --session <task-id> --tail 200 --json
```

Runtime selection is progressive:

- Omit `--runtime` for `auto`.
- Pass `--runtime bamboo|claude|codex` when a caller already knows the backend.
- Pass `--provider deepseek` to select Bamboo from the top-level `run` command.
- Use `pandacode <runtime> ...` only when you need runtime-specific advanced
  controls such as Bamboo cache/cost/compact flags.

After any runtime saves a session, `pandacode status`, `logs`, `resume`,
`answer`, `artifacts`, `interrupt`, and `stop` can use the global latest session
without repeating the runtime name. Automation should still pass an explicit
`--session <task-id>` when running concurrent tasks.

`--task-file` paths are resolved from the current process directory first and
then from `--cd`, so callers can keep task files next to the target workspace or
address them relative to the workspace root.

Commands that receive `--json` also return a machine-readable failure envelope:
`{ "ok": false, "state": "failed", "error": ... }`. Callers do not need a
separate stderr parser for runtime errors such as a missing API key.

`exec` and `resume` return compact JSON summaries. They report the runtime
status, session ids, model/effort where available, local artifact paths, and
short redacted output tails without dumping full app-server or terminal event
streams. Bamboo also reports usage, cache, verification, and changed files. Use
`logs --json` when a caller needs a structured tail of the underlying runtime
output.

If a runtime asks for external input, `exec`/`resume` return
`state: "waiting_for_user"` with `pending_user_input` instead of treating the
turn as a failure. Use `pandacode <runtime> answer --choice N --wait` or
`--text ...` to continue the session. Claude answers the visible TUI prompt;
Codex delegates to `codexctl session answer`; Bamboo maps `answer` to a resume
turn that passes the selected/text answer back into the same Bamboo run history.

The stable high-level states are:

- `completed`: the turn ended successfully.
- `waiting_for_user`: the runtime is blocked on explicit external input.
- `idle`: the runtime is alive and ready for another turn.
- `running`: the runtime is still working.
- `timeout`: PandaCode did not observe completion or a blocking prompt in time.
- `stopped`: the delegated runtime/session is gone.
- `blocked` or `failed`: the task cannot continue without a new instruction or
  the runtime reported a hard failure.

## Command Shape

```bash
pandacode run --task "fix the failing tests" --cd .
pandacode resume --task "continue from the last result" --cd .
pandacode status --cd .
pandacode logs --cd . --json

pandacode <runtime> exec --task "fix the failing tests" --cd .
pandacode <runtime> resume --session latest --task "continue from the last result"
pandacode <runtime> answer --session latest --choice 2 --wait --cd .
pandacode <runtime> status --session latest --cd .
pandacode <runtime> logs --session latest --cd .
pandacode <runtime> artifacts --session latest --cd .
pandacode <runtime> interrupt --session latest --cd .
pandacode <runtime> stop --session latest --cd .
pandacode <runtime> model --session latest --model <model> --effort high --cd .
pandacode <runtime> models
pandacode <runtime> list --cd .
pandacode <runtime> doctor
```

`<runtime>` is currently `codex`, `claude`, or `bamboo`.

Task input can be provided three ways:

```bash
pandacode claude exec --task "create a small HTML demo"
pandacode codex exec --task-file task.md
pandacode bamboo exec --provider deepseek --model deepseek-v4-pro --effort high --task-file task.md
pandacode claude exec - < task.md
```

## Agent Integration Contract

Future workflow agents should treat `pandacode` as the only public interface.
Once the binary is installed and the requested backend is available on the
machine, an agent can run coding tasks without preparing Claude settings, MCP
files, hooks, or project configuration.

Human-facing top-level inspection commands such as `pandacode doctor`,
`pandacode models`, and `pandacode list` print compact summaries by default.
Pass `--json` whenever a caller needs the full machine-readable report.

Recommended agent loop:

```bash
pandacode doctor --cd <workspace> --json
pandacode models --cd <workspace> --json
pandacode run --cd <workspace> --session <name> --runtime auto --model <model> --effort <effort> --permission max --task-file task.md --json
pandacode status --cd <workspace> --session <name> --json
pandacode logs --cd <workspace> --session <name> --json
pandacode answer --cd <workspace> --session <name> --choice 1 --wait --json
pandacode resume --cd <workspace> --session <name> --task-file next.md --json
pandacode stop --cd <workspace> --session <name> --json
```

`pandacode doctor` is a per-runtime health report. Top-level `ok` means at least
one runtime is usable; inspect each runtime's `ok`, `missing`, and
`capabilities` fields before choosing a backend.

With `--runtime auto`, a known `--model` can select the matching backend:
domestic model ids such as `kimi-k2.6` select Bamboo and infer their provider,
Claude aliases such as `opus` select Claude, and `gpt-*` ids select Codex.
Use `--provider` only when the Bamboo provider cannot be inferred from the
model id or you want to override the default.

The same command shape applies to Codex and Bamboo. `answer --choice` maps to
`codexctl session answer --pick`; `logs --visible` remains Claude-only because
Codex and Bamboo have structured run snapshots rather than a terminal
viewport.

For Bamboo domestic-model runs, prefer the top-level command for normal use:

```bash
pandacode run --cd <workspace> \
  --session <task-id> \
  --provider deepseek \
  --model deepseek-v4-pro \
  --effort high \
  --permission max \
  --task-file task.md \
  --json
```

Use `pandacode bamboo exec --help` only for advanced cache, cost, compact,
verification, and provider-specific request parameters.

PandaCode owns the runtime glue internally:

- Claude hook settings are generated as inline JSON for each process.
- The hook command calls the installed `pandacode` binary through its current
  executable path.
- User and project Claude settings are not modified.
- User MCP configuration is ignored for executor sessions.
- Runtime state is written under `.pandacode/` so callers can observe and
  resume sessions.
- Orchestrators can set `PANDACODE_STATE_DIR` to isolate executor metadata from
  the target workspace state; relative values resolve under `--cd`.

## Runtime Mapping

Codex uses `codexctl session start/send/execute/read/watch/interrupt/stop/list`.
`exec` starts the app-server session and then calls `session execute`, because
`session start` is a Plan-mode turn and PandaCode is meant to be an executor.
The PandaCode session record stores the Codex `run_id`, `thread_id`, and local
log paths under `.pandacode/sessions/codex`. Each Codex session gets its own
control channel: logs are written under `.pandacode/codex/runs/<session>/logs`
and the codexctl daemon socket is a short per-session temp socket. This keeps
parallel workflow nodes from sharing one codexctl daemon/run namespace.
Long task prompts are transported by file reference: PandaCode stores the full
task under `.pandacode/<runtime>/prompts/` and sends a short instruction telling
the runtime to read that file. This avoids tmux paste limits and codexctl
app-server pipe/socket failures while preserving the original task text for
observability. Codex start also retries transient transport failures with a fresh
control socket.

Claude uses `tmux` to start interactive Claude Code and sends turns into that
session. Completion is detected with an explicit marker in the visible tmux
buffer. It does not use `claude -p`, `--output-format stream-json`, or
`--json-schema`.

PandaCode starts Claude with isolated local settings, inline PandaCode-owned hook
settings, and an empty strict MCP config so unrelated user hooks or MCP auth
prompts do not pollute executor sessions. Hook configuration is not written into
the target workspace; only runtime event/log artifacts are persisted.

When a Claude tmux session must be restarted, PandaCode resumes the saved Claude
conversation with official `claude --resume <session-id>` using the session id
captured from Claude hook events.

Bamboo is embedded as a native PandaCode runtime. It calls the selected
OpenAI-compatible provider directly, runs Bamboo's autonomous
read/search/edit/write/bash tool loop in-process, and stores artifacts under
`.pandacode/bamboo/runs`. The PandaCode session record stores the Bamboo
`run_id` so `pandacode bamboo resume` can rebuild the compact-aware resume
context from the previous report and event tail. Bamboo event logs are exposed
through `pandacode bamboo logs`; full reports keep provider/model settings,
usage, cache hit/miss, estimated cost, verification, and context compaction
metrics.

Bamboo provider selection:

```bash
pandacode bamboo exec \
  --provider deepseek \
  --model deepseek-v4-pro \
  --effort high \
  --task "fix the failing tests"

pandacode bamboo model \
  --session latest \
  --provider xiaomi \
  --model mimo-v2.5-pro \
  --effort high
```

Generation parameters are exposed on `bamboo exec`, `bamboo resume`, and
`bamboo model`:

```bash
pandacode bamboo exec \
  --provider kimi \
  --model kimi-k2.6 \
  --effort none \
  --thinking disabled \
  --max-tokens 2048 \
  --temperature 0.6 \
  --top-p 0.95 \
  --task "make the requested change"
```

Supported Bamboo native generation flags include `--thinking enabled|disabled`,
`--max-tokens`, `--temperature`, `--top-p`, `--presence-penalty`,
`--frequency-penalty`, repeated `--stop`, and repeated `--param KEY=JSON`.
`bamboo model` persists these settings for the next `resume` turn.

Autonomous-run controls from the original Bamboo executor are also exposed on
`bamboo exec` and `bamboo resume`:

```bash
pandacode bamboo exec \
  --provider deepseek \
  --model deepseek-v4-pro \
  --max-steps 100 \
  --model-timeout-ms 180000 \
  --run-timeout-ms 7200000 \
  --max-total-tokens 800000 \
  --max-cost 10 \
  --max-cost-currency cny \
  --price-file .pandacode/bamboo/pricing.cn.json \
  --verify "cargo test" \
  --cache-warm \
  --task-file task.md
```

Budget, verification, cache warmup, compact, and price-file controls are
reported through PandaCode capabilities. Bamboo currently implements them
natively; Claude and Codex report them as unsupported in `doctor`/`models`
instead of accepting and silently ignoring them.

Provider credentials are read from PandaCode/Bamboo environment variables. The
preferred generic variables are `PANDACODE_BAMBOO_PROVIDER`,
`PANDACODE_BAMBOO_API_KEY`, `PANDACODE_BAMBOO_MODEL`,
`PANDACODE_BAMBOO_BASE_URL`, and `PANDACODE_BAMBOO_PRICE_FILE`. Legacy
`BAMBOO_*` aliases still work. Provider-specific keys such as
`DEEPSEEK_API_KEY`, `XIAOMI_API_KEY`, `KIMI_API_KEY`, `ZHIPU_API_KEY`,
`MINIMAX_API_KEY`, `QWEN_API_KEY`, and `STEPFUN_API_KEY` are also supported.
Base URLs can be overridden with provider-specific variables such as
`DEEPSEEK_BASE_URL`.

Useful Bamboo docs:

- `docs/agent-caller-quickstart.md`
- `docs/bamboo-agent-caller-guide.md`
- `docs/bamboo-coding-tools.md`
- `docs/bamboo-cache-strategy.md`
- `docs/bamboo-migration-audit.md`
- `docs/bamboo-verification-matrix.md`

Live env and price examples:

```bash
mkdir -p .pandacode/bamboo
cp docs/bamboo-live.env.example .pandacode/bamboo/live.env
cp docs/bamboo-pricing.cn.json.example .pandacode/bamboo/pricing.cn.json
```

Live provider smoke:

```bash
scripts/smoke-bamboo-live-providers.sh
```

## Permission Mode

PandaCode exposes two permission modes and defaults to `max`.

- `max`: highest trusted automation mode. Claude maps this to interactive
  `--dangerously-skip-permissions`; Codex maps it to
  `--dangerously-full-access`; Bamboo uses the full autonomous tool loop with
  built-in protected-path and dangerous-command safety.
- `limited`: lower-risk workspace-write mode. Codex maps this to
  `--sandbox workspace-write --approval-policy never`; Claude maps this to
  `--permission-mode acceptEdits`; Bamboo allows normal workspace edits and
  local verification but blocks background commands, installs, downloads, and
  recursive/destructive cleanup.

## Local State

Runtime state is written inside the target workspace:

```text
.pandacode/
  sessions/
    claude/
    codex/
    bamboo/
  claude/
    events/
    logs/
    prompts/
  codex/
    runs/
    prompts/
  bamboo/
    events/
    prompts/
    runs/
```

This keeps workflow callers able to inspect session metadata, visible logs, and
runtime artifacts without parsing terminal history.

For workflow runners, CI, and other orchestrators that need per-run isolation,
set `PANDACODE_STATE_DIR` before launching PandaCode:

```bash
PANDACODE_STATE_DIR=.odw/runs/<run-id>/pandacode-state/bamboo/<session> \
  pandacode bamboo exec --cd <workspace> --session <session> --task-file task.md --json
```

PandaCode itself does not depend on Open Dynamic Workflow. ODW may call
PandaCode through this CLI/environment contract, but executor prompts, terminal
logs, provider reports, and resume records remain owned by PandaCode.

## Verification

The current test suite includes unit tests plus fake-runtime integration tests
for all runtime paths:

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
cargo build
```

The fake integrations exercise `exec`, `resume`, `status`, `logs`, `artifacts`,
`answer`, `model`, `interrupt`, `stop`, `list`, `models`, and `doctor` without
spending real model calls.
