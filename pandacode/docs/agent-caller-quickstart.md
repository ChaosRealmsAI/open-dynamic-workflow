# Agent Caller Quickstart

Use top-level commands for normal automation. They hide runtime details until a
caller needs advanced controls.

## Minimal Loop

```bash
pandacode run --cd <workspace> --session <task-id> --task-file task.md --json
pandacode status --cd <workspace> --session <task-id> --json
pandacode logs --cd <workspace> --session <task-id> --tail 200 --json
pandacode resume --cd <workspace> --session <task-id> --task-file next.md --json
```

Use an explicit `--session <task-id>` for automation and parallel work. The
implicit `latest` session is convenient for a human operator, but callers should
not depend on it when multiple tasks may run in the same workspace.

`--task-file` is first resolved from the caller's current directory and then
relative to `--cd`. This lets a caller use either `--task-file task.md` inside a
workspace or `--cd app --task-file ../task.md`.

When `--json` is present, runtime failures are JSON too:

```json
{
  "ok": false,
  "state": "failed",
  "error": {
    "message": "missing API key; set ...",
    "chain": ["missing API key; set ..."]
  }
}
```

If `state` is `waiting_for_user`, answer and continue:

```bash
pandacode answer --cd <workspace> --session <task-id> --text "Use the default branch" --wait --json
```

## Runtime Selection

Default runtime is `auto`.

- `pandacode run ...` chooses the first usable runtime in this order:
  Bamboo with a configured key, Claude, then Codex.
- `pandacode run --provider deepseek ...` selects Bamboo.
- `pandacode run --runtime claude ...` or `--runtime codex` pins a delegated
  runtime.
- `PANDACODE_RUNTIME=bamboo|claude|codex` sets the default for top-level calls.

After a run, PandaCode writes a global latest pointer. `status`, `logs`,
`resume`, `answer`, `artifacts`, `interrupt`, and `stop` can omit `--runtime`
unless the caller wants to override it. If a caller passes a concrete session
name with `--runtime auto`, PandaCode resolves the session's runtime from local
session records before falling back to latest.

## Progressive Disclosure

Use `pandacode --help`, `pandacode run --help`, and runtime-specific help for
calling syntax. Use `pandacode doctor --json` for availability and `pandacode
models --json` for model/provider choices.

Top-level commands expose common controls only:

- task input
- workspace
- session
- model
- effort
- permission
- timeout
- JSON output

With `--runtime auto`, known model ids can select a backend. Domestic model ids
select Bamboo and infer their provider, Claude aliases such as `opus` select
Claude, and `gpt-*` ids select Codex.

Permission defaults to `max`. Use `--permission limited` when the caller wants a
lower-risk workspace-write run. Codex maps this to its built-in workspace-write
sandbox, Claude maps it to `acceptEdits`, and Bamboo enforces its own limited
tool policy.

Use runtime-specific commands for advanced controls:

```bash
pandacode bamboo exec --help
pandacode claude exec --help
pandacode codex exec --help
```

Bamboo currently implements provider cache, cost budgets, token budgets,
auto-compact, and verification commands. Claude and Codex expose their support
status through `pandacode doctor --json` and `pandacode models --json`.

## Output Contract

Every top-level JSON command returns a stable envelope with `ok`, `runtime`,
`action`, `session`, and `state` when the command is session-based. `logs --json`
returns a redacted `output_tail` or `log_tail` by default rather than full raw
terminal capture. Runtime-specific raw details are nested under `raw` or
`command`.
