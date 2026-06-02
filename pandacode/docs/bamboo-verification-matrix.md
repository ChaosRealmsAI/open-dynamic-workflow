# Bamboo Verification Matrix

## Static Gates

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy -- -D warnings
cargo build --release
```

## Runtime Gates

- `pandacode bamboo models` lists the built-in domestic model catalog.
- `pandacode bamboo doctor` verifies runtime catalog/config visibility.
- `pandacode bamboo exec` creates a durable run; `waiting_for_user` is a
  successful waiting state, while blocked/failed runs exit non-zero.
- `pandacode bamboo resume` continues from the prior run context.
- `pandacode bamboo model` stores provider/model/parameter settings for the next
  turn.
- `pandacode bamboo logs --json` returns JSONL event tails.
- `pandacode bamboo artifacts` returns durable report paths.

## Live Provider Smoke

Use:

```bash
scripts/smoke-bamboo-live-providers.sh
```

The smoke creates one isolated directory per provider, runs:

1. `exec`
2. `model`
3. `resume`

and verifies `smoke.md` contains both `exec=ok` and `resume=ok`.

## Evidence Fields

Reports should include:

- provider/model/effort/thinking
- changed files
- verification commands
- usage
- cache hit/miss
- estimated cost when price data exists
- duration
- context compaction
- final git audit when applicable
