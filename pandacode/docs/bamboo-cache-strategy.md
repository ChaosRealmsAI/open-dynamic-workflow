# Bamboo Cache Strategy

Bamboo treats provider caching as server-side prompt prefix caching. It does not
cache model responses locally.

## Product Goal

The goal is lower cost and higher autonomous throughput:

- Keep stable bytes stable: system prompt, tool protocol, repo summary,
  provider schema, and durable rules.
- Keep volatile bytes late: current task, recent tool results, command output,
  latest diff, and resume notes.
- Compact only old volatile history. Do not rewrite the stable prefix during a
  long task.

## Provider Notes

- DeepSeek exposes prompt cache hit/miss tokens.
- Xiaomi/MiMo exposes cached prompt tokens; miss tokens are derived when needed.
- Kimi supports `prompt_cache_key` and exposes cache usage when returned.
- MiniMax M-series exposes context cache token usage in OpenAI-compatible
  responses.
- Qwen supports context cache, but OpenAI-compatible usage may not always expose
  hit/miss fields.
- Zhipu GLM exposes cache counters when the provider returns them.
- StepFun may expose cached tokens depending on routed endpoint.

## Runtime Behavior

PandaCode Bamboo writes reports with:

- `summary.usage`
- `summary.cache`
- `summary.context_compaction`
- `model_settings`
- `stable_context` hash/size diagnostics

`resume` places previous report/event tail in the volatile task message, not in
the stable prefix, so resumed runs can keep cache reuse while still seeing prior
state.

## Practical Guidance

For repeated autonomous runs:

```bash
pandacode bamboo exec \
  --provider deepseek \
  --model deepseek-v4-pro \
  --effort high \
  --max-tokens 48000 \
  --timeout-ms 7200000 \
  --json \
  --task-file task.md
```

Inspect cache:

```bash
pandacode bamboo status --session latest --json | jq '.summary.cache'
```

