---
name: odw
description: Author and run Open Dynamic Workflow (odw) scripts — a CLI that runs JavaScript orchestration workflows (agent / parallel / pipeline / fanout / budget / worktree) and dispatches each node to a real coding executor through PandaCode (codex / claude / bamboo). Use this when a task needs deterministic multi-agent fan-out, staged pipelines, budget-bounded loops, or isolated parallel file edits over real executors — i.e. when you would reach for a "dynamic workflow" but want it as a reproducible, resumable CLI run.
---

# Open Dynamic Workflow (odw)

odw is the **orchestration layer**. You write a small JavaScript workflow that
calls `agent(prompt, opts)` nodes; odw schedules them (parallel / pipeline /
budget / worktree / resume) and dispatches each node to **PandaCode**, which runs
the actual AI (`codex`, `claude`, or `bamboo`) and owns accounts, logs, models.

```
你写 workflow 代码  →  odw 调度(并行/管道/预算/worktree/resume)  →  pandacode <runtime> exec  →  AI 执行
```

You only declare **what to do** and **which runtime**; odw + PandaCode hide the
executor mechanics. A node with no `schema` returns the executor's **final text**;
with a `schema` it returns a **validated object**; on failure it returns
`{ ok:false, error:{...} }`.

## 1. Is odw available? (choose an install method)

Run `odw --version`. If found, you are done. Otherwise install — pick by context:

| Situation | Command |
|---|---|
| Repo is checked out, want it on PATH | `cargo install --path /path/to/open-dynamic-workflow` |
| Just need the binary locally | `cargo build --release` → `target/release/odw` |
| Already built (dev) | `target/debug/odw` |

Then verify the executor wiring:

```bash
odw doctor                 # checks runtimes + required binaries; exits non-zero if unhealthy
export ODW_PANDACODE_BIN=/path/to/pandacode   # if pandacode is not on PATH
```

odw needs the `pandacode` binary for real runs (`--backend pandacode`). For
token-free dry runs you need nothing extra (`--backend mock`).

## 2. Write a workflow

A workflow is a JS file: a literal `meta`, then top-level async code. The script
runs in a deterministic sandbox — see the gotchas.

```js
export const meta = {
  name: "implement-feature",
  description: "Implement X across N files",
  phases: [{ title: "Implement" }, { title: "Verify" }],
};

phase("Implement");
const impl = await agent(
  "Read SPEC.md and implement it; run the tests and confirm they pass.",
  { runtime: "codex", isolation: "worktree", label: "impl" }
);
// no schema -> impl is the executor's final text (or { text, worktree } if a
// worktree captured changes).

phase("Verify");
const verdict = await agent(
  `Review this change for correctness:\n${impl.text ?? impl}`,
  { runtime: "codex", schema: { type: "object", required: ["passed"],
    properties: { passed: { type: "boolean" }, notes: { type: "string" } } } }
);
return { ok: verdict.passed === true, verdict };
```

### The API (full types: `.odw/framework/workflow-api.d.ts`)

- `agent(prompt, opts) → Promise<text | object | {ok:false,...}>`
  opts: `runtime` (`"codex"|"claude"|"bamboo"`), `schema` (inline JSON Schema **or**
  a path), `model`, `label`, `phase`, `isolation:"worktree"`, `maxAttempts` /
  `retry`, `timeout`, `effort`. Bamboo nodes (domestic models) use `provider` /
  `bambooProvider` (`deepseek`, `xiaomi`, `kimi`, `zhipu`, `minimax`, `qwen`,
  `stepfun`) and dispatch as `pandacode bamboo exec --provider <provider>`;
  provider is invalid for non-Bamboo runtimes.

  **Enable domestic models:** Bamboo needs the provider's API key in the
  environment — set the provider-specific var (`DEEPSEEK_API_KEY`, `KIMI_API_KEY`,
  `QWEN_API_KEY`, `ZHIPU_API_KEY`, `MINIMAX_API_KEY`, `XIAOMI_API_KEY`,
  `STEPFUN_API_KEY`) or the generic `PANDACODE_BAMBOO_API_KEY`. Without a key,
  the node returns `{ ok:false, error: "missing API key…" }`. `odw doctor` reports
  whether bamboo has a key. Then: `agent("…", { runtime:"bamboo", provider:"deepseek" })`.
- `parallel(thunks, opts?) → Promise<any[]>` — **barrier**; a thunk that throws
  becomes `null` (never rejects) → `.filter(Boolean)`.
- `pipeline(items, ...stages) → Promise<any[]>` — **no barrier**; each stage gets
  `(prev, item, index)`; a stage throw drops that item to `null`.
