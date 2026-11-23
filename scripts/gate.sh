#!/usr/bin/env bash
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
# shellcheck source=scripts/gate-env.sh
source scripts/gate-env.sh
exec python3 -B scripts/gate.py "$@"
