# Open Dynamic Workflow

Open Dynamic Workflow (`odw`) is a script-driven workflow runner for
agent-authored JavaScript workflows.

An agent can write a workflow script, then run it directly:

```bash
odw exec --script .claude/workflows/odw-flow.js --input-file workflow-input.json --backend pandacode
odw exec --resume latest
odw runs show latest
```

`workflow-input.json` should contain the goal and `prompts` object for the
slots declared by the selected workflow. Mock runs may use suggested prompt text
from the starter scripts.

The JavaScript workflow owns phases, branching, fan-out, intermediate results,
checkpoints, and synthesis. ODW provides the direct runner, node logs, resume
state, schema validation, and a PandaCode bridge for Claude/tmux plus
Codex/appserver executor nodes.

## Boundary

```text
Agent or CLI caller
  -> odw exec --script <workflow.js>
  -> .claude/workflows/*.js
  -> phase(...)
  -> agent(prompt, { label, phase, runtime, provider?, model?, schema?, schemaDescription?, agentType? })
  -> .odw/runs/<run_id>/events.jsonl + state.json
  -> pandacode claude|codex|bamboo exec
```

Claude Code remains an optional caller and compatibility surface through `/odw`
and `/workflows`. It is not required for the core path. Worker failures are also
part of the contract: failed workers return structured error feedback instead of
unclassified prose.

ODW is a pure orchestration runtime: the only executor backend is `pandacode`
(plus `mock` for token-free smoke tests). All Codex/Claude/Bamboo execution —
including persistent Claude terminals and domestic-model Bamboo runs — is owned
by PandaCode; ODW just dispatches `agent(prompt, { runtime, provider, model })`
nodes to `pandacode <runtime> exec` (single-shot).

## Install

Pick whichever fits your situation:

**One-shot (odw + pandacode together):** `./install.sh` builds and installs
both — odw (this repo) and the `pandacode` executor it dispatches to (found
next to this repo, on `PATH`, or via `PANDACODE_DIR` / `PANDACODE_REPO`).

Or install each piece yourself:


```bash
cargo install --path .          # put `odw` on PATH (recommended)
cargo build --release           # or just build: ./target/release/odw
```

Then scaffold a project and check the executor wiring:

```bash
odw init --path /path/to/project      # writes the pack (below)
odw validate --path /path/to/project  # asserts the pack is intact
odw doctor                            # checks runtimes + binaries (exits non-zero if unhealthy)
export ODW_PANDACODE_BIN=/path/to/pandacode   # if `pandacode` is not on PATH
```

`odw init` writes:

```text
.odw/odw.toml  .odw/README.md  .odw/bin/odw  .odw/runs/
.odw/framework/runtime-contract.md  .odw/framework/workflow-api.d.ts
.odw/schemas/*.schema.json
.claude/skills/odw/SKILL.md          # agent-usable skill — pick up and go
.claude/agents/odw-*.md  .claude/commands/odw*.md
.claude/workflows/odw-authoring-contract.md
.claude/workflows/odw-{audit,ship,flow}.js
.claude/settings.odw.example.json
```

**For agents:** after `odw init`, a Claude Code skill is installed at
`.claude/skills/odw/SKILL.md` (canonical copy: `skills/odw/` in this repo). Load
it and you have install + authoring + run instructions in one place — no need to
read the source.

## Direct Usage

```js
export const meta = { name: "ship-feature" };

phase("Implement", "Codex implements the change");
const implPrompt = promptSlot("implement", {
  input: args,
  required_schema: ".odw/schemas/codex-result.schema.json"
});
const result = await agent(implPrompt, {
  label: "codex-implement",
  phase: "Implement",
  runtime: "codex",
  agentType: "odw-codex-coder",
  schema: ".odw/schemas/codex-result.schema.json",
  schemaDescription: "Final response is the implementation result: changed files and verification evidence."
});

checkpoint("after-implement", { ok: result.ok });
return { ok: result.ok !== false };
```

```bash
odw exec --script ./ship-feature.js --input-file workflow-input.json --backend pandacode
odw runs show latest
odw exec --resume latest
```

## Execution graph report

After writing a workflow, generate a self-contained HTML execution graph:

```bash
odw report --script ./ship-feature.js --open
odw report --run latest --open
```

