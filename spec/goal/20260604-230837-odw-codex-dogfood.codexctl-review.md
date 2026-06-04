# Read-Only Review Prompt: ODW Codex Dogfood Fixes

You are reviewing local uncommitted changes in `/Users/Zhuanz/workspace/odw-oss`.

Review stance:
- Be strict. Report only concrete bugs, regressions, false positives/negatives, missing tests, or unsafe assumptions.
- Do not edit files.
- Focus on changed code and committed-intent artifacts, not style preference.

Context:
- A real ODW PandaCode run dogfooded 36 nodes in an isolated workspace: 33 Codex nodes succeeded, 3 Bamboo domestic-model nodes failed due missing API keys.
- The real run also revealed duplicate `state.agents[*].index` values for concurrent nodes because completed agents read the global `agentIndex`.
- Implemented fixes:
  - `odw/src/pack/templates/runtime/odw-js-runner.mjs` now captures a local `index` per `agent()` call.
  - Bamboo nodes now run `bambooApiKeyPreflight(...)` before writing prompt files or spawning PandaCode, returning `ok:false`, `state:"blocked"`, and `error.category:"bamboo_missing_api_key"` when no env/config key is available.
  - PandaCode raw report writes now receive fallback `session`/`label` context to avoid report filename collisions when downstream reports omit session.
  - `odw/scripts/selftest.mjs` fake Bamboo success-path tests now set `PANDACODE_BAMBOO_API_KEY=fake-key`.
  - Goal/spec evidence lives under `spec/goal/`.

Please inspect:
- `git diff -- odw/src/pack/templates/runtime/odw-js-runner.mjs odw/scripts/selftest.mjs odw/src/main.rs spec/goal`
- Relevant existing tests in `odw/scripts/selftest.mjs` and `odw/tests/parity_selftest.rs`

Validation already run:
- `cargo fmt --check` passed.
- `npm test` passed in `/tmp/odw-dogfood-isolated-wUVV4h`.
- `cargo run -p open-dynamic-workflow -- exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-bamboo-preflight-smoke.js --backend pandacode --json` passed with structured blocked result and no prompt/raw report.
- `cargo run -p open-dynamic-workflow -- exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-parallel-index-smoke.js --backend mock --json` passed with indexes `1,2,3,4,5`.
- Full mock dogfood workflow passed and had no duplicate indexes.
- `cargo test` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.

Questions:
1. Can Bamboo preflight incorrectly block a valid configured Bamboo run?
2. Can Bamboo preflight incorrectly allow an invalid run that should be blocked earlier?
3. Are there remaining paths where agent state/events still use global `agentIndex` instead of the local call index?
4. Does raw report fallback create stable unique paths without hiding real sessions?
5. Are selftest changes sufficient, or is there a missing regression test for the new preflight behavior?

Return:
- Findings first, with file/line references and severity.
- If no findings, say so clearly and note residual risk.
