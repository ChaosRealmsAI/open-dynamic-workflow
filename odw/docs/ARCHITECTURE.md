# Architecture

## Principle

Open Dynamic Workflow is a direct JavaScript orchestration runtime.

The core path is agent-driven: an agent writes or selects a workflow script,
then calls `odw exec` to run it. ODW owns orchestration, event journaling,
checkpoint state, schema retry, and dispatch to executor nodes.

The only executor backend for real work is PandaCode. The `mock` backend exists
for tests and smoke runs.

## Layers

```text
Agent or CLI caller
  -> odw exec --script <workflow.js>
  -> ODW JavaScript runner
  -> phase / agent / parallel / fanout / pipeline / log / checkpoint
  -> PandaCode executor backend
  -> pandacode codex exec for agent(prompt, { runtime: "codex" })
  -> .odw/runs/<run_id>/events.jsonl + state.json
```

## Agent Type vs Runtime

Open Dynamic Workflow treats optional node labels and executor runtime as
separate concepts.

```text
node label / agentType = optional author-defined routing/tag metadata
runtime = PandaCode execution target for the node
backend = ODW executor backend, currently mock or pandacode
```

In the current direct-runner pack:

- `odw exec` is the primary engine for workflow scripts.
- Nodes are ordinary `agent(prompt, options)` calls. ODW does not require a
  fixed agent type; `agentType` is just an optional string.
- Codex execution is selected with
  `agent(prompt, { runtime: "codex" })` under `--backend pandacode`.
- A Codex node is single-shot: the ODW runner passes the final prompt and node
  options to `pandacode codex exec` and stores the normalized result.
- Prompts are long, self-contained JavaScript template strings beside the node
  that uses them. Each agent/node can have its own prompt string with role,
  input, task, constraints, output contract, done criteria, and failure contract.
- Schemas are opt-in. If a node does not pass `schema`, ODW does not add one.
  If a node does pass `schema`, the workflow code must also pass
  `schemaDescription`, and the schema only constrains the final assistant
  response returned to the runner.
- Failed workers can route to any workflow-defined feedback node. The starter
  templates use `odw-failure-analyst` and
  `.odw/schemas/error-feedback.schema.json`.

This keeps ODW workflow code independent of a specific agent label while still
letting starter templates use conventional tags when useful.

## Why `odw exec` First

The core product is a programmable workflow runner that an agent can invoke.
That means the first-class surface is:

- `odw exec --script <workflow.js>` for direct execution
- `.odw/runs` for logs, state, and direct-run resume
- `phase`, `agent`, `parallel`, `fanout`, `pipeline`, `checkpoint`, and `log`
  as script helpers
- `.odw/framework` for the direct runner contract and script types
- `.odw/schemas` for worker output contracts
- `.odw/bin/odw` for project-local worker access to the ODW CLI
- `.odw/schemas/error-feedback.schema.json` for retry/blocker feedback

## Interface Coverage

Open Dynamic Workflow maps the requested workflow controls as follows:

| Capability | Owner |
| --- | --- |
| run workflow | `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>` |
| prompt injection | `promptSlot(name, context, suggested)` reads `input.prompts.<slot>`; suggested prompts are for mock smoke tests or explicit opt-in |
| parallel nodes | `parallel([() => agent(...), ...])` fan-out/join in the direct runner; every child node emits its own node events |
| dynamic task fan-out | `fanout(items, mapper)` maps structured upstream output into downstream workflow nodes |
| pipeline phases | `pipeline(items, ...stages)` plus normal script variables in starter workflows |
| live logs | `odw exec` streams workflow/phase/node/checkpoint events and writes `.odw/runs/<run_id>/events.jsonl` |
| local run journal | `odw runs list` / `odw runs show latest` |
| direct-run resume | `odw exec --resume <run_id|latest>` skips completed stable node ids |
| stop | stop the invoking process |
| save script | workflow scripts are normal files |
| remove template | `odw workflows remove` |
| workflow evidence | `odw evidence` reads saved workflow artifact JSON when present |
| framework spec | `odw spec` and `.odw/framework/workflow-api.d.ts` |
| PandaCode Codex node | `agent(prompt, { runtime: "codex" })` -> `pandacode codex exec` |
| schema retry | `agent(..., { schema, schemaDescription, retry: { maxAttempts } })` is opt-in; it validates only the final assistant response, injects mismatch context into the same node prompt, and retries |
| error feedback | `odw-failure-analyst` + `.odw/schemas/error-feedback.schema.json` |
| pack install/validate | `odw init`, `odw validate` |

## PandaCode Executor

The ODW runner does not implement Codex sessions itself. It delegates executor
work to PandaCode.

For Codex-backed implementation, a workflow uses one ordinary node:

```js
const result = await agent(prompt, {
  id: "implement-auth",
  label: "implement auth",
  phase: "Implement",
  runtime: "codex",
  schema: ".odw/schemas/codex-result.schema.json",
  schemaDescription: "Final response is implementation evidence for downstream verification.",
  retry: { maxAttempts: 2 }
});
```

Under `--backend pandacode`, that node dispatches once to `pandacode codex exec`.
Planning, decomposition, review, rework, and synthesis are modeled as normal
workflow nodes rather than as a hidden executor lifecycle.
