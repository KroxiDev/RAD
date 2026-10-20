# AGENTS.md — RAD · Rust Action Deck

## Proyecto y alcance

RAD configura y usa un Stream Deck mediante un editor visual y automatización local.
El runtime sin GUI es la única autoridad de configuración, ejecución y navegación y
el dueño del deck por hidraw. El editor eframe/egui (Glow) y `rad-cli` son clientes
del socket local; cada plugin activo tiene su propio proceso.

Entrypoints: `crates/rad-runtime/src/bin/rad-runtime.rs`,
`crates/rad-editor/src/main.rs` y `crates/rad-client/src/bin/rad-cli.rs`.
El código y los tests son la fuente de verdad. La historia inicial está en
[KroxiDev/RAD-Fase-Inicial](https://github.com/KroxiDev/RAD-Fase-Inicial).

El alcance inicial está cerrado. Siguen autorizadas las mejoras del editor y los
cambios funcionales pedidos para ello: simple, oscuro y profesional, cercano a
Elgato sin clonar su identidad; cada control visible solo cuando se pueda usar.
Ampliar alcance requiere pedido del usuario. Prioridades: estabilidad, respuesta
percibida, UX y después ahorro marginal de recursos.

## Procedimientos

- Antes de planificar o editar: leer `.agentic/golden-rules.md`.
- Para comportamiento ejecutable: aplicar `.agentic/skills/tdd.md`.
- Para revisar estándares y requisitos: `.agentic/skills/code-review.md`.
- Para recuperar o guardar contexto: `.agentic/skills/usar-engram.md`.
- Antes de localizar o comprender código: `.agentic/skills/usar-codegraph.md`;
  consultar el índice disponible o recurrir a búsqueda y lectura directa.
- Para tareas complejas ya especificadas: `.agentic/skills/orquestar.md`, con los
  roles de `.agentic/agents/`. Las tareas simples se resuelven directamente.
- Para editar instrucciones de agentes: aplicar el skill de escritura para
  agentes disponible en el entorno. La capa compartida vive en `.agentic/`.

## Validación completa

Ejecutar en serie desde la raíz, sin hardware ni ventanas. Reutilizar la caché;
no descargar dependencias ni instalar herramientas por inferencia. El target debe
quedar en el disco del repo: los builds en tmpfs agotaron su cuota.

```bash
export CARGO_HOME="$PWD/target/cargo-home"
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
cargo fmt --all -- --check
cargo build --offline --locked --release --workspace --all-features --bins --examples
export RAD_TEST_LAUNCHER_HELPER="$CARGO_TARGET_DIR/release/rad-launcher-helper"
export RAD_TEST_LAUNCHER_FIXTURE="$CARGO_TARGET_DIR/release/rad-launcher-fixture"
cargo test --offline --locked --release --workspace --all-features
cargo clippy --offline --locked --release --workspace --all-features --all-targets -- -D warnings
(
  export CARGO_TARGET_DIR="$PWD/target/plugins"
  cargo fmt --manifest-path plugins/Cargo.toml --all -- --check
  cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples
  cargo test --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features
  cargo clippy --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --all-targets -- -D warnings
)
bash scripts/conformance/audio-media/run.sh
bash scripts/conformance/obs/run.sh
bash scripts/conformance/metrics/run.sh
```

Los ejemplos/helpers deben compilarse antes de tests focalizados. Una comprobación
focal crítica es `cargo test --offline --locked --release -p rad-core --test permissions`.
Las conformidades usan fixtures de ambos workspaces y guardan salidas ignoradas
en `target/conformance/`.

Requisitos de tests: `bwrap`, `pactl`, `dbus-daemon` y namespaces Linux disponibles.
Audio usa un `pactl` falso salvo la prueba de producción sin servidor, que ejecuta
el real dentro de `bwrap`, sin acceso al audio personal. Multimedia usa un bus
D-Bus privado y reproductores falsos; OBS, servidores loopback efímeros; métricas,
archivos y NVML falsos. Si el sandbox no permite ejecutarlos, informar lo no
verificado: una suite verde no certifica hardware real ni todas las distribuciones.

## Ciclo de vida de los tests

Tests nativos de Rust; contratos públicos críticos en `crates/<paquete>/tests/`.
Controlar reloj, identidades e I/O en el dominio puro.

- Conservar sin preguntar las pruebas de comportamiento crítico. Cualquier otra
  prueba permanente requiere pedido explícito.
- Durante desarrollo se permiten tests temporales, identificados como tales.
  Al cerrar, conservar solo los críticos quitando su marca; retirar los demás,
  salvo pedido explícito de conservarlos.
- Ejecutar y conservar los tests existentes. La limpieza afecta únicamente a
  temporales de la tarea, salvo autorización específica.
- Un temporal visto fallar por el comportamiento y luego pasar aporta evidencia;
  no dejarlo permanente no incumple por sí solo la regla 3.5 de las golden rules.

## Git y GitHub

- Rama por defecto `main`; `origin` es `KroxiDev/RAD`, privado. Antes de editar,
  comprobar rama, status y diff; preservar cambios ajenos.
- Commit y push progresivos de entregas verificadas están autorizados. Mensajes
  breves en inglés con Conventional Commits (`fix:`, `feat:`, `docs:`, etc.).
- Crear/cambiar/borrar ramas, reescribir historia o descartar cambios requiere
  autorización explícita y específica, incluso clean/reset/restore destructivos
  y force-push. No abrir PR ni configurar remotos por inferencia.
- Para GitHub usar el MCP `github`. Usar `gh` solo si falta conexión/operación o
  falla el MCP, explicando el motivo. El checkout se opera con `git`; con checkout
  local no escribir archivos directamente al remoto mediante MCP.

## Seguridad

- Preservar secretos: no leer ni mostrar tokens, claves o archivos `.env`.
- Operar el XL solo con autorización y control exclusivo. Nunca RAD y
  StreamController escribiendo simultáneamente; preservar todos los datos de
  StreamController, sin migrarlos ni modificarlos.
- Probar fallos con escenarios sintéticos controlados, sin sondeos generales de
  sensores ni reproducción de bloqueos del driver.
- Instalar herramientas, publicar o acceder a remotos requiere autorización.
  La licencia MIT no autoriza hacer público este repositorio privado.
- Aislamiento estricto: no tocar, reiniciar, reconstruir, migrar, detener ni leer
  secretos de otros proyectos sin aprobación explícita y específica.
- Al retirar archivos sin respaldo comprobado en un repo, usar la papelera,
  no eliminación permanente.
