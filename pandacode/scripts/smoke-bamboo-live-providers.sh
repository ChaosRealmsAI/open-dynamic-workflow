#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ENV_FILE="${PANDACODE_BAMBOO_LIVE_ENV:-.pandacode/bamboo/live.env}"
if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

BIN="${PANDACODE_BIN:-target/release/pandacode}"
if [[ ! -x "$BIN" ]]; then
  cargo build --release
fi

RUN_DIR=".pandacode/evals/bamboo-provider-smoke-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN_DIR/logs"
printf '%s\n' "$RUN_DIR" > .pandacode/evals/latest-bamboo-provider-smoke

providers="${PANDACODE_BAMBOO_PROVIDERS:-deepseek xiaomi kimi zhipu minimax qwen stepfun}"

keyvar_for() {
  case "$1" in
    deepseek) printf 'DEEPSEEK_API_KEY' ;;
    xiaomi) printf 'XIAOMI_API_KEY' ;;
    kimi) printf 'KIMI_API_KEY' ;;
    zhipu) printf 'ZHIPU_API_KEY' ;;
    minimax) printf 'MINIMAX_API_KEY' ;;
    qwen) printf 'QWEN_API_KEY' ;;
    stepfun) printf 'STEPFUN_API_KEY' ;;
  esac
}

model_for() {
  case "$1" in
    deepseek) printf '%s' "${DEEPSEEK_MODEL:-deepseek-v4-pro}" ;;
    xiaomi) printf '%s' "${XIAOMI_MODEL:-mimo-v2.5-pro}" ;;
    kimi) printf '%s' "${KIMI_MODEL:-kimi-k2.6}" ;;
    zhipu) printf '%s' "${ZHIPU_MODEL:-glm-5.1}" ;;
    minimax) printf '%s' "${MINIMAX_MODEL:-MiniMax-M3}" ;;
    qwen) printf '%s' "${QWEN_MODEL:-qwen3.7-max}" ;;
    stepfun) printf '%s' "${STEPFUN_MODEL:-step-3.7-flash}" ;;
  esac
}

for provider in $providers; do
  keyvar="$(keyvar_for "$provider")"
  keyval="${!keyvar:-}"
  if [[ -z "$keyval" ]]; then
    printf '%s\tskipped\tmissing_key\n' "$provider" >> "$RUN_DIR/summary.tsv"
    continue
  fi

  model="$(model_for "$provider")"
  slug="$(printf '%s' "$model" | tr '/: .' '____')"
  work="$RUN_DIR/$provider-$slug"
  session="smoke-$provider"
  mkdir -p "$work"

  sampling=(--temperature 0 --top-p 1)
  if [[ "$provider:$model" == "kimi:kimi-k2.6" ]]; then
    sampling=(--temperature 0.6 --top-p 0.95)
  fi
  exec_task="Provider smoke test for pandacode bamboo. Create smoke.md with exactly these lines: provider=$provider, model=$model, exec=ok. Then run: test -s smoke.md && grep -q '^exec=ok$' smoke.md && grep -q '^provider=$provider$' smoke.md. Finish success only if the command passes."

  if "$BIN" bamboo exec \
    --cd "$work" \
    --session "$session" \
    --provider "$provider" \
    --model "$model" \
    --effort none \
    --thinking disabled \
    --max-tokens 2048 \
    "${sampling[@]}" \
    --timeout-ms 300000 \
    --json \
    --task "$exec_task" \
    > "$RUN_DIR/logs/$provider.$slug.exec.json" \
    2> "$RUN_DIR/logs/$provider.$slug.exec.err"; then
    "$BIN" bamboo model \
      --cd "$work" \
      --session "$session" \
      --provider "$provider" \
      --model "$model" \
      --effort none \
      --thinking disabled \
      --max-tokens 2048 \
      "${sampling[@]}" \
      --json \
      > "$RUN_DIR/logs/$provider.$slug.model.json" \
      2> "$RUN_DIR/logs/$provider.$slug.model.err"

    resume_task="Resume smoke test. Append exactly one line to smoke.md: resume=ok. Then run: grep -q '^exec=ok$' smoke.md && grep -q '^resume=ok$' smoke.md && wc -l smoke.md. Finish success only if the command passes."
    if "$BIN" bamboo resume \
      --cd "$work" \
      --session "$session" \
      --timeout-ms 300000 \
      --json \
      --task "$resume_task" \
      > "$RUN_DIR/logs/$provider.$slug.resume.json" \
      2> "$RUN_DIR/logs/$provider.$slug.resume.err"; then
      printf '%s\tpassed\t%s\t%s\n' "$provider" "$model" "$work" >> "$RUN_DIR/summary.tsv"
    else
      printf '%s\tresume_failed\t%s\t%s\n' "$provider" "$model" "$work" >> "$RUN_DIR/summary.tsv"
    fi
  else
    printf '%s\texec_failed\t%s\t%s\n' "$provider" "$model" "$work" >> "$RUN_DIR/summary.tsv"
  fi
done

cat "$RUN_DIR/summary.tsv"
