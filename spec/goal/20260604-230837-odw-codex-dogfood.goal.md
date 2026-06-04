# Goal Spec: ODW Codex backend dogfood

Created: 2026-06-04 23:08:37 CST
Spec file: `spec/goal/20260604-230837-odw-codex-dogfood.goal.md`
Expected working directory: `/Users/Zhuanz/workspace/odw-oss`

## Goal Command

```text
/goal Read and execute @spec/goal/20260604-230837-odw-codex-dogfood.goal.md as the source of truth.
Expected working directory: /Users/Zhuanz/workspace/odw-oss. If the run starts from another directory, resolve the Goal file path before continuing.
At the start of the run, after every context compact/compaction, after every resume, and whenever the next action is uncertain, reopen and reread @spec/goal/20260604-230837-odw-codex-dogfood.goal.md before continuing.
Follow the Outcome, Done Criteria, Dynamic Scope, Plan, Self-Validation Harness, Iteration Policy, Stop Conditions, and Final Report Requirements in that file.
Mandatory gates: write/confirm a plan before implementation, implement self-validation, make atomic git commits for completed milestones, and run a codexctl read-only review before final completion.
Keep the Progress Log, Goal History, Decision Log, and Evidence Log updated in the file when meaningful progress, decisions, or validation results occur.
Work in phases: discover, plan, implement, self-validate, review, iterate or pivot, then final audit.
Stop only as Complete, Blocked, or Budget-limited according to the spec; do not loop after completion or after a stop condition is reached.
```

## Artifact Boundaries

- This `.goal.md` is the source-of-truth Goal contract and execution history.
- Progress, decisions, evidence, atomic commits, codexctl review findings, compact/resume rereads, blocked reasons, and final status updates must be written back to this `.goal.md`.
- `*.codexctl-review.md` files are review prompts only. They must not redefine the Goal objective, Done Criteria, scope, or stop conditions.
- The dogfood workflow, comparison scripts, generated reports, and validation artifacts may live under `spec/goal/` or `odw/docs/examples/` only when they are intentionally committed as reusable evidence or regression coverage.

## Original Request

User intent, preserving the user's wording and direction:

> 不 你真实做 后台接入codex 就做复杂的任务 超级复杂 多步骤 并行穿行 多步骤   看看 跟 直接用codexctl是否会提升效果。 你做个goal md 吧 就是让你体验迭代体验迭代 至少 30轮。 改 owd 注意提交代码。

Interpreted correction: `owd` means `odw`.

## Rough Goal Brief

Final target:
- Real-dogfood ODW's `--backend pandacode` Codex path with a complex multi-step parallel and serial workflow, compare the user experience and evidence against direct `codexctl`, implement any high-value ODW improvements discovered, validate them, and commit the changes.
- Extend the dogfood to include Bamboo domestic-model lanes: high-quality domestic models for entry/planning and exit/review, plus low-cost or weaker domestic models for execution lanes when credentials are available.

Highest-ROI route:
- Build a bounded dogfood workflow that forces at least 30 real Codex-backed ODW node invocations or recorded iteration events, with `parallel`, `pipeline` or equivalent fanout, resume/report inspection, and structured result logging; run a direct `codexctl` baseline for a comparable task; convert observed ODW friction into a small code or documentation improvement with tests.
- Run the dogfood in an isolated git workspace rather than the ODW source tree, so workers can safely read/write without risking source changes. Use the ODW repo only for the workflow harness, evidence logs, and eventual product improvement.

Project evidence used:
- `README.md` describes ODW orchestration and PandaCode execution split.
- `odw/README.md` documents zero-install `odw exec`, reports, workflow API, and `pandacode` backend.
- `odw/src/main.rs` defines `doctor`, `exec`, `report`, `runs`, and the `--backend mock|pandacode` CLI.
- `odw/src/pack/templates/runtime/odw-js-runner.mjs` contains workflow runtime behavior, agent caching, schema handling, parallel/fanout helpers, budget handling, and PandaCode dispatch.
- `pandacode/src/runtimes/codex.rs` shows Codex execution through `codexctl session start/execute`, per-session sockets, logs, resume, answer, status, and daemon cleanup.
- `odw/examples/07-parallel-review-apply.js` is the existing complex starter for parallel Codex worktrees, review gate, repair, landing, and verification.
- `cargo test`, `cargo fmt --check`, and `cargo clippy --workspace --all-targets -- -D warnings` already passed before this Goal was written.

Validation focus:
- Prove real Codex execution through ODW, not only mock mode, by recording ODW run ids, event counts, node count, runtime/model data, logs, report path, and elapsed/error behavior.
- Prove domestic-model coverage by recording Bamboo provider-key discovery and, if keys exist, at least one high-quality entry/exit Bamboo run and one low-cost execution Bamboo run. If keys are absent, record the exact `pandacode bamboo doctor` and attempted run failure as a blocked external-capability result.
- Prove the comparison to direct `codexctl` with a separate read-only baseline command and concise findings.
- Prove code changes through targeted tests plus full workspace checks.

Dynamic scope:
- The executing agent may inspect and change ODW orchestration, ODW runtime template, ODW examples/docs, and PandaCode Codex integration only when evidence from dogfood runs points there.
- It may add a reusable smoke/evidence script or dogfood workflow if that provides durable validation.

