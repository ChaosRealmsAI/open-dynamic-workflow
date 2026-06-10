# First User Flow

This is the intended first-user path for using Open Dynamic Workflow as a direct
script runner.

## 1. Install

```bash
cd /path/to/project
odw init --path .
odw doctor --path .
odw validate --path .
odw spec
odw capabilities
odw runs list --path .
```

Review generated project settings before enabling any tool permissions in the
host environment.

## 2. Write and Run a Workflow

Create or reuse a workflow JavaScript file, then run it directly:

```bash
odw exec --script .claude/workflows/odw-audit.js --input '{"goal":"src/routes for missing auth checks"}' --backend mock
odw exec --script .claude/workflows/odw-flow.js --input '{"goal":"complex flow smoke"}' --backend mock
odw runs show latest
```

For PandaCode-backed execution:

```bash
odw exec --script <workflow.js> --input-file workflow-input.json --backend pandacode
```

`workflow-input.json` should include `goal` plus `prompts` for the workflow's
declared `meta.promptSlots`.

## 3. Expected Workflow Behavior

The caller or generated workflow should:

1. Read `.odw/README.md`.
2. Read `.odw/framework/runtime-contract.md` and
   `.odw/framework/workflow-api.d.ts`.
3. Read `.claude/workflows/odw-authoring-contract.md`.
4. Use `phase(...)` for observable stages.
5. Use `promptSlot(name, context, suggested)` and inject real prompts through
   `input.prompts.<slot>` for non-mock runs.
6. Give nodes stable labels; direct-run resume keys are derived from
   `prompt + options`.
7. Use `checkpoint(...)` at resume boundaries.
8. Treat `agentType` as optional author-defined metadata, not a required enum.
9. Route implementation planning/execution with explicit workflow code. Under
   PandaCode, use `runtime: "codex"` when selecting Codex execution.
10. Give executor nodes clear prompts, stable ids, and any required model or
    effort options.
11. Add `schema`, `schemaDescription`, and `retry.maxAttempts` only when a node
    must satisfy a structured final-response contract; ODW otherwise applies no
    default schema and will inject schema mismatch context into the same node
    prompt only for schema-enabled nodes.
12. Use `fanout(items, mapper)` when an upstream node decomposes work into
    dynamic downstream tasks.
13. Route executor, shell, schema, or worker failures to a workflow-defined
    feedback node.
14. Route tests, claim checking, and synthesis through ordinary `agent(...)`
    nodes selected by the workflow author.

## 4. PandaCode Codex Nodes

When a workflow reaches implementation, it should call a single Codex executor
node through PandaCode:

```js
const implementationPrompt = `
Role:
PandaCode Codex executor.

Input:
${JSON.stringify({ goal: args, plan }, null, 2)}

Task:
Implement the scoped change in one pass.

Constraints:
Keep the change bounded by the requested goal and known forbidden zones.
Return evidence for changed files, verification, risks, and blockers.

Output schema:
.odw/schemas/codex-result.schema.json

Done criteria:
The downstream verification node has enough evidence to check the change.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json-compatible
feedback inside the final result.
`;

const result = await agent(implementationPrompt, {
  id: "implement-auth",
  label: "implement auth",
  phase: "Implement",
  agentType: "odw-codex-coder",
  runtime: "codex",
  schema: ".odw/schemas/codex-result.schema.json",
  schemaDescription: "Final response is implementation evidence for downstream test and verifier nodes.",
  retry: { maxAttempts: 2 },
  sandbox: "danger-full-access",
  approvalPolicy: "never"
});
checkpoint("after-implement", result);
```

Under `--backend pandacode`, this dispatches to `pandacode codex exec`. Planning,
decomposition, review, rework, and synthesis should be ordinary workflow nodes
before or after the executor node, not a separate executor lifecycle.

## 5. Control Surface

Direct runner:

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- observe: `odw runs show latest`
- resume a direct run: `odw exec --resume latest`
- list runs: `odw runs list --path .` (`--json` keeps the raw records)

Use `odw workflows remove <name>` for filesystem cleanup.

For CLI-controlled runs:

```bash
odw exec --script <workflow.js> --input '{"goal":"..."}' --backend pandacode
odw exec --resume latest
odw runs show latest
odw evidence --path .
```

## 6. Acceptance Criteria

A successful first-user run proves:

- `odw exec` can run a workflow script directly.
- Direct logs show workflow, phase, node, checkpoint, and exit progress.
- `odw exec --resume latest` skips completed stable node ids from state.
- A workflow can fan out read-only reviewers.
- `odw exec` prints live logs and writes `.odw/runs/<run_id>/events.jsonl`.
- `odw runs show latest` can inspect the saved journal.
- A PandaCode Codex node runs through `agent(prompt, { runtime: "codex" })`.
- Worker failures return structured retry/blocker feedback.
- Final output includes changed files, verification, and residual risk.
