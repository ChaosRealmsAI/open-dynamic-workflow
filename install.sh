#!/usr/bin/env sh
# Build and install both binaries from the workspace:
#   odw        — the orchestration runtime
#   pandacode  — the executor that odw dispatches to
#
# Usage:
#   ./install.sh           # cargo install both onto PATH (recommended)
#   ./install.sh --build   # just build release binaries, do not install
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
say() { printf '\033[1;36m[install]\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m[install] %s\033[0m\n' "$1" >&2; exit 1; }

command -v cargo >/dev/null 2>&1 || die "cargo not found — install Rust from https://rustup.rs first."

say "Building the workspace (release): odw + pandacode …"
( cd "$here" && cargo build --release )

if [ "${1:-}" = "--build" ]; then
  say "Built. Binaries: $here/target/release/{odw,pandacode}"
  exit 0
fi

say "Installing odw + pandacode onto PATH …"
cargo install --path "$here/odw" --force
cargo install --path "$here/pandacode" --force

say "Done. Sanity check:"
say "  odw doctor            # verifies runtimes + that pandacode is reachable"
say "  odw init --path .     # scaffold a project (skill, schemas, examples)"