Stop boundaries:
- Stop as Blocked if Codex account quota, codexctl, Node, pandacode, git worktree safety, or clean commit requirements prevent real execution.
- Stop as Budget-limited if completing 30 real Codex-backed turns would consume disproportionate quota after clear evidence has already identified the ODW issue and the user has not authorized more spend.
- Stop before broad architecture changes, public API changes, auth/account changes, provider pricing changes, or unrelated PandaCode runtime refactors.

## Clarification Answers

Questions asked:
- None. The request was explicit enough to proceed: create a Goal spec, real-run ODW with Codex backend, do a complex multi-step parallel/serial dogfood run, compare against direct `codexctl`, modify ODW if evidence supports it, and commit code.

User answers:
- The user explicitly asked to proceed with real Codex-backed work instead of only static checks or mock runs.

Assumptions made:
- `owd` refers to `odw`.
- "至少 30 轮" means at least 30 counted dogfood iterations, where a counted round is an ODW-recorded real Codex-backed `agent()` node invocation or an execution-log iteration generated by the dogfood harness. Mock-only nodes do not count.
- The comparison target is practical user experience and operational evidence: reliability, observability, resumability, task decomposition, reporting, setup friction, and whether ODW improves orchestration over direct `codexctl` for multi-node work.
- Code should be committed in the existing Git repository at `/Users/Zhuanz/workspace/odw-oss`.

Open questions that should stop execution if blocking:
- Whether the user is willing to spend significant model quota if the first real run suggests 30 full Codex coding nodes would be excessive. If quota or rate limits appear, stop as Budget-limited with partial evidence instead of silently reducing real coverage.
- Whether Bamboo provider API keys can be provided if live domestic-model trials are mandatory. Current environment discovery found no `DEEPSEEK_API_KEY`, `KIMI_API_KEY`, `QWEN_API_KEY`, `ZHIPU_API_KEY`, `MINIMAX_API_KEY`, `XIAOMI_API_KEY`, `STEPFUN_API_KEY`, or `PANDACODE_BAMBOO_API_KEY`.

## Project Recon And Plan Basis

Repository/root or artifact location inspected:
- `/Users/Zhuanz/workspace/odw-oss`

Project instructions read:
- No `AGENTS.md` was found within this repository using `rg --files -g 'AGENTS.md'`.
- Read top-level `README.md`, `odw/README.md`, `pandacode/README.md`, the before-goal skill instructions, and the Goal spec template.

Key files, directories, modules, APIs, tests, configs, or scripts inspected:
- `Cargo.toml`, `odw/Cargo.toml`, `pandacode/Cargo.toml`
- `odw/src/main.rs`
- `odw/src/pack/templates/runtime/odw-js-runner.mjs`
- `pandacode/src/runtimes/codex.rs`
- `odw/examples/01-single-node.js`
- `odw/examples/07-parallel-review-apply.js`
- `odw/tests/parity_selftest.rs`
- `pandacode/tests/fake_runtimes.rs`

Discovery commands run:
- `git status --short`
- `rg --files -g '!target/**' -g '!.odw/**'`
- `rg --files -g 'AGENTS.md'`
- `sed -n` reads of the README, CLI, runtime template, Codex runtime, and example workflows
- `odw --help`, `pandacode --help`, `odw doctor --json`, `pandacode doctor --json`
- `cargo test`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `odw exec --backend mock` and `odw report --script` on `odw/examples/01-single-node.js`

Existing validators and commands discovered:
- `cargo test`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `odw exec --script <workflow> --backend mock --json`
- `odw exec --script <workflow> --backend pandacode --json`
- `odw report --script <workflow>` and `odw report --run latest`
- `odw runs list`, `odw runs show latest`
- `pandacode doctor --json`, `pandacode models --json`
- `codexctl plan`, `codexctl session start/execute/read/list/status`
- `pandacode bamboo doctor --json` and `pandacode bamboo models --json`

Existing test coverage and likely gaps:
- Rust unit tests cover ODW run journals, resume helper exposure, pruning, spec/capability surfaces, schema-free flexible nodes, and direct runner import blocking.
- PandaCode unit and fake runtime tests cover Codex/Claude/Bamboo command construction, sessions, answers, logs, stop/timeout behavior, prompt transport, provider inference, permissions, and agent tools.
- Gap: there is no committed high-stress real Codex dogfood harness that counts 30 ODW rounds, records observability artifacts, and compares the operator experience against direct `codexctl`.
- Gap: Bamboo can enumerate domestic models, but live domestic-model execution is configuration-blocked without provider API keys in this shell.

Relevant constraints, risky areas, and pre-existing unrelated changes:
- `git status --short` was clean before creating this spec.
- Real Codex use can consume quota and time.
- Bamboo live runs currently require missing provider keys; attempts should be recorded, then skipped or marked blocked rather than retried wastefully.
- ODW `--timeout` floors Codex nodes at 600 seconds in real coding paths, so long real nodes can overrun a short workflow timeout expectation.
- Worktree and path boundary logic must avoid `.git`, `.odw`, `.pandacode`, `node_modules`, absolute paths, and path escapes.
- Do not stage unrelated changes or generated transient `.odw` / `.pandacode` state unless intentionally committed evidence requires it.

