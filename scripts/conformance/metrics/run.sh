#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "$0")/../../.."
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export RAD_TEST_LAUNCHER_HELPER="$PWD/target/release/rad-launcher-helper"
run="target/conformance/metrics-$(date -u +%Y%m%dT%H%M%S)-$$"
mkdir -p target/conformance
mkdir "$run"
printf '%s\n' "offline locked release all-features jobs2, fixture-only, XDG private" > "$run/flags.txt"
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release -p rad-runtime -p rad-launcher -p rad-client --all-features --bins --examples > "$run/build-runtime.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build --manifest-path plugins/Cargo.toml --offline --locked --release -p rad-plugin-metrics --all-features --example metrics-fixture > "$run/build-fixture.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build --manifest-path plugins/Cargo.toml --offline --locked --release -p rad-plugin-sdk --bin radp-pack > "$run/build-packer.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build --manifest-path scripts/conformance/metrics/fixture/Cargo.toml --offline --locked --release > "$run/build-fault-fixture.log" 2>&1
python3 -I -S -B scripts/conformance/audio-media/checker.py "$run" metrics
flock /tmp/rad-phase2-cargo.lock cargo test --offline --locked --release -p rad-runtime --all-features --test official_metrics --no-run --message-format=json > "$run/test-artifact.jsonl" 2> "$run/test-build.log"
python3 -I -S - "$run/test-artifact.jsonl" > "$run/test-executable.txt" <<'PY'
import json,sys
paths={v['executable'] for line in open(sys.argv[1]) if (v:=json.loads(line)).get('reason')=='compiler-artifact' and v.get('target',{}).get('name')=='official_metrics' and v.get('executable')}
assert len(paths)==1, paths
print(paths.pop())
PY
set +e
timeout --signal=TERM --kill-after=10s 120s "$(cat "$run/test-executable.txt")" --ignored --exact official_metrics_interoperates_without_hardware --nocapture > "$run/conformance.log" 2>&1
code=$?
set -e
printf '%s\n' "$code" > "$run/conformance.exit"
printf '%s\n' "$run"
exit "$code"
