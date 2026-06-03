# Open Dynamic Workflow (odw) — agent usage guide

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

**Zero install.** There is nothing to scaffold into your project — this guide and
`odw --help` / `odw exec --help` / `odw spec` / `odw contract` are everything any
agent needs. Just have the `odw` binary on PATH (and `pandacode` for real runs).

## 1. Is odw available?

Run `odw --version`. For real runs odw needs the `pandacode` binary
(`--backend pandacode`); set `ODW_PANDACODE_BIN=/path/to/pandacode` if it is not on
PATH. For token-free dry runs you need nothing extra (`--backend mock`). Check the
wiring with `odw doctor` (verifies runtimes + binaries; exits non-zero if unhealthy).

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
  { runtime: "claude", schema: { type: "object", required: ["passed"],
    properties: { passed: { type: "boolean" }, notes: { type: "string" } } } }
);
return { ok: verdict.passed === true, verdict };
```

### The API (full TypeScript types: `odw spec`)

- `agent(prompt, opts) → Promise<text | object | {ok:false,...}>`
  opts: `runtime` (`"codex"|"claude"|"bamboo"`), `schema` (inline JSON Schema **or**
  a path), `model`, `label`, `phase`, `isolation:"worktree"`, `maxAttempts` /
  `retry`, `timeout`, `effort`. Bamboo nodes (domestic models) use `provider`
  (`deepseek`, `xiaomi`, `kimi`, `zhipu`, `minimax`, `qwen`, `stepfun`) and
  dispatch as `pandacode bamboo exec --provider <provider>`; provider is invalid
  for non-Bamboo runtimes.

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
  no total; once `spent() >= total` the next `agent()` throws. (Counts TOTAL
  tokens, not output-only — size budgets accordingly.)
- `workflow(nameOrRef, args)` — run a saved/sibling workflow inline (1 level deep).
- `reviewWorktreeDiffs(results, opts?)` — review captured worktree patches before
  landing. It first preflights the combined patch without mutating cwd, then runs
  one or more reviewer agents inside a temporary candidate worktree where the
  combined diff is already applied. It returns
  `decision:"approve"|"reject"|"needs_owner"`. Only `approve` has
  `applyReady:true`; `needs_owner` is where product/owner comments and decision
  gates belong.
- `applyWorktreeDiff(result)` / `applyWorktreeDiffs(results)` — apply captured
  `result.worktree` patches back to the main cwd. A batch is atomic by default:
  ODW checks the combined patch first, then applies it as one patch. Conflicts
  return `{ ok:false, error:{ category:"patch_conflict" } }` without mutating
  files. Use `continueOnError:true` only when partial landing is intentional.
- `phase(title)`, `log(msg)`, `checkpoint(name, value?)`, `promptSlot(...)`.
- `args` / `input` (the `--input` payload), `odw` (run metadata:
  `{ backend, runId, runDir, statePath, resumeFrom }`), `pandacode`
  (`.codex(prompt)` / `.claude(prompt)` / `.bamboo(prompt, { provider })` /
  `.exec(runtime, prompt)`).

### Patterns

```js
// Fan out independent edits, each isolated in its own worktree, collect diffs:
const results = await parallel(TASKS.map((t) => () =>
  agent(t.prompt, { runtime: "codex", isolation: "worktree", label: t.id })));
const gate = await reviewWorktreeDiffs(results, {
  label: "batch-gate",
  reviewerCount: 2,
  context: "Owner accepts only low-decision-cost changes with evidence."
});
if (!gate.applyReady) return { ok: false, gate };
const landed = applyWorktreeDiffs(results); // atomic by default; use after review
if (!landed.ok) return { ok: false, landed };

// Pipeline: implement -> verify, per item, no barrier between stages:
const out = await pipeline(items,
  (it) => agent(`Implement ${it.name}`, { runtime: "codex", schema: RESULT }),
  (impl, it) => agent(`Verify ${it.name}: ${impl.summary}`, { runtime: "claude", schema: VERDICT }));

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

# Print the reusable large-project starter:
odw starter parallel-review-apply > wf.js

