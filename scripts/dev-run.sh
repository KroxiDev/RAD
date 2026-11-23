#!/usr/bin/env bash
set -euo pipefail
umask 077

uso() {
  echo "Uso: scripts/dev-run.sh [--capture] [--plugin audio|media|obs|metrics [--real]]" >&2
  exit 2
}
plugin='' real='' capture=''
while (($#)); do
  case "$1" in
    --plugin) (($# >= 2)) || uso; plugin="$2"; shift 2 ;;
    --real) real='--real'; shift ;;
    --capture) capture=1; shift ;;
    *) uso ;;
  esac
done
case "$plugin" in '' | audio | media | obs | metrics) ;; *) uso ;; esac
if [[ -n $real && $plugin != obs && $plugin != metrics ]]; then
  echo "--real solo aplica a --plugin obs o metrics" >&2
  exit 2
fi

sesion="${XDG_RUNTIME_DIR:-}"
if [[ -z $capture ]]; then
  if [[ $sesion != /* || -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "Abrir desde la sesión gráfica: faltan XDG_RUNTIME_DIR o WAYLAND_DISPLAY." >&2
    exit 1
  fi
  command -v dbus-run-session > /dev/null || { echo "Falta dbus-run-session (paquete dbus)." >&2; exit 1; }
  [[ $WAYLAND_DISPLAY == /* ]] || WAYLAND_DISPLAY="$sesion/$WAYLAND_DISPLAY"
  export WAYLAND_DISPLAY
fi

repo="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$repo"
# shellcheck source=scripts/gate-env.sh
source scripts/gate-env.sh > /dev/null
echo "Compilando $repo"
cargo build --offline --locked --release --workspace --bins
if [[ -n $plugin ]]; then
  cargo build --offline --locked --release -p rad-runtime --example official_profile_seed
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR/plugins" cargo build --manifest-path plugins/Cargo.toml \
    --offline --locked --release --workspace --bins --examples
fi

mkdir -p target/dev-run
corrida="$repo/target/dev-run/$(date -u +%Y%m%dT%H%M%SZ)-$$"
install -d -m 700 "$corrida"
if [[ -n $plugin ]]; then
  target/release/examples/official_profile_seed "$corrida" "$plugin" ${real:+"--real-$plugin"} \
    > "$corrida/seed.log"
fi
install -d -m 700 "$corrida/data" "$corrida/config" "$corrida/state" "$corrida/cache" "$corrida/bin"
runtime_dir="$(mktemp -d "${sesion:-${TMPDIR:-/tmp}}/rad-dev.XXXXXX")"
if [[ -S $sesion/pulse/native ]]; then
  install -d -m 700 "$runtime_dir/pulse"
  ln -s "$sesion/pulse/native" "$runtime_dir/pulse/native"
fi
printf '#!/bin/sh\nexit 1\n' > "$corrida/bin/systemctl"
chmod 700 "$corrida/bin/systemctl"

export XDG_RUNTIME_DIR="$runtime_dir"
export XDG_DATA_HOME="$corrida/data"
export XDG_CONFIG_HOME="$corrida/config"
export XDG_STATE_HOME="$corrida/state"
export XDG_CACHE_HOME="$corrida/cache"

runtime_pid=''
cerrar() {
  local estado=$?
  trap - EXIT
  if [[ -n $runtime_pid ]]; then
    kill -TERM "$runtime_pid" 2> /dev/null || :
    wait "$runtime_pid" 2> /dev/null || :
  fi
  rm -rf "$runtime_dir"
  echo "Datos y logs: $corrida"
  exit "$estado"
}
trap cerrar EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

target/release/rad-runtime > "$corrida/runtime.log" 2>&1 &
runtime_pid=$!
for _ in $(seq 100); do
  target/release/rad-cli status > /dev/null 2>&1 && break
  kill -0 "$runtime_pid" 2> /dev/null || { echo "El runtime terminó: ver $corrida/runtime.log" >&2; exit 1; }
  sleep 0.1
done
target/release/rad-cli status > /dev/null 2>&1 ||
  { echo "El runtime no respondió en 10 s: ver $corrida/runtime.log" >&2; exit 1; }

if [[ -n $capture ]]; then
  install -d -m 700 "$corrida/capturas"
  RAD_CAPTURE_DIR="$corrida/capturas" PATH="$corrida/bin:$PATH" cargo test --offline --locked \
    --release --all-features -p rad-editor --lib app::capture::editor -- --ignored --exact --nocapture \
    > "$corrida/capture.log" 2>&1 || { echo "La captura falló: ver $corrida/capture.log" >&2; exit 1; }
  echo "Capturas en $corrida/capturas"
  exit 0
fi

echo "Editor abierto; al cerrarlo se detiene el runtime de prueba."
PATH="$corrida/bin:$PATH" dbus-run-session -- target/release/rad-editor > "$corrida/editor.log" 2>&1
