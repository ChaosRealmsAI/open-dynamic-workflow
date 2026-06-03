# Open Dynamic Workflow Workflow Authoring Contract

This file is a contract for agents that write Open Dynamic Workflow scripts.

The primary runtime is `odw exec`. Claude Code can still call the same scripts
through `/odw`, but slash commands are compatibility entrypoints, not the
required trigger.

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
it.

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
- `odw exec --resume latest`

Claude Code's `/workflows` surface remains available for Claude-launched runs.

Use `odw workflows remove <name>` to remove saved Open Dynamic Workflow templates from
the filesystem.

## PandaCode backend decision

Open Dynamic Workflow is a pure orchestration runtime. It dispatches executor
nodes to PandaCode with `pandacode <runtime> exec`; PandaCode owns runtime
accounts, logs, models, provider credentials, token usage, and execution. Bamboo
domestic-provider nodes dispatch as
`pandacode bamboo exec --provider <provider>`.
