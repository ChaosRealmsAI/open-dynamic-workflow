# Open Dynamic Workflow

[![CI](https://github.com/ChaosRealmsAI/open-dynamic-workflow/actions/workflows/ci.yml/badge.svg)](https://github.com/ChaosRealmsAI/open-dynamic-workflow/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)

**Open Dynamic Workflow (`odw`) is an open re-implementation of Claude Code's
built-in Dynamic Workflow** — a script-driven runner for agent-authored
JavaScript workflows, paired with a pluggable executor that runs each step on
the model runtime of your choice. It aims to match the built-in workflow tool
feature-for-feature and then go further: deterministic resume, git-worktree
isolation, token budgets, and an offline HTML execution-graph report out of the
box.

It splits cleanly in two — orchestration and execution are decoupled:

| Crate | Binary | Job |
|-------|--------|-----|
| [`odw/`](./odw) | `odw` | **Orchestration.** Runs the workflow graph — `agent()`, `parallel()`, `pipeline()`, phases, budgets, worktree isolation, deterministic resume — and dispatches every `agent()` node to an executor. It schedules; it never calls a model itself. |
| [`pandacode/`](./pandacode) | `pandacode` | **Execution.** One CLI shape that runs a single coding task through `codex`, `claude`, or a domestic LLM (`bamboo`: deepseek / kimi / qwen / zhipu / minimax / …) and returns a structured report. |

`odw` spawns `pandacode` as a subprocess, so the two stay independent: swap the
executor, keep the orchestration — or build either crate on its own.

## Why

The built-in Dynamic Workflow is great, but it is a black box: you cannot host
it, retarget its executor, see exactly which model ran a node, or resume a run
deterministically after a crash. `odw` is the open version:

- **You own the loop.** Plain JavaScript workflows — `agent()`, `parallel()`,
  `pipeline()`, `phase()`, `budget`, nested `workflow()` — run on a runtime you
  can read and host.
- **Bring your own executor.** `odw` only schedules. `pandacode` is the default
  executor and speaks codex, claude, and domestic LLMs through one command shape.
- **Observable by default.** Every run writes an offline HTML execution graph
  (Mermaid) where each node shows the *exact* runtime, model, prompt, token
  count, and duration — parsed straight from your code, no guesswork.
- **Resumable & isolated.** Deterministic resume from a journal; optional
  git-worktree isolation so parallel agents that edit files never collide.

## Install

```bash
git clone https://github.com/ChaosRealmsAI/open-dynamic-workflow
cd open-dynamic-workflow
./install.sh            # builds + installs both `odw` and `pandacode` onto PATH
```

Or build the workspace directly:

```bash
cargo build --release   # produces target/release/odw and target/release/pandacode
```

Then check the wiring:

```bash
odw doctor              # verifies runtimes + that pandacode is reachable
```

## Quick start

```bash
odw init --path ./my-project                       # scaffold the pack (skill, schemas, examples)
odw exec --script examples/hello.js --backend mock # token-free dry run — proves the graph
odw exec --script examples/hello.js                # real run via pandacode
```

A workflow is just JavaScript — the same shape as the built-in tool:

```js
export const meta = {
  name: "review-changes",
  description: "Review changed files across dimensions, verify each finding",
  phases: [{ title: "Review" }, { title: "Verify" }],
};

const DIMENSIONS = [
  { key: "bugs", prompt: "Find correctness bugs in the diff." },
  { key: "perf", prompt: "Find performance regressions in the diff." },
];

// Each dimension reviews, then its findings verify as soon as that review lands.
const results = await pipeline(
  DIMENSIONS,
  d => agent(d.prompt, { runtime: "codex", phase: "Review", schema: FINDINGS }),
  review => parallel(review.findings.map(f => () =>
    agent(`Adversarially verify: ${f.title}`, { runtime: "claude", phase: "Verify", schema: VERDICT })
      .then(v => ({ ...f, verdict: v }))))
);

return { confirmed: results.flat().filter(f => f.verdict?.isReal) };
```

## Execution-graph report

The moment a run finishes (success **or** failure), `odw` writes a standalone
`report.html` and prints its path. Open it and you get a Mermaid graph of the
run; click any node to see its **config parsed from your code** — runtime,
model, provider, schema, isolation — plus the verbatim prompt and the result
(status, tokens, duration). No colors, no prose, no telemetry: just what the
code said and what actually happened. Add `--open` to pop it automatically.

## For agents

After `odw init`, a Claude Code skill is installed at
`.claude/skills/odw/SKILL.md` (canonical copy: [`odw/skills/odw/`](./odw/skills/odw)).
Load it and an agent has install, authoring, and run instructions in one place —
no need to read the source.

## Runtimes (via pandacode)

| Runtime | What it is |
|---------|------------|
| `codex` | OpenAI Codex coding agent (writes/edits files, runs to completion). |
| `claude` | Claude as a coding/analysis agent. |
| `bamboo` | Domestic LLM providers — deepseek, kimi, qwen, zhipu, minimax, … — selected with `--provider`. |

Pick per node: `agent(prompt, { runtime: "bamboo", provider: "deepseek", model: "deepseek-chat" })`.

## Documentation

- **[odw/README.md](./odw/README.md)** — orchestration runtime, full CLI, the
  workflow API, and how it maps to the built-in tool feature-for-feature.
- **[pandacode/README.md](./pandacode/README.md)** — executor command shape,
  runtime mapping, and the odw↔pandacode integration contract.

## License

MIT — see [LICENSE](./LICENSE). Vendored browser assets used by the report
(`mermaid.min.js`, `marked.min.js`) retain their own MIT licenses.
