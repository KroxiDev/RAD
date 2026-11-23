rad_raiz="$(git rev-parse --show-toplevel)" || return 1
rad_principal="$(git -C "$rad_raiz" worktree list --porcelain | sed -n '1s/^worktree //p')"
if [[ "$rad_principal" != "$rad_raiz" ]]; then
  mkdir -p "$rad_raiz/target"
  [[ -e "$rad_raiz/target/cargo-home" ]] ||
    ln -s "$rad_principal/target/cargo-home" "$rad_raiz/target/cargo-home"
  for rad_dir in release plugins; do
    [[ -d "$rad_principal/target/$rad_dir" && ! -e "$rad_raiz/target/$rad_dir" ]] || continue
    if cp -a --reflink=always "$rad_principal/target/$rad_dir" "$rad_raiz/target/$rad_dir"; then
      echo "target/$rad_dir sembrado desde el checkout principal"
    else
      rm -rf "$rad_raiz/target/$rad_dir"
      echo "target/$rad_dir no admite reflink en este sistema de archivos: se compila desde cero" >&2
    fi
  done
fi

export CARGO_HOME="$rad_principal/target/cargo-home"
export CARGO_TARGET_DIR="$rad_raiz/target"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(( $(nproc) < 8 ? $(nproc) : 8 ))}"
export CARGO_INCREMENTAL=0
install -d -m 700 "$rad_raiz/target/test-session/runtime" "$rad_raiz/target/test-session/state"
export XDG_RUNTIME_DIR="$rad_raiz/target/test-session/runtime"
export XDG_STATE_HOME="$rad_raiz/target/test-session/state"
export RAD_TEST_LAUNCHER_HELPER="$CARGO_TARGET_DIR/release/rad-launcher-helper"
export RAD_TEST_LAUNCHER_FIXTURE="$CARGO_TARGET_DIR/release/rad-launcher-fixture"
unset rad_raiz rad_principal rad_dir
