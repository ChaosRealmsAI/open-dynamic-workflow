# Bamboo Migration Audit

Date: 2026-06-02

## Conclusion

The useful Bamboo coding-agent functionality has been folded into PandaCode as
the `pandacode bamboo` runtime. The standalone Bamboo CLI should no longer be
the product entry point.

## Migrated Into PandaCode

- Native autonomous coding loop.
- Domestic provider switching:
  - DeepSeek
  - Xiaomi/MiMo
  - Kimi
  - Zhipu
  - MiniMax
  - Qwen
  - StepFun
- Built-in model catalog with context, tier, thinking, cache, and tool metadata.
- Native OpenAI-compatible tool calling plus text-JSON fallback.
- Core coding tools: read, search, edit, write, bash, ask_user, finish.
- Safety boundaries for file tools, patch application, and shell commands.
- Usage, cache, cost, verification, context compaction, and final audit reports.
- JSONL event logs and durable run directories.
- Compact-aware resume context.
- Generation controls:
  - thinking
  - effort
  - max tokens
  - temperature
  - top-p
  - penalties
  - stop sequences
  - provider-specific `--param KEY=JSON`
- Automation-friendly non-zero exit when a run blocks.

## Unified Command Shape

Bamboo now follows the same runtime surface as Claude and Codex:

```bash
pandacode bamboo exec
pandacode bamboo resume
pandacode bamboo answer
pandacode bamboo status
pandacode bamboo logs
pandacode bamboo artifacts
pandacode bamboo model
pandacode bamboo models
pandacode bamboo list
pandacode bamboo doctor
```

## New PandaCode Defaults

Preferred runtime-local files now live under:

```text
.pandacode/bamboo/
```

Legacy `.bamboo` paths and `BAMBOO_*` env vars are still accepted for migration.

## Recent Live Smoke Evidence

The following providers completed `exec -> model -> resume` through PandaCode:

- `deepseek` / `deepseek-v4-pro`
- `xiaomi` / `mimo-v2.5-pro`
- `kimi` / `kimi-k2.6`
- `minimax` / `MiniMax-M3`
- `qwen` / `qwen3.7-max`

Zhipu returned account resource exhaustion. StepFun was skipped because no key
was present in the local env file.

Report:

```text
.pandacode/evals/bamboo-provider-smoke-20260602-161332/report.md
```

## Not Migrated Intentionally

- Standalone one-shot/chat/config CLI as a separate product surface.
- TUI, IDE, plugin, remote-control, and unrelated UI features.
- Plan/state-machine ceremony as the default behavior.

The product direction is direct autonomous execution behind a unified
PandaCode runtime interface.