Plan basis:
- The highest-ROI path is not a large architectural rewrite. It is a real ODW Codex dogfood run in an isolated workspace designed to expose practical friction, with Bamboo lanes attempted according to available credentials, followed by a small improvement and durable validation. Existing tests are already strong for mocked/fake behavior; the missing evidence is real multi-node Codex orchestration, domestic-model capability gating, and operator comparison.

## Outcome

ODW is improved or explicitly validated based on a real isolated-workspace dogfood run: at least 30 counted ODW/Codex rounds are evidenced, domestic Bamboo high-quality and low-cost lanes are attempted and either executed or blocked with exact credential evidence, a direct `codexctl` baseline is recorded, any high-value ODW friction found is addressed with scoped code/docs/tests, all required validators pass, an independent `codexctl` review is completed, and the final changes are committed atomically.

## Reasoning Brief

- Interpreted intent: the user wants a real, high-stress ODW experience report and practical product improvement, not a superficial mock check.
- Assumptions: `owd` is `odw`; counted rounds must be real enough to exercise the ODW-to-PandaCode-to-Codex path; domestic-model lanes are real only when provider credentials exist; the objective includes code commits when changes are made.
- Dynamic scope choices: start with a dogfood workflow and comparison script; only change runtime/CLI/docs/tests when evidence shows real friction.
- Main risks: model quota, long-running Codex nodes, noisy generated state, accidental unrelated commits, and conflating ODW orchestration improvements with PandaCode runtime internals.
- Strategy: instrument and run first, improve second, validate and review third, commit only coherent scoped changes.

## Dynamic Scope And Boundaries

Mission scope:
- Evaluate and improve ODW as a Codex-backed workflow orchestrator for complex parallel and serial tasks.
- Exercise ODW in a separate isolated project workspace so the dogfood workload can be complex and write-capable without touching the ODW source tree.

In scope:
- `odw/src/main.rs`
- `odw/src/pack/templates/runtime/odw-js-runner.mjs`
- `odw/src/guide.md`
- `odw/README.md`
- `odw/examples/`
- `odw/docs/examples/`
- `odw/tests/`
- `pandacode/src/runtimes/codex.rs` only when direct ODW evidence points to a Codex-runtime integration issue.
- `pandacode/tests/fake_runtimes.rs` only if a PandaCode-side fix is required.
- `spec/goal/20260604-230837-odw-codex-dogfood.goal.md` as execution log.
- A temporary isolated git workspace under `/tmp` or `/private/var/folders/...` for live dogfood runs.
- A task-specific `spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md`.

Out of scope:
- Changing public runtime semantics without test coverage.
- Broad refactors unrelated to dogfood evidence.
- Provider catalog/pricing updates.
- Claude or Bamboo runtime changes unless they are touched by shared code required for the ODW Codex path.
- Modifying user account config, Codex auth, Claude auth, MCP config, billing, or global shell profile.
- Fabricating Bamboo success when provider keys are missing.
- Committing transient `.odw/runs`, `.pandacode`, `target`, or temp artifacts unless deliberately curated as small documentation fixtures.

Discovery boundary:
- Search code/docs/tests within `/Users/Zhuanz/workspace/odw-oss`.
- Use `odw`, `pandacode`, and `codexctl` local CLI help and doctor output.
- Use internet only if a current external Codex/OpenAI behavior must be verified; prefer local CLI/docs and official OpenAI sources if that happens.
- Use subagents or direct `codexctl plan` for independent review; do not delegate uncontrolled write access to many agents editing the same files.

Scope expansion protocol:
- Record the evidence that requires expansion in the Decision Log.
- Identify newly affected files and validators before editing them.
- Stop for user input before changing public API contracts, security posture, account/auth behavior, deployment assumptions, or provider billing/model-selection rules.

Non-negotiable constraints:
- Use `rg`/targeted reads for exploration.
- Use `apply_patch` for manual edits.
- Do not revert user changes.
- Maintain ASCII in edited files unless existing file context requires otherwise.
- Make atomic commits for completed milestones.
- Run the mandatory codexctl read-only review before final completion.

## Plan And Approach Options

Candidate approaches:
- A: Add a small committed dogfood workflow/evidence harness, run it through ODW real Codex backend for 30 counted rounds, compare to direct codexctl, then improve the highest-friction issue with targeted tests.
- B: Extend ODW CLI with first-class dogfood or benchmark command if the run shows recurring manual friction that should be productized.
- C: Change PandaCode Codex runtime internals only if ODW dogfood evidence shows a concrete command, status, logging, transport, or daemon lifecycle defect there.

Chosen initial approach:
- Start with Approach A because it produces evidence quickly, keeps scope bounded, and preserves the option to pivot based on observed failures.

Pivot triggers:
- Pivot to B if the main friction is repeatable operator ergonomics in ODW CLI/report/run inspection.
- Pivot to C if the real run fails inside PandaCode Codex execution despite ODW dispatch behaving correctly.
- Record Bamboo as blocked, not failed ODW functionality, if the only issue is missing provider credentials.
- Stop as Budget-limited if 30 full Codex nodes are not feasible due to quota/time, after recording the exact blocker and the highest-fidelity partial evidence.

## Done Criteria