# Real run through PandaCode:
odw exec --script wf.js --backend pandacode --json
# (--json prints only the workflow's return value; drop it to watch live progress)
```

- `parallel-review-apply` is the default large-project shape: independent Codex
  worktrees, a candidate-worktree review gate, bounded repair/re-review
  (`args.maxReviewRounds`, default 2 for small batches and 3 for 3+ tasks),
  approve-only atomic landing, then final
  verification. Repair targets blocker-matched task files when possible and
  falls back to full-batch repair when blockers are ambiguous. It stops instead
  of landing on `needs_owner`. Final verification is guarded by a main-worktree
  snapshot; if the verifier modifies files after approval, the run restores
  those unapproved changes and fails instead of silently bypassing review.
  By default each task is expected to stay inside `task.file` / `task.files`;
  failed implementation nodes or cross-owned file edits are repaired before any
  review/apply gate runs. Set `strictTaskFileBoundaries:false` only when the
  owner explicitly wants cross-file task overlap.
  Because isolated worktrees branch from `HEAD`, the starter also refuses to run
  when declared task files already have uncommitted changes; commit/stash them
  first, or pass `allowDirtyTaskFiles:true` only when the owner accepts that
  workers will not see those dirty changes.
  It also blocks duplicate declared ownership of the same file; merge those
  tasks, run them serially, or pass `allowDuplicateTaskFiles:true` only when
  overlapping patches are intentional and reviewable.
- The workflow's `return` value is printed as `[result] <json>` (or the sole
  output under `--json`). Returning `{ ok:false, ... }` makes `odw exec` exit
  non-zero — usable as a CI/script gate.
- Inspect a run: `odw runs list`, `odw runs show <id>` (journal at
  `.odw/runs/<id>/events.jsonl`).
- `--resume <id>` re-runs a script; unchanged completed nodes return cached
  results (editing a node's prompt re-runs it).

## 4. Gotchas (read before your first run)

- **Determinism:** inside a workflow, `Date.now()`, `Math.random()`, and argless
  `new Date()` THROW (they break resume). Deterministic forms (`new Date(ts)`,
  other `Math.*`) work. Pass any timestamp/seed via `args`.
- **Worktree needs committed files:** `isolation:"worktree"` branches from HEAD,
  so **commit** any spec/fixture the agent must read first. The captured
  diff comes back in `result.worktree` (always present on a worktree node).
- **Schema vs no schema:** no schema → final **text string**; schema → validated
  **object**. Schema validation retries only if you set `retry`/`maxAttempts`
  (default is one attempt — unlike the built-in tool, which auto-retries). An
  unloadable schema path fails fast with `schema_load_error`.
- **Mock dry runs differ from real:** `--backend mock` has no executor, so a
  no-schema node returns a small *status object* (NOT final text). Nodes using
  ODW's packaged schemas return schema-valid synthetic objects so built-in
  starter flows can be dry-run and graphed without fake schema failures. For
  custom schemas, design the workflow to tolerate synthetic/mock values or run a
  real `--backend pandacode` pass before trusting the content. In a dry run,
  coerce no-schema results defensively
  (`typeof x === "string" ? x : x.text ?? JSON.stringify(x)`). Use mock to prove
  the *graph shape* (parallel/pipeline/phases) and packaged-schema wiring, not
  the semantic quality of node outputs.
- **Failure is data:** a node that exhausts retries returns
  `{ ok:false, error:{ category, ... } }` — it does **not** throw, so it stays
  truthy and `.filter(Boolean)` keeps it. Drop failed nodes with
  `.filter(r => r && r.ok !== false)`. (Only a thunk that *throws* becomes `null`.)
- **Bamboo is a coding agent:** great for file edits / commands. Its result is a
  prose *summary* of what it did, not raw content — a poor fit for nodes whose
  value IS the answer: prose deliverables and `schema:` structured-output nodes
  both tend to fail (a schema does NOT fix this). Route answer-shaped nodes to
  `runtime:"claude"` / `"codex"`; keep bamboo for the file/command work.
- **Concurrency:** `parallel`/`pipeline` cap at `min(16, cores-2)`; a 1000-agent
  per-run backstop guards runaway loops.

## More

- `odw spec` — the framework spec + TypeScript API types.
- `odw contract` — the full workflow authoring contract.
- `odw capabilities` — machine-readable capability map.
- `odw <command> --help` — every flag (exec / report / runs / doctor).
