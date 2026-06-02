# Open Dynamic Workflow Runtime Contract

Open Dynamic Workflow is a direct JavaScript workflow runner with Codex and
Claude Code compatibility adapters.

The direct runtime contract is:

1. The caller writes or loads a JavaScript workflow script.
2. The script exports `meta` and then uses top-level async workflow code.
3. Workflow input is available as `args`; ODW also exposes `input` for
   compatibility.
4. Real runs inject node prompts through `input.prompts.<slot>`. Scripts may
   call `promptSlot(name, context, suggested)`; suggested text is for mock
   smoke tests unless the caller explicitly enables it.
5. The script declares phases with `phase(title, detail?)`.
6. The script invokes nodes with `agent(prompt, options)`. ODW does not create
   default nodes; the workflow code decides the flow by calling `agent(...)`,
   `parallel(...)`, `fanout(...)`, and normal JavaScript control flow. ODW also
   does not apply a default schema. If `options.schema` is set, the runner
   appends the full JSON Schema as a final-response-only contract to that node
   prompt before execution. `options.schemaDescription` is optional; when
   provided it is added to that contract.
7. Node fan-out uses `parallel([() => agent(...), ...])`; item streams use
   `pipeline(items, ...stages)`.
8. The script keeps loops, branching, intermediate results, and aggregation in
   script variables.
9. Executor work uses ordinary `agent(...)` calls. With `--backend pandacode`,
   `runtime: "claude"` maps to `pandacode claude exec`,
   `runtime: "codex"` maps to `pandacode codex exec`, and
   `runtime: "bamboo"` with `provider: "<domestic-provider>"` maps to
   `pandacode bamboo exec --provider <domestic-provider>`. Supported Bamboo
   provider names include `deepseek`, `xiaomi`, `kimi`, `zhipu`, `minimax`,
   `qwen`, and `stepfun`. `provider` is only valid for Bamboo nodes.
10. Runs are watched through `odw runs show <run_id|latest>`.
11. Direct runs resume with `odw exec --resume <run_id|latest>`. Completed node
   ids are cached in `.odw/runs/<run_id>/state.json` and skipped on resume.
12. PandaCode node results passed to later nodes are compact summaries. The full
   raw PandaCode report is written under `.odw/runs/<run_id>/pandacode-*.report.json`
   and referenced by `result.artifacts.raw_report`.
13. A worker that cannot complete must return `.odw/schemas/error-feedback.schema.json`
   instead of unstructured prose.
14. Codex Plan mode is preserved. ODW uses it for plan-only work and as the
   approval gate before Codex execution.

Documented lifecycle alignment:

- run: `odw exec --script <workflow.js> --input <json> --backend <mock|pandacode>`
- optional Claude run: `/odw`, `/odw-audit`, `/odw-ship`, `/odw-flow`
- watch: `odw runs show latest`; optional Claude watch: `/workflows`
- pause/resume: `odw exec --resume latest`
- stop: stop the invoking process, or `/workflows` then `x` for Claude-launched runs
- restart selected running agent: direct exec resumes by stable node id; `/workflows`
  then `r` for Claude-launched runs
- save reusable workflow script: commit or copy the workflow file; `/workflows` then
  `s` for Claude-launched runs
- remove saved project template: `odw workflows remove <name>`
- read saved artifact evidence: `odw evidence`
- observe local run logs: `odw runs list` and `odw runs show latest`
- route executor work: `agent(..., { runtime: "claude" })` or
  `agent(..., { runtime: "codex" })`, or
  `agent(..., { runtime: "bamboo", provider: "deepseek" })` under
  `--backend pandacode`

Documented runtime limits:

- concurrency capped at `min(16, cpuCores - 2)` agents
- up to 1,000 agents total per run (runaway-loop backstop; `agent()` throws past it)
- no mid-run user input except permission prompts
- workflow scripts are orchestration-only (no direct filesystem/shell access);
  file, shell, and code-edit work goes through `agent(...)` executor nodes. The
  vm context provides that separation and a determinism guard, not a security
  boundary — treat workflows as trusted code you author
- `Date.now()`, `Math.random()`, and argless `new Date()` throw inside workflow
  scripts to keep resume deterministic
- `budget.total` (from `args.budget.total`) enforces a token ceiling: once
  `budget.spent()` reaches it, the next `agent(...)` throws. It is a best-effort
  ceiling, not a hard cap: under concurrency, nodes already in flight when the
  limit is crossed still finish, so a `parallel`/`pipeline` run can overshoot by
  up to ~`concurrency × per-node tokens` (the built-in tool's budget behaves the
  same). Use it to bound runaway loops, not for exact spend control.
- `agent(..., { isolation: "worktree" })` runs the executor in a throwaway git
  worktree branched from cwd, removed when the node finishes
- `workflow(nameOrRef, args)` runs a saved/sibling workflow inline (1 level),
  sharing this run's agent counter, caps, budget, and state

ODW extension points:

- `.claude/agents/*.md`: agent type definitions
- `.claude/commands/*.md`: slash-command entrypoints
- `.claude/workflows/*.js`: reusable workflow scripts
- `.odw/bin/odw`: project-local ODW CLI wrapper for worker Bash commands
- `.odw/runs/*/events.jsonl`: local observable run journals
- `.odw/schemas/*.schema.json`: worker output contracts
- `.odw/framework/workflow-api.d.ts`: script authoring types
