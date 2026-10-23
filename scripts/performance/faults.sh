#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
mkdir -p target/performance
run="target/performance/faults-$(date -u +%Y%m%dT%H%M%S)-$$"
build="$run-build"
mkdir "$build"
flock /tmp/rad-phase2-cargo.lock cargo build --offline --locked --release -p rad-runtime -p rad-editor -p rad-plugin-host --all-features --bins --examples > "$build/examples.log" 2>&1
flock /tmp/rad-phase2-cargo.lock cargo test --offline --locked --release -p rad-runtime -p rad-store --all-features --lib --no-run --message-format=json > "$build/runtime-artifacts.jsonl" 2> "$build/runtime-build.log"
CARGO_TARGET_DIR="$PWD/target/plugins" flock /tmp/rad-phase2-cargo.lock cargo test --manifest-path plugins/Cargo.toml --offline --locked --release -p rad-plugin-media --all-features --lib --no-run --message-format=json > "$build/media-artifacts.jsonl" 2> "$build/media-build.log"
mapfile -t artifacts < <(python3 -I -S -B - "$build/runtime-artifacts.jsonl" "$build/media-artifacts.jsonl" <<'PY'
import json,sys
found={}
for path in sys.argv[1:]:
 for line in open(path):
  row=json.loads(line)
  name=row.get('target',{}).get('name')
  if row.get('reason')=='compiler-artifact' and row.get('profile',{}).get('test') and row.get('executable'):
   found[name]=row['executable']
for name in ['rad_runtime','rad_store','rad_plugin_media']:
 assert name in found,name
 print(found[name])
PY
)
test "${#artifacts[@]}" -eq 3
python3 -I -S -B scripts/performance/faults.py "$run" --runtime-tests "${artifacts[0]}" --store-tests "${artifacts[1]}" --media-tests "${artifacts[2]}" "$@"
cp -a "$build" "$run/build"