- [ ] At least 30 counted real ODW/Codex rounds are recorded with run id, event count, runtime/model evidence, and the counting method.
- [ ] Per-round quality is recorded or sampled with explicit criteria: file evidence, novelty, actionability, validation awareness, contradiction/review value, and repetition/boilerplate risk.
- [ ] The final analysis states how quality improved or could improve across iterations, including prompt changes, model placement, schema/score gates, and entry/exit review strategy.
- [ ] The dogfood workload uses both parallel and serial orchestration, with at least one fanout/parallel section and one downstream synthesis or verification section.
- [ ] A direct `codexctl` baseline for a comparable task is run and summarized against ODW on observability, resumability, setup friction, result quality, and failure/debug ergonomics.
- [ ] The real dogfood workload runs in an isolated git workspace outside the ODW source tree.
- [ ] Bamboo domestic-model lanes are attempted for high-quality entry/exit and low-cost execution; live success or missing-key blockage is recorded with evidence.
- [ ] At least one concrete ODW improvement is implemented, or the Decision Log records why no code change was justified after real evidence.
- [ ] Added or changed code/docs/tests are scoped to the dogfood findings and avoid unrelated refactors.
- [ ] `cargo fmt --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes.
- [ ] A targeted ODW smoke or regression validator exercises the changed behavior.
- [ ] Plan is written or confirmed before implementation and recorded in the Decision Log.
- [ ] Self-validation harness exists or is explicitly justified as not feasible, with evidence.
- [ ] Code changes have appropriate test coverage, including integration and end-to-end tests where behavior crosses boundaries or affects user workflows.
- [ ] Atomic git commits are created for completed implementation milestones without including unrelated user changes.
- [ ] Independent codexctl read-only review runs before final completion, and findings are resolved or explicitly accepted.

## Mandatory Gates

Planning gate:
- [ ] Write or confirm the implementation plan before editing.
- [ ] Record the chosen plan and pivot triggers in the Decision Log.

Self-validation gate:
- [ ] Design validation before implementation.
- [ ] Create or improve code-level validators where feasible.

Code test gate:
- [ ] Decide which unit, integration, and end-to-end tests are required for this task.
- [ ] Add or update integration tests when behavior crosses modules, subprocess boundaries, filesystem state, run journals, report rendering, or executor invocation.
- [ ] End-to-end browser tests are not required unless the work changes HTML report behavior; if report UI changes, inspect generated HTML and add the closest practical smoke check.
- [ ] If live Codex E2E cannot complete because of quota/time, record the exact limitation and create a deterministic smoke/contract substitute for the changed code.

Atomic commit gate:
- [ ] Record pre-edit `git status --short`.
- [ ] Identify pre-existing unrelated changes and avoid staging them.
- [ ] Commit after each completed implementation milestone or phase.
- [ ] Record commit hash, message, changed files, and validation evidence.

Codexctl review gate:
- [ ] Write a task-specific `spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md` review prompt.
- [ ] Run `codexctl plan --cwd /Users/Zhuanz/workspace/odw-oss --prompt-file spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md --sandbox read-only --approval-policy never --effort high --timeout unlimited`.
- [ ] Record review output summary and findings.
- [ ] Resolve or explicitly accept high/medium findings.
- [ ] Rerun validators and rerun codexctl review after review-driven code changes unless changes are documentation/log-only.

## Self-Validation Harness

Validation design:
- What must be proven: ODW can run a complex real Codex-backed workflow and yields better orchestration evidence than raw direct `codexctl`; any resulting change is correct and covered.
- Baseline evidence to collect: current `odw doctor`, `pandacode doctor`, `pandacode bamboo doctor`, provider-key discovery, `codexctl` baseline run, current test pass state, and real ODW run behavior before changes.
- Target evidence for completion: isolated workspace path, 30 counted rounds, ODW run/report/log artifacts, Bamboo attempted/live-or-blocked result, comparison summary, passing validators, commit hash, codexctl review findings addressed.
- Quality evidence: a per-node or sampled quality table rating evidence grounding, novelty, actionability, and verification strength; repeated low-quality patterns must feed into the ODW improvement decision.
- Continuous checks after changes: targeted ODW smoke/test for touched behavior, `cargo fmt --check`, and focused `cargo test -p <crate>` when possible.
- Final checks before completion: full `cargo test`, `cargo clippy --workspace --all-targets -- -D warnings`, final ODW smoke, `git status --short`, codexctl review.
- Test coverage decision for code changes: Rust unit tests for Rust logic; integration/fake-runtime tests for subprocess/session behavior; ODW mock smoke for workflow-runtime changes; live Codex evidence for user-experience dogfood but not as the only regression test.
- Inconclusive evidence rule: if live Codex cannot finish because of rate limits, auth, quota, or repeated transport failure, stop as Blocked or Budget-limited after preserving logs and adding deterministic coverage for any local code changes.

Existing validators:
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test`
- `odw exec --script odw/examples/01-single-node.js --backend mock --json`
- `odw report --script odw/examples/01-single-node.js`
- `pandacode doctor --json`
- `odw doctor --json`

Validators to create or improve:
- A committed dogfood workflow or harness that makes the 30-round ODW/Codex run repeatable enough to audit.
- A targeted deterministic test or smoke command for any ODW code improvement made.

