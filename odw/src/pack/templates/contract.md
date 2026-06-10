# Open Dynamic Workflow Workflow Authoring Contract

This file is a contract for agents that write Open Dynamic Workflow scripts.

The primary runtime is `odw exec`. External agents such as Claude Code or Codex
can call the same CLI and scripts, but ODW itself does not install slash commands
or project files.

For a built-in large-project workflow template, run:

```bash
odw starter parallel-review-apply > wf.js
```

That starter fans out isolated Codex worktrees, reviews the combined candidate
in a temporary worktree, repairs reviewer-rejected batches up to
`args.maxReviewRounds` (default 2 for small batches and 3 for 3+ tasks), targets blocker-matched task files when
possible, lands only `approve` gates atomically, and then verifies the main
working directory under a read-only snapshot guard. If final verification
modifies files after approval, the run restores those unapproved changes and
fails instead of bypassing review.
Pass explicit `args.tasks` when decomposition and file ownership are already
known. For lower decision cost, pass `args.request` or `args.spec` without
`tasks`; the starter first runs a structured planning node that returns owned
task files, then sends that plan through the same preflight, review, apply, and
verification gates.
Each task must declare a stable unique `id`; ODW uses task ids for node keys,
sessions, repair history, and reports. Each task must also declare a non-empty
string `prompt`; empty or non-string prompts are rejected before worktrees are
created.
Before review, it blocks failed implementation nodes and cross-owned file edits.
Use `task.file` or `task.files` to declare each task's ownership. Use the
built-in request/spec planner for exploratory decomposition, or set
`allowUndeclaredTaskFiles:true` only when the owner accepts weaker ownership
checks. Declared files must be normalized
repo-relative paths outside `.git`, `.odw`, `.pandacode`, and `node_modules`;
absolute paths, backslashes, and `..` escapes are rejected before worktrees are
created. Set `strictTaskFileBoundaries:false` only with explicit owner intent.
Test and documentation tasks should target the declared files and exports from
the planned task set. If a required public entrypoint is missing from task
ownership, treat it as a planning blocker or add it to a task; do not invent
undeclared entrypoints or skip tests to make isolated verification pass.
The starter injects the run context and full planned task list into every
implementation/repair prompt, so tests, docs, entrypoints, and implementation
modules can align on one shared contract even though they run in isolated
worktrees.
Because isolated worktrees branch from `HEAD`, it also blocks dirty declared
task files before implementation; commit/stash them first, or set
`allowDirtyTaskFiles:true` only when the owner accepts that workers will not see
those uncommitted changes.
It also blocks duplicate declared ownership of the same file; merge those tasks,
run them serially, or set `allowDuplicateTaskFiles:true` only when overlapping
patches are intentional and reviewable.

## Required concepts

- `phase`: named stage in the run, visible in terminal logs and
  `.odw/runs/<run_id>/events.jsonl`
- `agent`: one Open Dynamic Workflow worker invocation. `agentType` is an
  optional author-defined routing/tag string, not a fixed enum.
- `pandacode`: convenience namespace — `pandacode.codex(prompt)`,
  `pandacode.claude(prompt)`, `pandacode.bamboo(prompt, { provider })`, and
  `pandacode.exec(runtime, prompt)`; each is ordinary `agent(...)` dispatch.
  Bamboo provider nodes use `runtime: "bamboo"` with `provider` / `bambooProvider`.
- `checkpoint`: persist a resume boundary for `odw exec --resume`
- `parallel`: fan out independent agents
- `pipeline`: pass verified outputs from one phase to the next
- `reviewWorktreeDiffs`: review captured worktree patches before landing; ODW
  preflights the combined patch, applies it inside a temporary candidate
  worktree, and runs structured reviewer agents there
- `applyWorktreeDiffs`: atomically apply captured worktree patches to the main
  cwd after review; partial landing requires explicit `continueOnError:true`
- `verify`: adversarial review before synthesis
- `synthesize`: final answer returned to the caller

ODW starter scripts use this shape:

```js
export const meta = {
  name: "example",
  phases: [{ title: "Research" }, { title: "Verify" }]
};

const target = args;

phase("Research", "read files");
const result = await agent(prompt, {
  label: "research",
  phase: "Research"
});

return result;
```

For parallel work, use `parallel([() => agent(...), ...])`. ODW emits group-level
`parallel_start` / `parallel_done` events, runs child nodes up to `max`
concurrency, preserves result order, and resumes from the stable
`prompt + options` cache key.

For dynamic decomposition, let an upstream node return a structured task array,
then use `fanout(tasks, (task, index) => agent(...))`. Each child should have a
stable `id` derived from the task.