`odw report --script` performs a token-free mock dry run, reads the emitted event
stream, and derives the graph automatically. `odw report --run <id>` renders an
existing mock or real run. Use `--input` for the mock run payload and `--out` to
choose the HTML path.

The report shows a Mermaid execution graph on the left, coloured by runtime
(`codex`, `claude`, `bamboo`), and node details on the right: model, prompt,
status, tokens, and duration.

Node-level fan-out uses the runner API, not ad hoc orchestration:

```js
const reviews = await parallel(batches.map((batch, index) => () =>
  agent(promptSlot("review_batch", {
    batch,
    index,
    required_schema: ".odw/schemas/security-finding.schema.json"
  }, `
Role:
Read-only evidence-backed reviewer.

Input batch:
{{context}}

Task:
Review this batch and return only evidence-backed findings.

Constraints:
Do not edit files. Cite exact evidence.

Output schema:
.odw/schemas/security-finding.schema.json

Done criteria:
The batch is either clean or every finding has file evidence.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json.
`), {
    id: `review-${index}`,
    label: `review ${batch.name}`,
    phase: "Review",
    agentType: "odw-security-reviewer",
    schema: ".odw/schemas/security-finding.schema.json",
    schemaDescription: "Final response is the evidence-backed review result for this one batch.",
    retry: { maxAttempts: 2 }
  })
));
```

## Claude Usage

Inside Claude Code:

```text
/odw-audit src/routes for missing auth checks
/odw-ship implement the agreed billing permission fix
/odw-flow decompose this feature into parallel Codex tasks
```

Claude should load `.odw/framework/workflow-api.d.ts` and write or adapt a
workflow with this shape:

```js
phase("Research", "read files");
const result = await agent(prompt, {
  label: "research",
  phase: "Research",
  agentType: "odw-researcher"
});
```

For workflow-node parallelism, use `parallel([() => agent(...), ...])`.
ODW emits parallel group start/done events, caps concurrency at 16, preserves
result order, and joins before returning. For item streams, use
`pipeline(items, ...stages)`. For dynamic task decomposition, let an upstream
node return a task array, then map it with `fanout(tasks, (task) =>
agent(...))`.

## Core Concepts

- `workflow`: a JavaScript module that an agent writes or loads, with
  `export const meta` followed by top-level async workflow code. Input is
  available as `args`. Workflow scripts are sandboxed orchestration code; file,
  shell, and code-edit work must go through `agent(...)` executor nodes.
- `phase`: a named stage emitted to live logs and `.odw/runs/*/events.jsonl`.
- `agent`: a node invocation. The aligned call shape is
  `agent(prompt, { label, phase, runtime, model, schema, schemaDescription, retry, agentType })`.
  ODW does not create default nodes or require fixed agent types; each call in
  workflow code is the node, and `agentType` is only an optional author-defined
  routing/tag value.
- `promptSlot`: workflow scripts declare prompt slots; real runs inject
  `input.prompts.<slot>`. Suggested template literals are for mock smoke tests
  or explicit caller opt-in.
- `pandacode`: convenience namespace — `pandacode.codex(prompt)`,
  `pandacode.claude(prompt)`, `pandacode.bamboo(prompt, { provider })`,
  `pandacode.exec(runtime, prompt)`; each is just `agent(prompt, { runtime })`.
- `runtime`: with `--backend pandacode`, ordinary `agent(...)` nodes dispatch
  to `runtime: "claude"`, `runtime: "codex"`, or `runtime: "bamboo"` while the
  workflow remains the caller and orchestrator. Bamboo nodes (domestic models)
  require `provider` (or `bambooProvider`) such as `deepseek`, `xiaomi`, `kimi`,
  `zhipu`, `minimax`, `qwen`, or `stepfun`; provider is invalid on non-Bamboo
  runtimes. Enable a provider by setting its API key in the environment — the
  provider-specific var (`DEEPSEEK_API_KEY`, `KIMI_API_KEY`, `QWEN_API_KEY`,
  `ZHIPU_API_KEY`, `MINIMAX_API_KEY`, `XIAOMI_API_KEY`, `STEPFUN_API_KEY`) or the
  generic `PANDACODE_BAMBOO_API_KEY`. Without a key the node returns a structured
  `missing API key` failure; `odw doctor` shows whether bamboo is configured.
