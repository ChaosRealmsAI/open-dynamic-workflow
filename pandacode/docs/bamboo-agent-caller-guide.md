# Bamboo Agent Caller Guide

This guide is for an upstream AI agent, scheduler, CI job, or orchestration
service that calls PandaCode's Bamboo runtime as a headless coding executor.

## Primary Invocation

Use `pandacode bamboo exec` for a new autonomous coding task:

```bash
pandacode bamboo exec \
  --cd /path/to/repo \
  --session my-task \
  --provider deepseek \
  --model deepseek-v4-pro \
  --effort high \
  --permission max \
  --thinking enabled \
  --max-tokens 48000 \
  --max-steps 100 \
  --model-timeout-ms 180000 \
  --run-timeout-ms 7200000 \
  --max-total-tokens 800000 \
  --max-cost 10 \
  --max-cost-currency cny \
  --price-file .pandacode/bamboo/pricing.cn.json \
  --verify "cargo test" \
  --json \
  --task-file task.md
```

The caller does not implement tool calling. Bamboo sends native tools when the
provider supports them, executes tool results locally, and falls back to
text-JSON tool actions when needed.

## Resume

Resume the same task by session:

```bash
pandacode bamboo resume \
  --cd /path/to/repo \
  --session my-task \
  --task "Continue from the previous result and finish verification."
```

Resume uses the previous `report.json`, `events.jsonl` tail, and
`resume-context.md`. It does not replay the full raw transcript.

## Model Switching

Set provider/model/parameters for the next turn:

```bash
pandacode bamboo model \
  --cd /path/to/repo \
  --session my-task \
  --provider qwen \
  --model qwen3.7-max \
  --effort high \
  --thinking enabled \
  --max-tokens 32000 \
  --temperature 0.2 \
  --top-p 0.9
```

`resume` inherits stored settings unless you override them on the command line.

`--permission limited` is available for lower-risk runs. It still allows normal
workspace reads/writes and local verification, but blocks background shell,
package installs, downloads, and recursive/destructive cleanup.

## Parameter Surface

Common Bamboo generation flags:

- `--thinking enabled|disabled`
- `--effort none|minimal|low|medium|high|xhigh|max`
- `--max-tokens`
- `--temperature`
- `--top-p`
- `--presence-penalty`
- `--frequency-penalty`
- repeated `--stop TEXT`
- repeated `--param KEY=JSON`

Provider-specific notes:

- Kimi `kimi-k2.6` currently accepts only `temperature=0.6` and `top_p=0.95`
  when those parameters are supplied.
- MiniMax supports provider-specific params such as
  `--param service_tier='"standard"'`.
- DeepSeek maps effort to `reasoning_effort`.
- Qwen maps thinking to `enable_thinking`.
- MiniMax sends `reasoning_split=true` by default.

## Budgets And Verification

`bamboo exec` and `bamboo resume` expose PandaCode's native unattended-run
controls for the Bamboo runtime:

- `--max-steps`
- `--shell-timeout-ms`
- `--model-timeout-ms`
- `--run-timeout-ms`
- `--history-keep-last`
- `--compact-threshold-tokens`
- `--compact-reserve-tokens`
- `--max-input-tokens`
- `--max-output-tokens`
- `--max-total-tokens`
- `--max-cost`
- `--max-cost-currency`
- `--price-file`
- repeated `--verify`
- `--auto-verify`
- `--cache-warm`
- `--cache-warm-rounds`
- `--cache-prefix`
- repeated `--cache-prefix-file`
- `--cache-key`
- `--cache-retention`

## Output And Artifacts

`exec` and `resume` print JSON. Important fields:

- `ok`
- `state`
- `summary.status`
- `pending_user_input`
- `summary.changed_files`
- `summary.verification`
- `summary.usage`
- `summary.cache`
- `summary.estimated_cost`
- `summary.context_compaction`
- `record.artifacts`

Durable artifacts are written under the task workspace:

```text
.pandacode/
  sessions/bamboo/
  bamboo/
    prompts/
    runs/<run-id>/
      events.jsonl
      report.json
      metadata.json
      resume-context.md
```

## Exit Codes

`pandacode bamboo exec` and `pandacode bamboo resume` exit zero for
`completed` and `waiting_for_user`. Waiting reports include
`pending_user_input` and can be continued with `pandacode bamboo answer`.
Blocked or failed runs exit non-zero after writing the JSON report and durable
artifacts, so callers should parse stdout or the durable report even on
non-zero exit.

## Environment

Preferred PandaCode env vars:

```bash
export PANDACODE_BAMBOO_PROVIDER=deepseek
export PANDACODE_BAMBOO_API_KEY="..."
export PANDACODE_BAMBOO_MODEL=deepseek-v4-pro
export PANDACODE_BAMBOO_PRICE_FILE=.pandacode/bamboo/pricing.cn.json
```

Legacy `BAMBOO_*` aliases still work. Provider-specific keys such as
`DEEPSEEK_API_KEY`, `XIAOMI_API_KEY`, `KIMI_API_KEY`, `MINIMAX_API_KEY`,
`QWEN_API_KEY`, `ZHIPU_API_KEY`, and `STEPFUN_API_KEY` are also supported.
