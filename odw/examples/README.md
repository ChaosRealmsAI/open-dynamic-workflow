# odw examples

Runnable example workflows. Each runs token-free on the `mock` backend (no
executor needed) so you can see the orchestration shape, then for real on
`pandacode`.

```bash
# Dry run (token-free): prints the workflow's return value, no AI is called.
odw exec --script examples/01-single-node.js --backend mock --json

# Preview the execution graph from a mock dry run.
odw report --script examples/01-single-node.js --open

# Print the large-project starter from an installed `odw` binary.
odw starter parallel-review-apply > wf.js

# Real run (needs `pandacode` on PATH, or ODW_PANDACODE_BIN set):
odw exec --script examples/01-single-node.js --backend pandacode --json
```

| File | Shows |
|---|---|
| `01-single-node.js` | one `agent` node; no-schema → returns final text |
| `02-parallel-fanout.js` | `parallel` barrier + `isolation:"worktree"` for concurrent edits |
| `03-pipeline-verify.js` | `pipeline` (no barrier) implement → verify, `schema` verdict, CI-gate exit |
| `04-bamboo-provider.js` | Bamboo domestic-provider dispatch with `runtime:"bamboo"` and `pandacode.bamboo(...)` |
| `05-heterogeneous-models.js` | fan one question across several different models in parallel (deepseek/qwen/kimi), reconcile with claude — ODW's heterogeneous-executor edge |
| `06-build-project.js` | build a real project end-to-end: codex implements → claude reviews → codex fixes + runs the test command until green (the dogfood KV-store / roman-numeral shape) |
| `07-parallel-review-apply.js` | large-project parallel landing: Codex worktrees → `reviewWorktreeDiffs` candidate workspace → bounded repair/re-review on reject → approve-only atomic `applyWorktreeDiffs` → read-only verification guard |

Real `worktree` runs require `cwd` to be a git repository, and any spec/fixture
the agent must read should be committed first (the worktree branches from HEAD).
Example 07 intentionally lands approved changes into `cwd`; run it from the
target project or a disposable git repo. It treats caller-supplied context and
task prompts as owner intent, repairs blocker-matched tasks up to
`args.maxReviewRounds` (default 2 for small batches and 3 for 3+ tasks), falls back to full-batch repair when blockers
cannot be mapped to task files, stops for `needs_owner`, and treats final
verification as read-only: any post-approval file mutation fails the run and is
restored from the pre-verification snapshot.
Each task must declare a stable unique `id`; ODW uses task ids for node keys,
sessions, repair history, and reports.
Before review, it also blocks failed implementation nodes and cross-owned file
edits. Declare one `task.file` or multiple `task.files` for each task; set
`strictTaskFileBoundaries:false` only with explicit owner intent.
Because isolated worktrees branch from `HEAD`, it also blocks dirty declared
task files before implementation; commit/stash them first, or set
`allowDirtyTaskFiles:true` only when the owner accepts that workers will not see
those uncommitted changes.
It also blocks duplicate declared ownership of the same file; merge those tasks,
run them serially, or set `allowDuplicateTaskFiles:true` only when overlapping
patches are intentional and reviewable.

See `odw guide` for the full authoring guide and `odw spec` for the typed API.