- `checkpoint`: persists resume state and emits a checkpoint event.
- `pipeline`: `pipeline(items, ...stages)` runs each item through sequential
  stages while items fan out.
- `parallel`: a workflow node-level fan-out/join via
  `parallel([() => agent(...), ...])`; keep concurrency at or below 16.
- `fanout`: dynamic node fan-out from structured upstream output via
  `fanout(items, (item, index) => agent(...))`.
- `schema`: schema use is opt-in. If a workflow author passes
  `agent(..., { schema, schemaDescription, retry })`, ODW appends the full JSON
  Schema as a final-response-only contract, validates that final response,
  injects schema mismatch context into the same node prompt, and retries before
  returning structured `schema_mismatch` feedback. If `schema` is omitted, ODW
  applies no default schema.
- `error feedback`: `.odw/schemas/error-feedback.schema.json` is the standard
  result when a worker, command, schema, or CodexCTL step fails.

`odw-orchestrator` plans and routes. The workflow script owns executable
branching, fan-out, loops, intermediate state, and final aggregation.

## Lifecycle

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- optional Claude run: `/odw`, `/odw-audit`, `/odw-ship`, `/odw-flow`
- watch: `odw runs show latest`; optional Claude watch: `/workflows`
- pause/resume: `odw exec --resume latest`
- stop: stop the invoking process, or use `/workflows` for Claude-launched runs
- restart node: direct exec resumes by the stable `prompt + options` cache key;
  completed nodes are skipped from state
- remove saved template: `odw workflows remove <name>`
- read artifact evidence: `odw evidence --path .`
- live logs: `odw exec` streams node progress
- local journals: `odw runs list` and `odw runs show latest`

## Agent Types

- `odw-orchestrator`: plans workflows and routes work
- `odw-researcher`: read-only repository discovery
- `odw-security-reviewer`: evidence-backed security review
- `odw-codex-coder`: implements scoped code edits single-shot via the PandaCode
  Codex executor (`runtime: "codex"`)
- `odw-test-runner`: scoped command verification
- `odw-failure-analyst`: structured retry/blocker feedback for failed workers
- `odw-verifier`: adversarial claim validation
- `odw-synthesizer`: final concise synthesis

Subagent model and tool selection remain Claude Code-native through each
`.claude/agents/*.md` frontmatter.

## CLI

```bash
odw init --path .
odw doctor --path .
odw validate --path .
odw spec
odw capabilities
odw exec --script .claude/workflows/odw-audit.js --input '{"goal":"README smoke"}' --backend mock
odw exec --script .claude/workflows/odw-flow.js --input '{"goal":"complex flow smoke"}' --backend mock
odw exec --resume latest
odw evidence --path .
odw runs list --path .
odw runs show latest --path .
odw agents list --path .
odw agents list --built-in
odw workflows list --path .
odw workflows remove odw-audit --path . --dry-run
odw contract
```

`odw exec` is the direct runner. It streams workflow, phase, node, checkpoint,
error, and exit events, writes `.odw/runs/<run_id>/events.jsonl`, and persists
node resume state in `.odw/runs/<run_id>/state.json`. The only executor backend
is `pandacode` (plus `mock` for token-free smoke tests).

## Codex

Codex runs through PandaCode, single-shot. A node with
`agent(prompt, { runtime: "codex" })` dispatches to `pandacode codex exec`, which
runs to completion. PandaCode owns Codex account handling, logs, and model
discovery; the workflow script stays the orchestrator.

```js
const result = await agent(implementationPrompt, {
  runtime: "codex",
  agentType: "odw-codex-coder",
  schema: ".odw/schemas/codex-result.schema.json"
});
```

If the executor fails, workers return `.odw/schemas/error-feedback.schema.json`
with a classified error (category, retryability, next action) instead of burying
the failure in prose.

## Bamboo

Bamboo domestic-model runs also go through PandaCode. ODW only dispatches the
node; PandaCode owns provider credentials, model selection, logs, token usage,
and execution.

```js
const result = await agent(prompt, {
  runtime: "bamboo",
  provider: "deepseek"
});

const same = await pandacode.bamboo(prompt, { provider: "deepseek" });
```

