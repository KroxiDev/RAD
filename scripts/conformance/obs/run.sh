#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "$0")/../../.."
export CARGO_HOME="$PWD/target/cargo-home" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="$PWD/target"
export RAD_TEST_LAUNCHER_HELPER="$PWD/target/release/rad-launcher-helper"
export RAD_TEST_PLUGIN_FIXTURE="$PWD/target/release/rad-plugin-fixture"
export RAD_TEST_PLUGIN_FIXTURE_V1="$PWD/target/release/rad-plugin-fixture-v1"
export RAD_TEST_CREDENTIAL_FIXTURE="$PWD/target/release/rad-plugin-credentials-fixture"
run="target/conformance/obs-$(date -u +%Y%m%dT%H%M%S)-$$"
mkdir -p target/conformance
mkdir "$run"
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release --workspace --all-features --bins --examples > "$run/build-root.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples > "$run/build-plugins.log" 2>&1
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo test --manifest-path plugins/Cargo.toml --offline --locked --release -p rad-plugin-obs --all-features --example obs-conformance-server > "$run/oracle-tests.log" 2>&1
python3 -I -S -B scripts/conformance/audio-media/checker.py "$run" obs
# Build and locate exactly one conformance test executable.
flock /tmp/rad-phase2-cargo.lock cargo test --offline --locked --release -p rad-runtime --all-features --test official_obs --no-run --message-format=json > "$run/test-artifact.jsonl" 2> "$run/test-build.log"
python3 - "$run/test-artifact.jsonl" > "$run/test-executable.txt" <<'PYTHON'
import json,sys
paths={v['executable'] for line in open(sys.argv[1]) if (v:=json.loads(line)).get('reason')=='compiler-artifact' and v.get('target',{}).get('name')=='official_obs' and v.get('executable')}
assert len(paths)==1, paths
print(paths.pop())
PYTHON
for n in 1 2 3; do
 flock /tmp/rad-phase2-cargo.lock cargo test --offline --locked --release -p rad-runtime --all-features --test official_obs -- --ignored --exact official_obs_interoperates_with_runtime_and_cli --nocapture > "$run/conformance-$n.log" 2>&1
done
printf '%s\n' "$run"