Required evidence before completion:
- ODW real-run command, run id, counted round total, report path, and summary.
- Direct `codexctl` command and summary.
- Diff summary and commit hash.
- Validator command outputs summarized in Evidence Log.
- codexctl review command and finding summary.

## Atomic Commit Protocol

- Record `git status --short` before editing.
- Do not stage or commit unrelated user changes.
- Use path-specific staging when necessary.
- Keep each commit small, coherent, and reversible.
- Commit after each completed implementation milestone or phase.
- Run relevant validators before milestone commits unless intentionally committing a failing baseline/test.
- Record commit hash, message, changed files, and validation evidence in the Evidence Log.
- Stop as Blocked if clean atomic commits are impossible because of git state, unrelated changes, or missing git metadata.

## Codexctl Review Gate

Review prompt file:
- `spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md`

Required command:

```bash
codexctl plan --cwd /Users/Zhuanz/workspace/odw-oss --prompt-file spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md --sandbox read-only --approval-policy never --effort high --timeout unlimited
```

Review prompt requirements:
- The executing Codex must write this prompt based on this task, this `.goal.md`, current diff, commits, tests, and evidence.
- Read this `.goal.md`.
- Inspect current diff and relevant commits.
- Check bugs, regressions, missing unit/integration/end-to-end tests, scope creep, security/privacy issues, and completion evidence.
- Return prioritized findings with file references.
- Do not edit files.

Review completion rule:
- High/medium findings must be fixed or explicitly accepted with rationale.
- Validators must be rerun after review-driven changes.
- codexctl review must be rerun after review-driven code changes unless changes are documentation/log-only.

## Execution Phases

### Phase 0: Context Discovery And Baseline
- [ ] Inspect relevant files, docs, logs, repo instructions, ODW examples, PandaCode Codex runtime, and existing validators.
- [ ] Record current behavior, real-runtime availability, and baseline checks.
- [ ] Record direct `codexctl` baseline plan for comparison.
Exit criteria: baseline evidence and the first dogfood plan are recorded in the logs.
Stop if: Codex or codexctl is unavailable, auth is missing, or the task cannot be scoped without user input.

### Phase 1: Plan And Validation Design
- [ ] Compare approaches A/B/C and select the first implementation plan.
- [ ] Define the 30-round counting method and dogfood workload before running it.
- [ ] Define the isolated workspace path and Bamboo high-quality/low-cost trial matrix.
- [ ] Decide required unit, integration, and live-smoke coverage.
- [ ] Record the plan in the Decision Log before implementation.
Exit criteria: dogfood workload, counting method, validators, and pivot triggers are recorded.
Stop if: no credible validator or implementation path exists.

### Phase 2: Real ODW/Codex Dogfood
- [ ] Run the complex workflow through `odw exec --backend pandacode`.
- [ ] Ensure the workload includes both parallel and serial sections.
- [ ] Keep dogfood writes inside the isolated workspace.
- [ ] Attempt Bamboo high-quality entry/exit and low-cost execution lanes when credentials exist; otherwise record exact missing-key evidence.
- [ ] Collect run id, events, logs, status, report, runtime/model info, failures, and elapsed-time observations.
- [ ] Extract per-round quality signals from node results and record how prompt/model/orchestration choices affected quality.
- [ ] Count at least 30 real rounds or stop as Budget-limited with exact evidence.
Exit criteria: 30 counted rounds and comparison-ready evidence are recorded, or a bounded stop state is justified.
Stop if: quota/rate/time limits prevent meaningful continuation.

### Phase 3: Versioned Implementation
- [ ] Convert the highest-value dogfood finding into a small ODW improvement.
- [ ] Update or create tests/docs/harnesses with the change.
- [ ] Keep changes tied to the active hypothesis and dynamic scope.
- [ ] Create an atomic git commit after the milestone passes required validation.
Exit criteria: the core validator passes once and a commit exists.
Stop if: implementation requires an out-of-scope public API or runtime architecture change.

### Phase 4: Self-Validation And Review
- [ ] Run full relevant validators.
- [ ] Run required unit, integration, and smoke checks.
- [ ] Run mandatory codexctl read-only review using the task-specific prompt file.
- [ ] Fix regressions introduced by implementation.
Exit criteria: all Done Criteria pass and review findings are addressed or accepted.
Stop if: two consecutive attempts do not improve any Done Criterion.

### Phase 5: Iterate Or Pivot
- [ ] If evidence invalidates the plan, record why and choose the next bounded approach.
- [ ] Do not repeat the same hypothesis without new evidence.
Exit criteria: either Done Criteria pass or a new evidence-backed plan is selected.
Stop if: repeated pivots become speculative or exceed scope.

### Phase 6: Final Audit And Report
- [ ] Review the diff against dynamic scope and non-negotiable constraints.
- [ ] Confirm every Done Criterion has evidence.
- [ ] Confirm atomic commits and codexctl review evidence are recorded.
- [ ] Write the final report.
Exit criteria: final report is complete.
Stop if: any Done Criterion lacks evidence.

## Task Allocation

- Supervisor: Maintain this spec, checklist, stop rules, and progress log.
- Implementer: Make scoped code changes only after dogfood evidence supports them.
- Verifier: Run validators and record evidence.
- Reviewer: Use `codexctl plan` for independent read-only review before completion.

