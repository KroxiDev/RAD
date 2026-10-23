#!/usr/bin/env bash
# Isolated synthetic baseline. No downloads, personal services, windows or deck.
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
mkdir -p target/performance
run="target/performance/$(date -u +%Y%m%dT%H%M%S)-$$"
build="$run-build.log"
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release \
  -p rad-runtime -p rad-launcher --all-features --bins --examples > "$build" 2>&1
python3 -I -S -B scripts/performance/profile.py "$run" "$@"
cp "$build" "$run/build.log"
printf '%s\n' 'offline locked release all-features jobs2 incremental0; default Cargo release profile' > "$run/build-flags.txt"
