# Open Dynamic Workflow

Open Dynamic Workflow is a script-driven workflow runner for agent-authored
JavaScript workflows.

The primary user is an agent or CLI caller. The caller writes or selects a
workflow script, then runs it directly:

```bash
odw exec --script .claude/workflows/odw-flow.js --input-file workflow-input.json --backend pandacode
odw exec --resume latest
odw runs show latest
```

`workflow-input.json` should contain the goal and `prompts` object for the
slots declared by the selected workflow. Mock runs may use suggested prompt text
from the starter scripts.

Open Dynamic Workflow gives that caller:

- custom agent types under `.claude/agents`
- slash commands under `.claude/commands`
- workflow authoring contract and starter scripts under `.claude/workflows`
- worker output schemas under `.odw/schemas`
- a project-local ODW CLI wrapper under `.odw/bin/odw`
- a PandaCode bridge that calls Claude, Codex, and Bamboo executor nodes
- local run journals under `.odw/runs`
- direct live logs for workflow, phase, node, checkpoint, error, and exit events
- resumable direct runs through `.odw/runs/<run_id>/state.json`
- prompt-slot injection through `input.prompts.<slot>` for real runs
- structured failure feedback for blocked or failed worker runs

ODW is a pure orchestration runtime. The only executor backend is `pandacode`
(plus `mock` for token-free smoke tests). A node runs one thing: an
`agent(prompt, { runtime, provider? })` call dispatches to
`pandacode <runtime> exec` (single-shot, runs to completion). `runtime: "codex"`
maps to `pandacode codex exec`; `runtime: "claude"` maps to
`pandacode claude exec`; `runtime: "bamboo"` with `provider` maps to
`pandacode bamboo exec --provider <provider>`. The workflow script stays the
caller/orchestrator.

Codex shape (single-shot):

```js
const result = await agent(implementationPrompt, {
  runtime: "codex",
  agentType: "odw-codex-coder",
  schema: ".odw/schemas/codex-result.schema.json"
});
```

Bamboo domestic provider shape:

```js
const result = await agent(implementationPrompt, {
  runtime: "bamboo",
  provider: "deepseek"
});

const same = await pandacode.bamboo(implementationPrompt, {
  provider: "deepseek"
});
```

Supported Bamboo providers: `deepseek`, `xiaomi`, `kimi`, `zhipu`, `minimax`,
`qwen`, `stepfun`. Provider is invalid on non-Bamboo runtimes.

If the executor fails, workers return `.odw/schemas/error-feedback.schema.json`
with a category, retryability, user-facing message, and next action. They do not
bury command failures inside prose.

Recommended direct use:

```text
Write a workflow module, then run:
odw exec --script <workflow.js> --input-file workflow-input.json --backend pandacode
```

Node-level fan-out uses the runner API:

```js
const reviews = await parallel(batches.map((batch, index) => () =>
  agent(promptSlot("review_batch", { batch, index }), {
    id: `review-${index}`,
    label: `review ${batch.name}`,
    phase: "Review",
    agentType: "odw-security-reviewer"
  })
));

const verified = await pipeline(
  reviews,
  finding => agent(verifyPrompt(finding), { phase: "Verify" }),
  verdict => agent(synthesizePrompt(verdict), { phase: "Synthesize" })
);
```

Optional use inside Claude Code:

```text
/odw-audit src/routes for missing auth checks
/odw-ship implement the agreed small feature with Codex for code edits and verifier agents for checks
/odw-flow decompose a feature into parallel Codex tasks, join, verify, review, and quality-gate it
```

Framework files:

- `.odw/framework/runtime-contract.md`
- `.odw/framework/workflow-api.d.ts`
- `.odw/bin/odw`

Starter workflow drafts:

- `.claude/workflows/odw-audit.js`
- `.claude/workflows/odw-ship.js`
- `.claude/workflows/odw-flow.js`

These scripts use Claude Code Dynamic Workflow-compatible top-level JavaScript:
`export const meta`, then `phase(...)`, `await agent(...)`, `parallel(...)`,
`pipeline(...)`, and `return`.

## Core Concepts

- `workflow`: a JavaScript module that an agent writes or loads, with
  `export const meta` followed by top-level async workflow code. Input is
  available as `args`. Workflow scripts are sandboxed orchestration code; file,
  shell, and code-edit work must go through `agent(...)` executor nodes.
- `phase`: a named stage emitted to live logs and `.odw/runs/*/events.jsonl`.
- `agent`: a node invocation. The aligned call shape is
  `agent(prompt, { label, phase, runtime, provider?, model, schema?, schemaDescription?, agentType?, isolation? })`.
  ODW does not create default nodes; each `agent(...)`, `parallel(...)`, or
  `fanout(...)` call in workflow code defines the executable node shape.
  `agentType` is an optional author-defined routing/tag value, not a fixed enum.
- `runtime`: ordinary `agent(...)` nodes dispatch through `pandacode` to
  `runtime: "claude"`, `runtime: "codex"`, or `runtime: "bamboo"`, single-shot.
  Bamboo provider nodes dispatch as `pandacode bamboo exec --provider <provider>`.
- `promptSlot`: workflow scripts declare prompt slots; real runs inject
  `input.prompts.<slot>`. Suggested template literals are for mock smoke tests
  or explicit caller opt-in.
- `checkpoint`: persists resume state and emits a checkpoint event.
- `pipeline`: `pipeline(items, ...stages)` runs each item through sequential
  stages while items fan out.
- `parallel`: workflow node-level fan-out/join via
  `parallel([() => agent(...), ...])`; keep concurrency at or below 16.
- `workflow()`: `workflow(nameOrRef, args)` runs a saved/sibling workflow inline
  as one step (1 level), sharing this run's agent counter, budget, and state.
- `isolation`: `agent(..., { isolation: "worktree" })` runs the executor in a
  throwaway git worktree; the agent's diff is returned in `result.worktree`.
- `budget`: seed `args.budget.total` (tokens); `budget.spent()`/`remaining()`
  track real usage and the next `agent(...)` throws once the total is reached.
- `schema`: schema use is opt-in. If a workflow author passes
  `agent(..., { schema })` (with optional `schemaDescription`), ODW appends the
  full JSON Schema as a final-response-only contract, then validates and retries
  with schema mismatch context when needed.
- `error feedback`: failed workers return `.odw/schemas/error-feedback.schema.json`
  with category, retryability, retry prompt, and next action.

`odw-orchestrator` plans and routes. The workflow script owns executable
branching, fan-out, loops, intermediate state, and final aggregation.

Operational mapping:

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- watch: `odw runs show latest`
- pause/resume: `odw exec --resume latest`
- stop: stop the invoking process
- restart node: resume with the stable `prompt + options` cache key; completed
  nodes are skipped from state
- save: workflow scripts are normal project files
- remove saved template: `odw workflows remove <name>`
- evidence: `odw evidence --path .` reads saved Claude Code workflow artifacts
- local journals: `odw runs list` and `odw runs show latest`
- resume latest direct workflow: `odw exec --resume latest`
