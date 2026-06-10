# Open Dynamic Workflow

Open Dynamic Workflow (`odw`) is a script-driven workflow runner for
agent-authored JavaScript workflows.

It is **zero install** — nothing is scaffolded into your project. Run `odw guide`
for the full self-contained usage guide, then write a workflow script and run it:

```bash
odw guide                                                 # how to author + run (self-contained)
odw starter parallel-review-apply > wf.js                 # reusable large-project workflow
odw exec --script wf.js --input-file input.json --backend pandacode
odw exec --resume latest
odw runs show latest
```

`input.json` (optional) contains the goal and any `prompts` for slots the
workflow declares; it is exposed to the script as `args`.

The JavaScript workflow owns phases, branching, fan-out, intermediate results,
checkpoints, and synthesis. ODW provides the direct runner, node logs, resume
state, schema validation, and a PandaCode bridge for the Claude / Codex / Bamboo
executor runtimes.

## Boundary

```text
Agent or CLI caller
  -> odw exec --script <workflow.js>
  -> phase(...)
  -> agent(prompt, { label, phase, runtime, provider?, model?, schema?, schemaDescription?, agentType? })
  -> .odw/runs/<run_id>/events.jsonl + state.json
  -> pandacode claude|codex|bamboo exec
```

Claude Code, Codex, shell scripts, CI, or another agent can call the same CLI and
workflow files. ODW itself does not install slash commands or project files.
Worker failures are also part of the contract: failed workers return structured
error feedback instead of unclassified prose.

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

**Zero install — nothing to scaffold.** odw never writes files into your project.
The binary is self-documenting, so any agent can use it straight from the CLI:

```bash
odw guide                       # the full self-contained authoring + run guide (read this first)
odw starter --list              # built-in workflow templates
odw doctor                      # check node + the pandacode executor are wired up
odw spec | odw contract         # machine-readable API types + the authoring contract
```

`odw` finds the `pandacode` binary automatically when it sits next to `odw` (the
workspace builds both into the same dir, whether `cargo install` or `cargo
build`). Only set `ODW_PANDACODE_BIN=/path/to/pandacode` (or `--pandacode-bin`)
if yours lives elsewhere.

**For agents:** run `odw guide`. It is the single self-contained entry point —
what odw is, the full authoring API, how to run, and the gotchas. No skill to
install, no files to read; the same content any AI (Claude, codex, or otherwise)
gets straight from the CLI. `odw spec` adds the TypeScript types; `odw contract`
the full authoring contract.

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

## External Agent Usage

Any agent can use ODW through the CLI. The reliable bootstrap is:

```bash
odw guide
odw starter parallel-review-apply > wf.js
odw exec --script wf.js --input-file input.json --backend pandacode
```

The built-in starter is the large-project path: parallel Codex worktrees,
candidate-worktree review, bounded repair/re-review, approve-only atomic
landing, and read-only final verification. It repairs failed implementation
nodes or cross-owned file edits before review; declare each task with `task.file`
or `task.files` when you want maximum control. For lower decision cost, pass a
high-level `request` or `spec` without `tasks`; the starter first asks a
structured planner to produce owned task files, then runs the same preflight,
review, apply, and verification gates. Set `strictTaskFileBoundaries:false` only
with explicit owner intent. It also refuses to run when declared task files are
already dirty, because isolated worktrees branch from `HEAD`; commit/stash those
files first or pass `allowDirtyTaskFiles:true` only when the owner accepts that
workers will not see the dirty changes.

Then write or adapt a workflow with this shape:

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
  available as `args`. Workflow scripts are orchestration-only (no direct
  filesystem/shell access); file, shell, and code-edit work must go through
  `agent(...)` executor nodes. The vm context gives that separation and a
  determinism guard, not a security boundary — treat workflows as trusted code
  you author, not a way to run untrusted scripts.
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
  Schema as a final-response-only contract, validates that final response, and on
  a mismatch injects the mismatch context into the node prompt and retries.
  **Retry is NOT automatic: a node attempts once by default** (`maxAttempts:1`),
  unlike the built-in tool which keeps retrying on mismatch — pass
  `retry: { maxAttempts: N }` (or `maxAttempts: N`) for built-in-style retry,
  else a single non-conforming reply returns structured `schema_mismatch`
  feedback immediately. Route schema nodes to claude (coding agents like codex/
  bamboo are unreliable at structured output). If `schema` is omitted, ODW applies
  no default schema.
- `error feedback`: `.odw/schemas/error-feedback.schema.json` is the standard
  result when a worker, command, schema, or Codex step fails.

`odw-orchestrator` plans and routes. The workflow script owns executable
branching, fan-out, loops, intermediate state, and final aggregation.