For staged item streams, use `pipeline(items, ...stages)`.

For parallel file edits, run mutating nodes with `isolation: "worktree"`, then
call `reviewWorktreeDiffs(results, opts)` before `applyWorktreeDiffs(results)`.
Only a review gate with `decision: "approve"` / `applyReady: true` should be
auto-landed. `reject` means rework first, preferably by running a fresh isolated
worktree round with reviewer feedback; `needs_owner` means ask the product/code
owner before applying.

ODW does not create default nodes or apply a default schema. The workflow
author decides the flow with ordinary JavaScript plus `agent(...)`,
`parallel(...)`, `fanout(...)`, and `checkpoint(...)`.

Workflow scripts are orchestration code. Do not import Node modules, read or
write project files, spawn shell commands, or edit code directly from the
script. Put that work inside `agent(...)` executor nodes.

When `agent(..., { schema, schemaDescription, retry: { maxAttempts } })` is
used, ODW treats the schema as an opt-in final-response contract. The node still
does its normal work first, including file edits and commands when requested.
ODW validates only the final response against the schema. On mismatch it emits
`agent_schema_invalid`, injects the schema errors and previous result into the
same node prompt, and retries until `maxAttempts` is exhausted. The final
failure is structured as `.odw/schemas/error-feedback.schema.json` with
`schema_mismatch` and a node reference so downstream feedback nodes can route
it. Schema nodes must return final JSON only; review verdicts should put reject
evidence in `blockers`, `risks`, `owner_questions`, and `verification` rather
than prose outside the JSON object.

## Optional Starter Labels

Starter workflows use labels such as `odw-researcher`, `odw-codex-coder`, and
`odw-verifier` as conventions. Workflow authors can use any `agentType` string
or omit it entirely. Execution is selected by the workflow code with
`runtime: "claude"`, `runtime: "codex"`, or `runtime: "bamboo"` under
`--backend pandacode`. Bamboo requires `provider` such as `deepseek`, `xiaomi`,
`kimi`, `zhipu`, `minimax`, `qwen`, or `stepfun`; provider is invalid on
non-Bamboo runtimes.

## Worker prompt format

Reusable scripts should declare prompt slots in `meta.promptSlots` and call
`promptSlot(name, context, suggested)` near the node that uses the prompt. Real
runs should inject `input.prompts.<slot>`. Suggested prompt text exists for mock
smoke tests or explicit caller opt-in; it is not the product's hidden runtime
policy.

Every injected or suggested worker prompt must include:

```text
Role:
Input:
Task:
Constraints:
Output contract:
Done criteria:
Failure contract:
```

If the node opts into `schema`, the corresponding `agent(...)` call must include
`schemaDescription` explaining what the final structured response is for, and
the prompt's output contract should name that schema. Nodes without `schema`
should still explain the expected final response, but ODW will not validate it
against a default schema.

Failure contract should say: if blocked, return
`.odw/schemas/error-feedback.schema.json` with category, retryability, next
action, and retry prompt. Do not return plain-language failure only.

Example:

```js
export const meta = {
  name: "example",
  promptSlots: ["plan"]
};

const planPrompt = promptSlot("plan", {
  goal: args,
  required_schema: ".odw/schemas/codex-plan.schema.json"
}, `
Role:
Codex Plan-mode worker.

Input:
{{context}}

Task:
Create a scoped implementation plan.

Constraints:
Plan only. Do not edit files.

Output schema:
.odw/schemas/codex-plan.schema.json

Done criteria:
The implementation node can execute the plan without rediscovery.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
```

## Runtime operations

Use ODW's direct surface for:

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- observe: `odw runs show latest`
- report: `odw report --script <workflow.js> --open` for a mock-derived graph,
  or `odw report --run <id> --open` for an existing run
- resume: `odw exec --resume latest`
- list runs: `odw runs list --path .`

When launched through `odw exec`, ODW records:

- live workflow, phase, node, checkpoint, error, and exit summaries
- full event journal at `.odw/runs/<run_id>/events.jsonl`
- direct resume state at `.odw/runs/<run_id>/state.json`
- raw script stderr at `.odw/runs/<run_id>/script-debug.log`
- run metadata at `.odw/runs/<run_id>/run.json`
- HTML report at `.odw/runs/<run_id>/report.html` when report generation is enabled
- `odw exec --resume latest`

## PandaCode backend decision

Open Dynamic Workflow is a pure orchestration runtime. It dispatches executor
nodes to PandaCode with `pandacode <runtime> exec`; PandaCode owns runtime
accounts, logs, models, provider credentials, token usage, and execution. Bamboo
domestic-provider nodes dispatch as
`pandacode bamboo exec --provider <provider>`.
