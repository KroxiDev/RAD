#!/usr/bin/env bash
# Offline instrument. The environment must permit private Unix sockets and bwrap namespaces.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "$0")/../../.."
export CARGO_HOME="${CARGO_HOME:-$PWD/target/cargo-home}" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
out="${RAD_CONFORMANCE_OUTPUT:-target/conformance/audio-media-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
# Reserve a fresh directory atomically; never reuse a run or symlink.
mkdir -p -- "$(dirname "$out")"
if ! mkdir -- "$out"; then
  echo "Cannot reserve new conformance output directory: $out" >&2
  exit 1
fi
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release --all-features -p rad-runtime -p rad-client -p rad-launcher --bins --examples > "$out/build-product.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples > "$out/build-plugins.log" 2>&1
python3 -I -S -B scripts/conformance/audio-media/checker.py "$out" audio media
rustfmt --edition 2024 --check crates/rad-runtime/tests/official_audio_media.rs > "$out/fmt.log" 2>&1
for n in 1 2 3; do
  flock /tmp/rad-phase2-cargo.lock cargo test --offline --locked --release --all-features -p rad-runtime --test official_audio_media -- --ignored --nocapture > "$out/final-$n.log" 2>&1
done
echo 'PASS three isolated official-plugin conformance runs'
