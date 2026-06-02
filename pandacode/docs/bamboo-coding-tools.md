# Bamboo Coding Tools

PandaCode's Bamboo runtime is a headless autonomous coding loop. The model gets
tool instructions in the stable prompt prefix so repeated runs can benefit from
provider prefix caching.

## Default Tool Surface

The default model-facing tools are intentionally small:

- `read`: read one or more files with bounded output.
- `search`: search the workspace.
- `edit`: replace or insert targeted text.
- `write`: create files or write large generated files in chunks.
- `bash`: run non-interactive workspace commands.
- `ask_user`: stop only when external input is genuinely required.
- `finish`: end with `success` or `blocked` and a concise report.

Older fine-grained actions remain accepted for compatibility and resume, but
new runs should prefer the compact core tools.

## Recommended Model Behavior

1. Inspect only the context needed for the task.
2. Use targeted edits for existing files and chunked `write` for large new
   files.
3. Run concrete verification commands through `bash`.
4. Read errors and repair before finishing.
5. Use `ask_user` only when the task is impossible without external input.
6. Call `finish` once the requested result exists and the evidence is credible.

There is no mandatory plan mode. The model may plan internally, but the product
contract is direct autonomous execution.

## Safety

File tools and patches are workspace-confined and cannot modify protected
runtime or secret paths including:

- `.git`
- `.pandacode`
- `.odw/runs` when PandaCode is called by Open Dynamic Workflow
- legacy `.bamboo`
- `.ssh`
- `.env*`
- private key files such as `.pem`, `.key`, `.p12`, `id_rsa`, `id_ed25519`

`bash` is available because coding agents need it, but destructive host-level
commands are blocked, including broad deletes, `git reset --hard`, `git clean`,
`sudo`, pipe-to-shell installers, shutdown/reboot commands, broad kill commands,
and protected-path access.

## Observability

Every run records:

- JSONL events
- final report JSON
- model settings and thinking parameters
- usage and cache hit/miss
- verification evidence
- final git audit when the workspace is a git repo
- compact-aware resume context
