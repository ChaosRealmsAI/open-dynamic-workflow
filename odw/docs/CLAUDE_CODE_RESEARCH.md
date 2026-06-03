# Claude Code Research Notes

Checked against official Claude Code documentation on 2026-05-31.

## Dynamic Workflows

Official shape:

- Dynamic workflows orchestrate many subagents from a JavaScript script that
  Claude writes and the runtime executes in the background.
- They are meant for codebase audits, large migrations, cross-checked research,
  and other tasks where the orchestration itself should be repeatable.
- Intermediate results live in script variables, not in the main Claude context.
- Runs are managed through `/workflows`.
- The documented controls are watch, drill into phases/agents, pause/resume
  with `p`, stop with `x`, restart selected running agents with `r`, and save
  the run script with `s`.
- The documented runtime limits include no direct filesystem or shell access
  from the workflow script itself, up to 16 concurrent agents, and up to 1,000
  agents per run.
- Workflows are available in the CLI, Desktop app, IDE extensions,
  non-interactive `claude -p`, and Agent SDK surfaces.

Sources:

- https://code.claude.com/docs/en/workflows
- https://code.claude.com/docs/en/cli-usage

## Subagents

Official shape:

- Subagents are specialized assistants with their own context window, system
  prompt, tool access, permissions, and optional model choice.
- Project-level subagents live in `.claude/agents/` and can be checked into a
  repository.
- Subagent files use YAML frontmatter. `name` and `description` are required.
- `tools`, `model`, `permissionMode`, `maxTurns`, `mcpServers`, `skills`,
  `effort`, `isolation`, and related fields can further constrain behavior.
- The `model` field can be `sonnet`, `opus`, `haiku`, a full model ID, or
  `inherit`.
- Subagents cannot use the `Agent` tool, so nested subagent spawning is not the
  supported product shape. Dynamic workflows and the main Claude Code session
  are the orchestration layer.
- Claude Code can also pass subagent definitions through `--agents` JSON for one
  CLI session.

Sources:

- https://code.claude.com/docs/en/sub-agents
- https://code.claude.com/docs/en/settings

## Cost And Availability

Official shape:

- Dynamic workflows are in research preview and require Claude Code v2.1.154 or
  later.
- The workflows page says they are available on paid plans, Anthropic API
  access, and supported cloud-provider routes.
- Workflow runs spawn many agents and count toward the same plan usage and rate
  limits as normal Claude Code sessions.
- Claude Code cost tracking is exposed through `/cost` and Console workspace
  usage. Team usage is billed by API token consumption.

Sources:

- https://code.claude.com/docs/en/workflows
- https://code.claude.com/docs/en/costs

## Product Conclusion

This research is background only. The product boundary has since converged on a
direct `odw exec` runner and no longer exposes a Claude Code compatibility
surface.

The correct product boundary is:

- ODW owns direct workflow execution, live logs, run journals, and direct-run
  resume for `odw exec`.
- Open Dynamic Workflow installs workflow starter scripts, schemas, and CLI
  management tools.
- ODW does not own live Claude Code `/workflows` state.
- Real executor work goes through PandaCode. Codex-backed workflow nodes use
  `agent(prompt, { runtime: "codex" })`, which dispatches once to
  `pandacode codex exec`.