Subagent plan:
- Recon/search: use direct local commands first; use a read-only codexctl plan only for independent review or if context pressure grows.
- Implementation: keep implementation in the main agent unless the touched files become independent enough for a delegated isolated patch.
- Verification/review: mandatory codexctl review plus local validators.
- Merge rule: the supervisor records subagent or codexctl conclusions in this `.goal.md` and only applies evidence-backed findings.

## Iteration Policy

- Each counted iteration must name one hypothesis or next best action.
- Each counted iteration must change code, tests, evidence, or the plan.
- Each counted ODW/Codex round must be auditable through ODW events, PandaCode session records, or a committed dogfood summary.
- Each iteration must state whether the current plan is still valid or whether a pivot is needed.
- Update the Progress Log after each phase or meaningful attempt.
- Stop as Blocked if two consecutive implementation iterations do not improve any Done Criterion.

## Stop Conditions

Complete only when:
- [ ] Every Done Criterion is satisfied with recorded evidence.

Stop as blocked when:
- [ ] Required credentials, files, product decisions, or external systems are missing.
- [ ] The task requires scope expansion not authorized by this spec.
- [ ] The same validator fails for the same root cause after the allowed attempts.
- [ ] Two consecutive implementation attempts do not improve any Done Criterion.
- [ ] Atomic commits cannot be made safely.
- [ ] codexctl review is unavailable or cannot run.

Stop as budget-limited when:
- [ ] Budget, rate limits, time limits, or context limits prevent meaningful continuation of real Codex rounds after preserving available evidence.

## Progress Log

| Time | Phase | Action | Evidence | Status | Next |
| --- | --- | --- | --- | --- | --- |
| 2026-06-04 23:08 CST | Pre-goal | Created task-specific Goal spec after read-only recon | `spec/goal/20260604-230837-odw-codex-dogfood.goal.md` | In progress | Start Phase 0 baseline and real dogfood plan |
| 2026-06-04 23:22 CST | Phase 0 | Created isolated git fixture at `/tmp/odw-dogfood-isolated-wUVV4h`; initial `npm test` found a bad test regex, then fixture was fixed and tests passed | Fixture commits `7359f22`, `01ad349`; `npm test` passed 2 tests | In progress | Run ODW workflow mock and real backends |
| 2026-06-04 23:23 CST | Phase 1 | Mock ODW run found workflow authoring bug: bare `cwd` is not available; fixed script to use `globalThis.cwd` | Failed run `odw-exec-1780586618027-59338`; patched `spec/goal/odw-codex-30round-dogfood.js` | In progress | Rerun mock |
| 2026-06-04 23:26 CST | Phase 0 | Direct `codexctl` baseline completed in one read-only Plan turn; it found useful risks but also appeared to report stale/sandbox-skewed test evidence compared with current shell validation | Thread `019e933d-636c-74b3-84e6-e65b6fa810d2`; log `/Users/Zhuanz/.codexctl/logs/run-1780586732382.jsonl`; current `npm test` passes | In progress | Run real ODW backend |
| 2026-06-04 23:35 CST | Phase 2 | Real ODW PandaCode backend run completed with 36 attempted nodes and 33 successful Codex nodes | Run `odw-exec-1780586920954-61912`; report `/tmp/odw-dogfood-isolated-wUVV4h/.odw/runs/odw-exec-1780586920954-61912/report.html` | In progress | Analyze quality and implement evidence-backed ODW fixes |
| 2026-06-04 23:43 CST | Phase 3 | Implemented Bamboo missing-key preflight and raw report fallback context | `odw/src/pack/templates/runtime/odw-js-runner.mjs`; `odw/src/main.rs` assertions | In progress | Smoke-test preflight |
| 2026-06-04 23:44 CST | Phase 3 | Bamboo preflight smoke passed: missing Qwen key returns structured blocked result before executor dispatch | Run `odw-exec-1780587867364-79533`; no Bamboo prompt/raw report generated | In progress | Record quality and continue validation |
| 2026-06-04 23:48 CST | Phase 3 | Quality analysis found duplicate state indexes in concurrent nodes; fixed agent bookkeeping to capture local call index | Historical run had duplicate indexes `24` and `34`; mock smoke run `odw-exec-1780588130389-82632` now has indexes `1,2,3,4,5` | In progress | Run full validators and independent review |
| 2026-06-04 23:58 CST | Phase 4 | First codexctl review found 4 issues; fixed default Deepseek preflight, unknown-provider blocking, raw-report action suffixes, and preflight/raw-report regression tests | Thread `019e9359-2d66-7e82-bc6e-6615c3a60154`; log `/Users/Zhuanz/.codexctl/logs/run-1780588555545.jsonl` | In progress | Rerun validators and codexctl review |
| 2026-06-05 00:05 CST | Phase 4 | Final validators and second codexctl review passed | `cargo fmt --check`; fixture `npm test`; `cargo test`; `cargo clippy --workspace --all-targets -- -D warnings`; review thread `019e9360-8897-7370-ac34-c5a34d402175` | In progress | Commit changes |

## Goal History

