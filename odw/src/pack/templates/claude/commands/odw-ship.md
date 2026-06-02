# Open Dynamic Workflow ship workflow

Run a Dynamic Workflow to implement:

```text
$ARGUMENTS
```

Required shape:
1. Read `.claude/workflows/odw-ship.js` as the starter script.
2. Discovery phase: `odw-researcher` identifies the minimal change surface.
3. Implementation phase: `odw-codex-coder` with `runtime: "codex"` implements
   the scoped change in one pass.
4. Verification phase: `odw-test-runner` runs scoped tests and
   `odw-verifier` checks the change against the goal.
5. Failure phase: if implementation or verification fails, `odw-failure-analyst`
   returns structured retry/blocker feedback.
6. Synthesis phase: `odw-synthesizer` reports changed files, verification,
   residual risk, and exact next action.

Code execution runs single-shot via the pandacode Codex executor
(`runtime: "codex"`); do not reimplement the Codex app-server in Claude.
