#!/usr/bin/env bash
# Lightweight installer for Open Dynamic Workflow (odw) + PandaCode.
#
# odw is the orchestration entrypoint; it dispatches every workflow node to the
# `pandacode` executor (codex / claude / bamboo). This script installs both so a
# new user / AI agent is ready out of the box.
#
#   ./install.sh
#
# pandacode is located in this order: already on PATH -> $ODW_PANDACODE_BIN ->
# a sibling ../pandacode checkout -> cloned from $PANDACODE_REPO. Override with:
#   PANDACODE_DIR=/path/to/pandacode ./install.sh
#   PANDACODE_REPO=https://github.com/<org>/pandacode ./install.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

command -v cargo >/dev/null 2>&1 || { echo "cargo (Rust) is required: https://rustup.rs" >&2; exit 1; }

# 1) Install odw -------------------------------------------------------------
say "Installing odw …"
cargo install --path "$here" --quiet
say "odw -> $(command -v odw)"

# 2) Ensure pandacode --------------------------------------------------------
if command -v pandacode >/dev/null 2>&1; then
  say "pandacode already on PATH: $(command -v pandacode)"
elif [ -n "${ODW_PANDACODE_BIN:-}" ] && [ -x "${ODW_PANDACODE_BIN}" ]; then
  say "pandacode via ODW_PANDACODE_BIN: ${ODW_PANDACODE_BIN}"
else
  pc_dir="${PANDACODE_DIR:-${here}/../pandacode}"
  if [ ! -d "$pc_dir" ] && [ -n "${PANDACODE_REPO:-}" ]; then
    pc_dir="$(mktemp -d)/pandacode"
    say "Cloning pandacode from ${PANDACODE_REPO} …"
    git clone --depth 1 "${PANDACODE_REPO}" "$pc_dir"
  fi
  if [ -d "$pc_dir" ]; then
    say "Installing pandacode from ${pc_dir} …"
    cargo install --path "$pc_dir" --quiet
    say "pandacode -> $(command -v pandacode || echo '~/.cargo/bin/pandacode')"
  else
    echo "pandacode not found. Put it next to this repo, set PANDACODE_DIR=<path>," >&2
    echo "or PANDACODE_REPO=<git url>, then re-run ./install.sh." >&2
    exit 1
  fi
fi

# 3) Verify ------------------------------------------------------------------
say "Checking the install …"
odw doctor || true
cat <<'NEXT'

Installed. Next:
  odw init --path .                                   # scaffold the pack + agent skill
  odw report --script examples/01-single-node.js --open   # see a workflow graph (mock, free)
  odw exec --script examples/01-single-node.js --backend pandacode --json
NEXT
