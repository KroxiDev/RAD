#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/gate-env.sh
exec python3 -B scripts/gate.py conformance audio-media
