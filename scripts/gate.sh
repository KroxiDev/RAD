#!/usr/bin/env bash
# Gate de docs/testing.md: preparación única y pruebas aisladas en paralelo.
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
# shellcheck source=scripts/gate-env.sh
source scripts/gate-env.sh
exec python3 -B scripts/gate.py "$@"
