# Open Dynamic Workflow audit workflow

Run a Dynamic Workflow audit over:

```text
$ARGUMENTS
```

Required shape:
1. Read `.claude/workflows/odw-audit.js` as the starter script.
2. Discovery phase: use `odw-researcher` agents to inventory target files.
3. Review phase: fan out `odw-security-reviewer` or read-only specialists over
   independent files/modules.
4. Verification phase: use `odw-verifier` agents to reject weak claims.
5. Synthesis phase: use `odw-synthesizer` to return the final report.

Do not edit files during an audit. If a fix is requested later, use
`/odw-ship` with `odw-codex-coder`.
