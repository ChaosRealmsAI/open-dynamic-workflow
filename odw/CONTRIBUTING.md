# Contributing to Open Dynamic Workflow

Thanks for helping! odw is a small, gate-driven Rust CLI that embeds a JavaScript
runtime. Keep changes focused and the gate green.

## The gate (run before every commit)

```bash
cargo build --all-targets
cargo test                                  # unit + the parity_selftest integration test
cargo clippy --all-targets -- -D warnings   # warnings are errors
node scripts/selftest.mjs                   # parity self-test (token-free, mock backend)
```

CI (`.github/workflows/ci.yml`) runs exactly this on every push / PR.

## Layout

- `src/main.rs` — the `odw` CLI (commands, run journal, capabilities).
- `src/pack/` — the project pack `odw init` installs; templates under
  `src/pack/templates/`.
- `src/pack/templates/runtime/odw-js-runner.mjs` — the embedded workflow runtime
  (`agent`/`parallel`/`pipeline`/`budget`/worktree/resume). Changes here must be
  reflected in `workflow-api.d.ts` and covered by `scripts/selftest.mjs`.
- `skills/odw/SKILL.md` — the agent-usable skill (also installed by `odw init`).
- `examples/` — `mock`-runnable example workflows.

## Conventions

- **Boundary:** odw only orchestrates and dispatches to `pandacode`. Executor
  reliability (codex/claude/bamboo) belongs to PandaCode, not odw.
- **No silent failures:** surface executor failures as `{ ok:false, error }`;
  a non-zero pandacode exit is always a failure.
- **Parity first:** match the built-in Dynamic Workflow semantics; the
  `.d.ts`-vs-sandbox drift guard and `selftest.mjs` enforce this.
- **Atomic commits:** one reversible decision per commit; explain *why* in the body.

## Adding a runtime behavior

1. Implement in `odw-js-runner.mjs`.
2. Update `src/pack/templates/odw/framework/workflow-api.d.ts`.
3. Add a token-free assertion to `scripts/selftest.mjs`.
4. Run the gate.
