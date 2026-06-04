# ODW Codex Dogfood Quality Record

Run: `/tmp/odw-dogfood-isolated-wUVV4h/.odw/runs/odw-exec-1780586920954-61912`

Scoring rubric: evidence density (0-3), verification/reproduction (0-2), novelty/risk contribution (0-2), correction/second-pass value (0-2), useful detail bonus (0-1). The score is a practical quality signal for dogfood iteration, not a model benchmark.

Summary:
- 36 attempted nodes: 33 successful Codex nodes, 3 blocked Bamboo domestic-model nodes.
- Average successful Codex node quality: 7.24/10.
- Best quality pattern: focused recon followed by explicit verify nodes that challenged overstatements.
- Weakest quality pattern: Bamboo high/low domestic trials could not run because no API key was configured.
- Product bug found by quality logging: concurrent agent bookkeeping reused the global `agentIndex` at completion time, so successful parallel nodes had duplicate `state.agents[*].index` values (`24` and `34`). Fixed by capturing a per-call local index.
- Product improvement found by model trial failures: ODW should preflight Bamboo API key availability before dispatch. Fixed by returning structured `state: "blocked"` / `category: "bamboo_missing_api_key"` before PandaCode spawn.

Post-review iteration:
- First codexctl review found that the initial preflight could falsely block default Bamboo runs with only `DEEPSEEK_API_KEY`, could let unknown explicit providers skip the gate, and could still overwrite raw reports for same-session exec/answer actions.
- The final implementation aligns no-provider Bamboo with PandaCode's default `deepseek`, blocks unknown providers as `bamboo_unknown_provider`, and appends the PandaCode action to raw report filenames.
- Added selftest coverage for missing-key preflight, default Deepseek dispatch, unknown-provider blocking, and raw report no-overwrite behavior.

| # | node | status | score | quality note |
|---:|---|---|---:|---|
| 1 | bamboo-entry-high-qwen | blocked | 0 | High-quality domestic entry could not run; missing Bamboo API key. |
| 2 | bamboo-exec-low-qwen | blocked | 0 | Low-cost domestic execution could not run; same missing key. |
| 3 | bamboo-exit-high-kimi | blocked | 0 | High-quality domestic exit could not run; same missing key. |
| 4 | codex-entry | ok | 8 | Good project map and parallel audit dimensions; included validator. |
| 5 | recon-package-surface | ok | 6 | Accurate dependency/script surface, but mostly descriptive. |
| 6 | recon-cli-contract | ok | 8 | Strong README/code contract evidence and runtime behavior summary. |
| 7 | recon-parser | ok | 6 | Good parser edge-case inventory, no direct reproduction. |
| 8 | recon-formatter | ok | 6 | Useful output-format evidence, limited severity analysis. |
| 9 | recon-storage | ok | 6 | Correct storage assumptions, mostly code inspection. |
| 10 | recon-errors | ok | 8 | Clear failure-mode inventory and test gap evidence. |
| 11 | recon-tests-unit | ok | 6 | Narrow coverage described accurately. |
| 12 | recon-tests-integration | ok | 7 | Good subprocess/exit-code gap analysis. |
| 13 | recon-docs-readme | ok | 6 | Accurate docs gap list, no runtime proof. |
| 14 | recon-docs-examples | ok | 8 | Good alignment between docs, implementation, and tests. |
| 15 | recon-security-paths | ok | 8 | Strong mismatch between README locality claim and unchecked `TASKBOARD_DB`. |
| 16 | recon-data-model | ok | 6 | Good schema assumptions, no malformed-data reproduction. |
| 17 | recon-performance | ok | 5 | Directionally right, but low user impact proof. |
| 18 | recon-concurrency | ok | 8 | Highest-value recon: reproduced silent lost updates with 20 processes. |
| 19 | recon-observability | ok | 8 | Good command/logging evidence and actionable observability gap. |
| 20 | recon-migration | ok | 6 | Correct versioning gap, mostly static. |
| 21 | recon-ux-new-user | ok | 6 | Useful first-run gap, no user trace. |
| 22 | recon-ux-failure | ok | 5 | Directional, but mostly duplicates error-lane evidence. |
| 23 | recon-maintainability | ok | 6 | Balanced module-boundary analysis. |
| 24 | recon-release | ok | 8 | Clear manifest evidence and release blocker. |
| 25 | pipe-cli-flow-inspect | ok | 6 | Found real test gap, but needed challenge pass. |
| 26 | pipe-task-store-inspect | ok | 6 | Correct but slightly overstated without reproduction. |
| 27 | pipe-reporting-inspect | ok | 6 | Actionable UX concern, severity under-evidenced. |
| 28 | pipe-docs-contract-inspect | ok | 5 | Weaker because installable CLI requirement was assumed. |
| 29 | pipe-quality-gates-inspect | ok | 6 | Correct lack of layered gates, overstated current tests. |
| 30 | pipe-cli-flow-verify | ok | 10 | Excellent correction: separated real test gap from unsupported claims. |
| 31 | pipe-task-store-verify | ok | 10 | Good second-pass calibration; asked for malformed/concurrent proof. |
| 32 | pipe-reporting-verify | ok | 10 | Good severity correction and test-contract awareness. |
| 33 | pipe-docs-contract-verify | ok | 9 | Strongly corrected unsupported packaging claim. |
| 34 | pipe-quality-gates-verify | ok | 10 | Best calibration of existing tests vs missing error-path coverage. |
| 35 | codex-synthesis | ok | 10 | Correctly compared ODW vs direct codexctl and selected preflight fix. |
| 36 | codex-exit-review | ok | 10 | High-quality exit: named evidence needed and product improvement. |

Iteration conclusion:
- Quality improved when the workflow forced `inspect -> verify -> synthesize`; verify nodes materially reduced overclaiming.
- The most valuable single node was not a high-effort model call; it was a low-effort focused concurrency recon that performed a reproduction.
- For future quality gains, route cheap/low-effort agents to narrow evidence collection, then require high-quality entry/exit reviewers to reject unsupported severity claims.
