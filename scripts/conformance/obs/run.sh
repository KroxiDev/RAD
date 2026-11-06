#!/usr/bin/env bash
# Conformidad aislada; prepara artefactos y ejecuta una vez.
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/gate-env.sh
exec python3 -B scripts/gate.py conformance obs