- `fanout(items, mapper, opts?)` — convenience over `parallel`.
- `budget` — `{ total, spent(), remaining() }`; `remaining()` is `Infinity` when
  no total; once `spent() >= total` the next `agent()` throws.
- `workflow(nameOrRef, args)` — run a saved/sibling workflow inline (1 level deep).
- `phase(title)`, `log(msg)`, `checkpoint(name, value?)`, `promptSlot(...)`.
- `args` / `input` (the `--input` payload), `odw` (run metadata), `pandacode`
  (`.codex(prompt)` / `.claude(prompt)` / `.bamboo(prompt, { provider })` /
  `.exec(runtime, prompt)`).

### Patterns

```js
// Fan out independent edits, each isolated in its own worktree, collect diffs:
const results = await parallel(TASKS.map((t) => () =>
  agent(t.prompt, { runtime: "codex", isolation: "worktree", label: t.id })));

// Pipeline: implement -> verify, per item, no barrier between stages:
const out = await pipeline(items,
  (it) => agent(`Implement ${it.name}`, { runtime: "codex", schema: RESULT }),
  (impl, it) => agent(`Verify ${it.name}: ${impl.summary}`, { runtime: "codex", schema: VERDICT }));

// Bamboo domestic provider node:
const cn = await agent("Implement the requested change.", {
  runtime: "bamboo",
  provider: "deepseek",
  label: "bamboo-impl"
});

// Heterogeneous fan-out — each node a DIFFERENT model, then reconcile (this is
// odw's edge over the built-in tool; the report shows each node's real model):
const takes = await parallel(
  ["deepseek", "qwen", "kimi"].map((p) => () =>
    agent(QUESTION, { runtime: "bamboo", provider: p, label: `ask:${p}` })));
const best = await agent(`Reconcile:\n${takes.join("\n\n")}`, { runtime: "claude" });

// Budget-bounded loop (scale work to a token target):
while (budget.total && budget.remaining() > 50_000) {
  const r = await agent("Find one more bug.", { schema: BUG });
  if (!r.bug) break;
}
```

## 3. Run it

```bash
# Token-free dry run first — proves the orchestration without spending:
odw exec --script wf.js --backend mock --json

# One-command execution graph preview from a mock dry run:
odw report --script wf.js --open

# Real run through PandaCode:
odw exec --script wf.js --backend pandacode --json
# (--json prints only the workflow's return value; drop it to watch live progress)
```

- The workflow's `return` value is printed as `[result] <json>` (or the sole
  output under `--json`). Returning `{ ok:false, ... }` makes `odw exec` exit
  non-zero — usable as a CI/script gate.
- Inspect a run: `odw runs list`, `odw runs show <id>` (journal at
  `.odw/runs/<id>/events.jsonl`).
- `--resume <id>` re-runs a script; completed nodes return cached results.

## 4. Gotchas (read before your first run)

- **Determinism:** inside a workflow, `Date.now()`, `Math.random()`, and argless
  `new Date()` THROW (they break resume). Deterministic forms (`new Date(ts)`,
  other `Math.*`) work. Pass any timestamp/seed via `args`.
- **Worktree needs committed files:** `isolation:"worktree"` branches from HEAD,
  so **commit or stage** any spec/fixture the agent must read first. The captured
  diff comes back in `result.worktree`.
- **Schema vs no schema:** no schema → final **text string**; schema → validated
  **object** (the node still does its real work first). Use a schema when you need
  to branch on structured fields.
- **Failure is data:** a node that exhausts retries returns
  `{ ok:false, error:{ category, ... } }` — it does **not** throw, so it stays
  truthy and `.filter(Boolean)` keeps it. Drop failed nodes with
  `.filter(r => r && r.ok !== false)`. (Only a thunk that *throws* becomes `null`.)
- **Bamboo is a coding agent:** great for file edits / commands. Its result is a
  prose *summary* of what it did, not raw content — so it is a poor fit for nodes
  whose value IS the answer: prose deliverables (reviews, analyses) and `schema:`
  structured-output nodes both tend to fail (verified: a schema'd bamboo classify
  node returned "...negative with score 0.95..." as prose and missed the schema 3x).
  A schema does NOT fix this. Route answer-shaped nodes to `runtime:"claude"` /
  `"codex"`; keep bamboo for the file/command work it's built for.
- **Concurrency:** `parallel`/`pipeline` cap at `min(16, cores-2)`; a 1000-agent
  per-run backstop guards runaway loops.

## Reference

- API types: `.odw/framework/workflow-api.d.ts`
- Authoring contract: `.odw/framework/runtime-contract.md`
- Examples: `examples/` in the odw repo (`mock`-runnable)
- `odw <command> --help` for every flag (exec / runs / init / doctor / validate).
