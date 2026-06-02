# odw examples

Runnable example workflows. Each runs token-free on the `mock` backend (no
executor needed) so you can see the orchestration shape, then for real on
`pandacode`.

```bash
# Dry run (token-free): prints the workflow's return value, no AI is called.
odw exec --script examples/01-single-node.js --backend mock --json

# Preview the execution graph from a mock dry run.
odw report --script examples/01-single-node.js --open

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

Real `worktree` runs require `cwd` to be a git repository, and any spec/fixture
the agent must read should be committed first (the worktree branches from HEAD).

See `skills/odw/SKILL.md` for the full authoring guide and
`.odw/framework/workflow-api.d.ts` (after `odw init`) for the typed API.