## Lifecycle

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- watch: `odw runs show latest`
- pause/resume: `odw exec --resume latest`
- stop: stop the invoking process
- restart node: direct exec resumes by the stable `prompt + options` cache key;
  completed nodes are skipped from state (editing a node's prompt re-runs it)
- live logs: `odw exec` streams node progress
- local journals: `odw runs list` and `odw runs show latest`; use
  `odw runs list --json` for the raw machine-readable list

## CLI

```bash
odw guide                                            # self-contained authoring + run guide
odw doctor                                           # check node + pandacode are wired up
odw spec                                             # framework spec + TypeScript API types
odw contract                                         # full authoring contract
odw capabilities                                     # machine-readable capability map
odw exec --script wf.js --input '{"goal":"x"}' --backend mock   # token-free dry run
odw exec --script wf.js --backend pandacode          # real run
odw exec --resume latest
odw report --script wf.js --open                     # HTML execution-graph preview
odw runs list                                      # compact run list
odw runs list --json                               # raw run records
odw runs show latest
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

Bamboo is a **tool-using coding agent** — it shines at reading/writing files and
running commands. Its node result is a prose *summary* of what it did, not raw
content, so it is a poor fit for nodes whose value IS the answer:

- **Prose deliverables** (a review, an analysis) can fail with `missing JSON
  object in model response` when the agent answers in text instead of a tool call.
- **`schema:` structured-output nodes** also tend to fail — a schema does **not**
  fix it (verified: a schema'd bamboo classification returned its answer as prose
  — "...negative with score 0.95..." — and missed the schema on every retry).

Route answer-shaped nodes to `runtime: "claude"` / `"codex"`; keep bamboo for the
file/command work it is built for. See issue #5.

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
  error, or timeout. Requires `cwd` to be a git repo. Captured diffs can be
  reviewed with `reviewWorktreeDiffs(results)` before landing; reviewers run in a
  temporary candidate worktree where the combined diff is already applied. Land
  only an `approve` gate with `applyWorktreeDiffs(results)`, which is atomic by
  default.
- **Real `budget`.** Seed `args.budget.total` (tokens). `budget.spent()` sums
  each node's **total** token usage (input + output + cache + reasoning) from
  PandaCode reports. **This differs from the built-in tool, whose `spent()` counts
  output tokens only** — and the gap is large for coding-agent nodes, whose cost
  is dominated by the input harness (a trivial Bamboo node can report ~19k total
  but <300 output). So a budget loop ported from the built-in exhausts far sooner
  here; size budgets in *total* tokens, not output. Nodes without token usage
  report 0 and mark the budget `approx`; `budget.remaining()` tracks it; once
  spent reaches total, the next `agent(...)` throws. `spent` persists across
  `--resume` and is not double-counted for cached nodes. It is best-effort, not a
  hard cap: under concurrency, in-flight nodes still finish, so a run can overshoot
  by up to ~`concurrency × per-node tokens` (as the built-in tool's budget also does).
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

### Where ODW goes further

- **Heterogeneous executors.** Each node picks its own runtime/provider/model, so
  one `parallel(...)` can fan a task across codex + claude + several domestic
  models at once — the built-in tool runs claude subagents only.
- **Persistent, offline observability.** Every run writes `events.jsonl` and a
  standalone HTML execution graph: per-node runtime, the *resolved* model (even
  when the script left it implicit), real token count, prompt, status, and
  duration. The built-in tool's progress tree is ephemeral.
- **Structured failures, not exceptions.** A node that exhausts its retries
  returns `{ ok: false, error: { category, message } }` instead of throwing, so a
  script can inspect *why* a node failed (`result.ok === false`,
  `result.error.category`). A thunk that *throws* still resolves to `null` inside
  `parallel`/`pipeline`, matching the built-in `.filter(Boolean)` idiom — so use
  `.filter(r => r && r.ok !== false)` when you want to drop failed nodes too.
- **claude token accounting.** claude nodes report token usage (parsed from the
  Claude Code session transcript), so `budget` counts them like codex/bamboo.

### Where the built-in tool is better

Honest tradeoffs — reach for the built-in Workflow when these matter:

- **Zero setup.** It runs inside Claude Code with no install; ODW needs the `odw`
  binary, Node, and (for real runs) PandaCode + runtime CLIs on PATH.
- **Live progress.** Its progress tree updates in-terminal as agents run; ODW's
  HTML graph is rendered after the run (the live signal is `events.jsonl`).
- **Real subagent types.** `agentType` there selects a custom Claude subagent with
  its own system prompt and toolset; ODW has no subagent registry, so `agentType`
  is only a routing/tag value — `runtime` (`codex`/`claude`/`bamboo`) is the real
  selector. Prose/analysis/structured-output nodes should target `claude`.
- **Automatic schema retry.** It keeps re-prompting until structured output
  validates; ODW attempts once by default (`maxAttempts:1`) — pass `retry`.
- **Output-token budgets.** Its `budget.spent()` counts output tokens (matching
  Claude Code's `+Nk` metering); ODW counts total tokens, so the same ceiling
  trips much sooner here. Size ODW budgets in total tokens.

## Status

Implemented now:

- Rust CLI named `odw`
- direct workflow script contract
- direct JavaScript runner through `odw exec`
- prompt-slot injection for node prompts
- complex flow starter with dynamic fan-out, join, parallel review, quality gate,
  and bounded rework loop
- reusable large-project example: parallel Codex worktrees → candidate-workspace
  review gate → approve-only atomic landing → read-only verification guard
- framework `.d.ts` and runtime contract docs
- built-in `odw starter parallel-review-apply`
- worker output schemas
- live run journals under `.odw/runs`
- compact `odw runs show` summaries with report paths
- self-contained HTML execution reports
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
