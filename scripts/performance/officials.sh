#!/usr/bin/env bash
# Same implementation, private fake backends, no product changes or personal services.
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
mkdir -p target/performance
run="${1:-target/performance/o-$(date -u +%Y%m%dT%H%M%S)-$$}"
if (($#)); then shift; fi
build="$run-build.log"
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release \
  -p rad-runtime -p rad-launcher --all-features --bins --examples > "$build" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build \
  --manifest-path plugins/Cargo.toml --offline --locked --release --workspace \
  --all-features --bins --examples >> "$build" 2>&1
python3 -I -S -B scripts/performance/officials.py "$run" "$@"
cp "$build" "$run/build.log"
