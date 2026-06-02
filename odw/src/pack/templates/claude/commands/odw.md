# Open Dynamic Workflow workflow

Run a Claude Code Dynamic Workflow using Open Dynamic Workflow.

Input from the user:

```text
$ARGUMENTS
```

Process:
1. Read `.odw/README.md`, `.claude/workflows/odw-authoring-contract.md`,
   and relevant `.odw/schemas/*.schema.json` files.
2. Use a dynamic workflow, not a turn-by-turn manual plan, when the task has
   separable discovery, implementation, verification, or synthesis work.
3. Route read-only discovery to `odw-researcher`.
4. Route security review to `odw-security-reviewer`.
5. Route code edits to `odw-codex-coder` with `runtime: "codex"` (single-shot).
6. Route worker, shell, or executor failures to `odw-failure-analyst`.
7. Route verification to `odw-test-runner` and `odw-verifier`.
8. Route final report to `odw-synthesizer`.

Make the workflow phases explicit. For direct `odw exec` scripts, prefer
`promptSlot(name, context, suggested)` and have the caller inject
`input.prompts.<slot>`; suggested text is for mock smoke tests or explicit
caller opt-in.
