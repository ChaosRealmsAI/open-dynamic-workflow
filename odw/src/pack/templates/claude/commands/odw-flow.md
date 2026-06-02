# Open Dynamic Workflow complex flow workflow

Run a complex Dynamic Workflow over:

```text
$ARGUMENTS
```

Required shape:
1. Read `.claude/workflows/odw-flow.js` as the starter script.
2. Plan/decompose phase: create a structured task plan with parallelizable
   child tasks.
3. Implement phase: dynamically fan out child tasks with
   `fanout(tasks, mapper)`. Each child task should use `odw-codex-coder`.
4. Join phase: collect child task evidence into a single structured result.
5. Verify phase: run an adversarial verifier over the joined evidence.
6. Review phase: fan out independent review agents over review targets.
7. Quality gate: pass, fail, or emit rework tasks. Rework tasks loop back
   through the same Codex fan-out path with a bounded iteration count.
8. Synthesis phase: return only verified, quality-gated facts.

Use prompt slots instead of hidden framework prompts: the caller should inject
`input.prompts.<slot>` for every node prompt. Give every node a stable id,
schema, and retry policy. The run must be observable through `odw runs show`.