| Time | Event | Summary | Evidence/Link |
| --- | --- | --- | --- |
| 2026-06-04 23:08 CST | Created | Goal spec created for real ODW Codex backend dogfood and improvement work | `spec/goal/20260604-230837-odw-codex-dogfood.goal.md` |
| 2026-06-04 23:22 CST | Isolated workspace created | Dogfood runs will target `/tmp/odw-dogfood-isolated-wUVV4h`, not the ODW source tree | `git log --oneline` in isolated workspace shows `01ad349`, `7359f22` |
| 2026-06-04 23:35 CST | Real 30+ round run completed | ODW orchestrated 3 Bamboo trials, 1 Codex entry, 20 parallel recon nodes, 10 pipeline nodes, synthesis, and exit review | Run `odw-exec-1780586920954-61912` |
| 2026-06-04 23:49 CST | Quality record created | Per-node quality table and iteration conclusions recorded | `spec/goal/odw-codex-dogfood-quality.md`; `spec/goal/analyze-odw-dogfood-quality.mjs` |

## Decision Log

| Time | Decision | Rationale | Evidence |
| --- | --- | --- | --- |
| 2026-06-04 23:08 CST | Treat `owd` as `odw` and proceed without clarification | The repo and prior context use ODW/Open Dynamic Workflow; user explicitly asked to proceed with code and commits | User request; repo path `/Users/Zhuanz/workspace/odw-oss` |
| 2026-06-04 23:08 CST | Start with Approach A | It provides direct evidence before changing code and keeps scope bounded | Existing ODW examples and validators already pass; missing piece is real Codex dogfood evidence |
| 2026-06-04 23:22 CST | Use isolated git workspace for live dogfood | User requested complex tasks and isolated directory; this allows real agent reads/writes without risking ODW source files | `/tmp/odw-dogfood-isolated-wUVV4h` |
| 2026-06-04 23:23 CST | Treat script-global ergonomics as a candidate ODW improvement | The workflow failed after doing all mock nodes because `cwd` was documented/available as `globalThis.cwd` but not as a lexical binding | Run `odw-exec-1780586618027-59338` |
| 2026-06-04 23:26 CST | Keep real ODW node prompts focused on source files | Direct baseline inspected `.odw` artifacts and produced a very large context; real ODW nodes should ignore `.odw/.pandacode` unless explicitly asked | Patched `projectContext()` in `spec/goal/odw-codex-30round-dogfood.js` |
| 2026-06-04 23:35 CST | Implement Bamboo preflight as the primary product fix | Three domestic-model lanes failed immediately with the same missing-key error and one raw report artifact was overwritten, proving ODW should block earlier with structured remediation | Run `odw-exec-1780586920954-61912`; raw report path collision `pandacode-bamboo-bamboo-exec.report.json` |
| 2026-06-04 23:48 CST | Also fix concurrent agent index bookkeeping | Quality analysis showed parallel state records reused completion-time global `agentIndex`, which weakens reports and per-node quality accounting | `spec/goal/analyze-odw-dogfood-quality.mjs`; historical duplicate indexes `24`, `34` |
| 2026-06-04 23:58 CST | Accept and fix all first-review findings | The findings were concrete and locally reproducible: preflight had false-positive/false-negative edges, raw report action collision remained possible, and tests missed the new blocked contract | codexctl thread `019e9359-2d66-7e82-bc6e-6615c3a60154` |
| 2026-06-05 00:04 CST | Treat final review residual config-layout risk as accepted | Second review found no concrete regression; remaining risk is future provider/config layout drift, which is outside this scoped fix and covered by alias/table tests for current repo behavior | codexctl thread `019e9360-8897-7370-ac34-c5a34d402175` |

## Evidence Log

