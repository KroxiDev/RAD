#!/usr/bin/env bash
# Gate de docs/testing.md: en serie, sin hardware ni ventanas.
# Deja en target/gate/<corrida>/ un log por paso, resumen.tsv y estado.txt.
# Uso: scripts/gate.sh            gate completo
#      scripts/gate.sh version    solo rad-update y packaging, tras un gate completo
set -uo pipefail
modo="${1:-completo}"
case "$modo" in
  completo | version) ;;
  *) echo "Uso: scripts/gate.sh [version]" >&2; exit 2 ;;
esac
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)" || exit 2
# shellcheck source=scripts/gate-env.sh
source scripts/gate-env.sh || exit 2

corrida="target/gate/$(date -u +%Y%m%dT%H%M%SZ)-$$"
mkdir -p "$corrida"

# Árbol del checkout tal como está (rastreado más nuevo no ignorado): identifica lo probado.
arbol() {
  local indice
  indice="$(mktemp)" || return 1
  cp "$(git rev-parse --git-path index)" "$indice" &&
    GIT_INDEX_FILE="$indice" git add -A &&
    GIT_INDEX_FILE="$indice" git write-tree
  rm -f "$indice"
}

arbol_inicio="$(arbol)" || exit 2
printf 'modo %s\ncommit %s\narbol %s\njobs %s\ninicio %s\n' \
  "$modo" "$(git rev-parse HEAD)" "$arbol_inicio" "$CARGO_BUILD_JOBS" "$(date -u +%FT%TZ)" \
  > "$corrida/estado.txt"
echo "Gate $modo en $corrida (jobs $CARGO_BUILD_JOBS)"

fallos=0
paso() {
  local nombre="$1" inicio=$SECONDS rc
  shift
  echo "== $nombre"
  "$@" > "$corrida/$nombre.log" 2>&1
  rc=$?
  printf '%s\t%s\t%s\n' "$nombre" "$rc" "$((SECONDS - inicio))" >> "$corrida/resumen.tsv"
  if (( rc != 0 )); then
    fallos=$((fallos + 1))
    echo "   falló ($rc): $corrida/$nombre.log"
  fi
}
plugins() { CARGO_TARGET_DIR="$CARGO_TARGET_DIR/plugins" "$@"; }

if [[ "$modo" == version ]]; then
  # rad-update compila packaging/release.json en su contrato.
  paso test-rad-update cargo test --offline --locked --release -p rad-update --all-features
else
  paso fmt-raiz cargo fmt --all -- --check
  paso build-raiz cargo build --offline --locked --release --workspace --all-features --bins --examples
  paso fmt-plugins plugins cargo fmt --manifest-path plugins/Cargo.toml --all -- --check
  paso build-plugins plugins cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples
  paso test-raiz cargo test --offline --locked --release --workspace --all-features
  paso clippy-raiz cargo clippy --offline --locked --release --workspace --all-features --all-targets -- -D warnings
  paso test-plugins plugins cargo test --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features
  paso clippy-plugins plugins cargo clippy --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --all-targets -- -D warnings
  paso conformidad-audio-media bash scripts/conformance/audio-media/run.sh
  paso conformidad-obs bash scripts/conformance/obs/run.sh
  paso conformidad-metrics bash scripts/conformance/metrics/run.sh
fi
paso packaging env PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s packaging/tests -v

arbol_fin="$(arbol)"
echo "fin $(date -u +%FT%TZ)" >> "$corrida/estado.txt"
if [[ "$arbol_fin" != "$arbol_inicio" ]]; then
  echo "arbol-final $arbol_fin" >> "$corrida/estado.txt"
  echo "El árbol cambió durante el gate: el resultado no certifica ningún estado." >&2
  fallos=$((fallos + 1))
fi

column -t -s $'\t' "$corrida/resumen.tsv"
if (( fallos > 0 )); then
  echo "GATE FALLÓ ($fallos): $corrida"
  exit 1
fi
echo "GATE OK ($modo)  árbol $arbol_inicio  ($corrida)"