The dispatch is `pandacode bamboo exec --provider <provider> ...`. Supported
provider names are `deepseek`, `xiaomi`, `kimi`, `zhipu`, `minimax`, `qwen`, and
`stepfun`. Passing `provider` with `runtime: "claude"` or `runtime: "codex"` is
an authoring error.

## Built-in Workflow parity

ODW's script runtime matches the Claude Code built-in Workflow tool on these
runtime behaviors:

- **Cores-aware concurrency.** `parallel`/`pipeline`/`fanout` cap at
  `min(16, cpuCores - 2)`; a 1000-agent-per-run backstop guards runaway loops.
- **Determinism guard.** Inside a workflow script, `Date.now()`,
  `Math.random()`, and argless `new Date()` throw (they break resume). Deterministic
  forms (`new Date(ts)`, `Date.parse`, all other `Math.*`) still work. Runner
  internals keep using the real clock.
- **`isolation: "worktree"`.** Set it on an `agent(...)` node to run its executor
  in a throwaway git worktree branched from `cwd`, so file-mutating agents in a
  `parallel(...)` group do not conflict. The worktree is removed on success,
  error, or timeout. Requires `cwd` to be a git repo.
- **Real `budget`.** Seed `args.budget.total` (tokens). `budget.spent()` sums
  real token usage from PandaCode reports, including Bamboo reports that include
  usage. Nodes without token usage report 0 and mark the budget `approx`;
  `budget.remaining()` tracks it; once spent reaches total, the next
  `agent(...)` throws. `spent` persists across `--resume` and is not
  double-counted for cached nodes.
- **`workflow(nameOrRef, args)`.** Run a saved/sibling workflow inline as one
  step. It shares this run's agent counter, concurrency caps, budget, and state.
  1 level only: a sub-workflow that calls `workflow()` throws. Names resolve to
  `.claude/workflows/<name>.js`, `odw-<name>.js`, or a relative/absolute path.
- **`meta.whenToUse` and per-phase `model`.** `meta.phases[].model` sets a
  default model that a phase's agents inherit when they omit `options.model`.

```js
export const meta = {
  name: "ship-with-budget",
  whenToUse: "implement a change under a token budget with isolated workers",
  phases: [{ title: "Build", detail: "codex implements", model: "opus" }]
};
phase("Build");
const reviews = await parallel(batches.map((b, i) => () =>
  agent(promptSlot("review", { b }), { id: `rev-${i}`, isolation: "worktree" })
));
if (budget.remaining() !== null && budget.remaining() < 50_000) {
  return { ok: true, note: "budget nearly exhausted" };
}
const sub = await workflow("synthesize", { reviews });
```

These behaviors are self-verified: `node scripts/selftest.mjs` runs `odw` against
crafted mock workflows and asserts every parity feature (token-free,
deterministic). It is wired into `cargo test` as the `parity_selftest`
integration test, so the gate fails if any parity behavior regresses.

## Status

Implemented now:

- Rust CLI named `odw`
- Open Dynamic Workflow project pack installer
- direct workflow script contract
- direct JavaScript runner through `odw exec`
- prompt-slot injection for node prompts
- complex flow starter with dynamic fan-out, join, parallel review, quality gate,
  and bounded rework loop
- framework `.d.ts` and runtime contract docs
- project-level Claude Code agent types
- slash commands and starter workflow scripts
- worker output schemas
- saved workflow artifact evidence reader
- live run journals under `.odw/runs`
- checkpointed direct resume with `odw exec --resume`
- single-shot Codex execution through PandaCode (`runtime: "codex"`)
- Bamboo provider dispatch through PandaCode (`runtime: "bamboo", provider`)
- structured error feedback schema and failure analyst agent
- validation, doctor, unit tests, clippy checks

Remaining product work:

- richer reusable workflow template library
- live `runs watch` view over the journal

## Contributing

The gate every change must pass (also enforced by CI in `.github/workflows/`):

```bash
cargo build --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
node scripts/selftest.mjs
```

`scripts/selftest.mjs` is the parity self-test — token-free, mock-backed, and the
fastest way to confirm the runtime still matches the built-in Workflow contract.
See `CONTRIBUTING.md` for details.

## License

MIT — see [LICENSE](LICENSE).