| Time | Evidence Type | Command, Commit, File, Route, Or Artifact | Result |
| --- | --- | --- | --- |
| 2026-06-04 23:03 CST | Doctor | `odw doctor --json` | `ok: true`; Node, PandaCode, Codex, Claude, tmux available; Bamboo missing API key only |
| 2026-06-04 23:03 CST | Doctor | `pandacode doctor --json` | `ok: true`; Codex and Claude available; Bamboo provider key missing |
| 2026-06-04 23:04 CST | Test | `cargo test` | Passed: ODW tests, PandaCode unit tests, fake runtime integration tests |
| 2026-06-04 23:04 CST | Lint/format | `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| 2026-06-04 23:04 CST | Mock smoke | `odw exec --script odw/examples/01-single-node.js --backend mock --json` | Passed with `{ ok: true }` |
| 2026-06-04 23:04 CST | Report smoke | `odw report --script odw/examples/01-single-node.js --out <tmp>/report.html` | Wrote HTML report |
| 2026-06-04 23:21 CST | Bamboo config | `env | rg '^(DEEPSEEK|KIMI|QWEN|ZHIPU|MINIMAX|XIAOMI|STEPFUN|PANDACODE_BAMBOO)_API_KEY='`; `pandacode bamboo doctor --json` | No provider keys in shell; Bamboo state `configuration_needed`, missing `api_key` |
| 2026-06-04 23:22 CST | Isolated validator | `npm test` in `/tmp/odw-dogfood-isolated-wUVV4h` | First run failed due over-escaped test regex; after fix passed 2 tests |
| 2026-06-04 23:23 CST | Mock workflow | `odw exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-codex-30round-dogfood.js --backend mock --json` | Failed at final return: `ReferenceError: cwd is not defined`; 36 mock nodes completed before failure |
| 2026-06-04 23:24 CST | Mock workflow | `odw exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-codex-30round-dogfood.js --backend mock --json` | Passed; result reports `requestedCodexRounds: 33`, 20 parallel recon nodes, 10 pipeline nodes, 3 Bamboo trials |
| 2026-06-04 23:26 CST | Direct codexctl baseline | `codexctl plan --cwd /tmp/odw-dogfood-isolated-wUVV4h --prompt-file spec/goal/odw-codex-dogfood-direct-codexctl.prompt.md --sandbox read-only --approval-policy never --model gpt-5.4-mini --effort low --timeout unlimited` | Completed in ~49s; thread `019e933d-636c-74b3-84e6-e65b6fa810d2`; reported direct audit strengths/weaknesses and ODW improvement watch item |
| 2026-06-04 23:27 CST | Baseline cross-check | `nl -ba test/cli.test.js`; `npm test`; `git log --oneline --max-count=3` in isolated workspace | Current file has fixed regex; `npm test` passes 2 tests; direct baseline likely observed stale or sandbox-skewed test evidence |
| 2026-06-04 23:30 CST | Quality metric update | User asked to record every run's quality and how iteration improves quality | Added quality criteria to Done Criteria and Phase 2 evidence requirements |
| 2026-06-04 23:35 CST | Real ODW run | `odw exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-codex-30round-dogfood.js --input-file spec/goal/odw-codex-30round-dogfood.input.json --backend pandacode --json` | Completed; 33 Codex nodes succeeded; 3 Bamboo nodes failed due missing API key; exit review selected Bamboo preflight as top ODW improvement |
| 2026-06-04 23:39 CST | Quality analysis | `node spec/goal/analyze-odw-dogfood-quality.mjs /tmp/odw-dogfood-isolated-wUVV4h/.odw/runs/odw-exec-1780586920954-61912` | Successful Codex node average 7.24/10; duplicate indexes found; quality file created |
| 2026-06-04 23:44 CST | Product smoke | `cargo run -p open-dynamic-workflow -- exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-bamboo-preflight-smoke.js --backend pandacode --json` | Passed; returned `state: "blocked"` and `error.category: "bamboo_missing_api_key"` before prompt/raw report dispatch |
| 2026-06-04 23:48 CST | Product smoke | `cargo run -p open-dynamic-workflow -- exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-parallel-index-smoke.js --backend mock --json`; state inspection | Passed; five parallel nodes recorded unique indexes `1,2,3,4,5` |
| 2026-06-04 23:52 CST | Full mock workflow | `cargo run -p open-dynamic-workflow -- exec --path /tmp/odw-dogfood-isolated-wUVV4h --script spec/goal/odw-codex-30round-dogfood.js --input-file spec/goal/odw-codex-30round-dogfood.input.json --backend mock --json`; state inspection | Passed; 36 nodes; no duplicate indexes |
| 2026-06-04 23:56 CST | Test/lint | `cargo fmt --check`; `/tmp/odw-dogfood-isolated-wUVV4h npm test`; `cargo test`; `cargo clippy --workspace --all-targets -- -D warnings` | Passed after fixing selftest fake Bamboo env for new preflight |
| 2026-06-04 23:58 CST | codexctl review | `codexctl plan --cwd /Users/Zhuanz/workspace/odw-oss --prompt-file spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md --sandbox read-only --approval-policy never --model gpt-5.4-mini --effort medium --timeout unlimited` | Found 4 actionable issues; all fixed |
| 2026-06-05 00:01 CST | Regression test | `cargo test -p open-dynamic-workflow --test parity_selftest` | Passed after adding blocked preflight, default Deepseek, unknown provider, and raw-report no-overwrite tests |
| 2026-06-05 00:03 CST | Final test/lint | `cargo fmt --check`; `/tmp/odw-dogfood-isolated-wUVV4h npm test`; `cargo test`; `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| 2026-06-05 00:04 CST | Final codexctl review | `codexctl plan --cwd /Users/Zhuanz/workspace/odw-oss --prompt-file spec/goal/20260604-230837-odw-codex-dogfood.codexctl-review.md --sandbox read-only --approval-policy never --model gpt-5.4-mini --effort low --timeout unlimited` | No concrete regressions found; residual risk limited to future Bamboo config/provider layout drift |
| 2026-06-05 00:06 CST | Final smoke | `odw-bamboo-preflight-smoke.js`; `odw-parallel-index-smoke.js`; latest state inspection | Passed on current code; Bamboo smoke run `odw-exec-1780589198793-28851`; parallel smoke run `odw-exec-1780589198802-28850`; indexes `1,2,3,4,5` |
| 2026-06-05 00:07 CST | Commit | `git commit -m "Dogfood ODW Codex backend orchestration"` | Created commit `67603e7` with runner fixes, selftests, and dogfood evidence artifacts |

## Final Report Requirements

The executing Goal run must finish with:
- Final status: Complete, Blocked, or Budget-limited
- Checklist state
- Files changed
- Atomic commits created
- Commands run and results
- Test coverage summary, including unit/integration/end-to-end coverage or infeasibility rationale
- ODW real-run command, run id, counted rounds, and report/log artifacts
- Direct codexctl baseline command and comparison summary
- codexctl review command and findings summary
- Evidence artifacts
- Risks and follow-ups
