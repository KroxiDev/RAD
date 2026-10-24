# Pruebas y validación

Procedimiento canónico para desarrolladores y agentes. Las reglas de seguridad y
las autorizaciones se mantienen en [AGENTS.md](../AGENTS.md#seguridad). Esta guía
no concede permiso para operar hardware, servicios o datos personales.

## Elegir la comprobación

- **Comportamiento ejecutable:** aplicar [TDD](../.agentic/skills/tdd.md), ejecutar
  las pruebas focales y completar el gate de ambos workspaces antes de declarar
  la entrega verificada.
- **Solo documentación:** comprobar enlaces locales y anclas, referencias a
  fuentes, bloques de código y `git diff --check`. Leer también los archivos
  nuevos: el diff de Git no incluye los que aún no están rastreados. Esto valida
  documentación, no equivale a ejecutar tests de producto.
- **Hardware, escritorio o servicios reales:** requieren un encargo específico,
  aislamiento y control exclusivo. La [evidencia de rendimiento](performance.md#evidencia-y-límites)
  documenta escenarios medidos; no es permiso para repetirlos.

Registrar comandos, resultado, alcance y estado exacto probado. Un impedimento
se informa como no verificado, nunca como un pase. Los checks focales ayudan a
iterar, pero no sustituyen la validación completa de cambios ejecutables.

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

## Matriz de regresiones

Casos que se conservan, o se añaden si falta cobertura.

| Contrato | Casos |
|---|---|
| Cliente y UI | Desconexión tras el envío, worker muerto, sin replay, crédito único, vistas coherentes por revisión, avisos coalescidos, Unknown resuelto releyendo, cursor inválido o parcial, salida JSON de `rad-cli` |
| Textos | Todas las claves existen en cada catálogo; ningún código en bruto en la UI; respaldo en inglés |
| Publicación y Store | Fallos de escritura, sync o rollback; before/after; reinicio; hold de evidencia corrupta; medios cambiados en la segunda pasada |
| Formatos | Golden real de cada versión; importadores de v1, v2 y v3 en el Store y en la restauración; rechazo de versiones futuras |
| Identidades | Contadores tras recuperación y restauración; poda tras el historial; límite sobre lo vigente con mensaje claro; un ShortId nunca reasignado |
| Copias | Los tres destinos; reserva de ambos rangos; destino editado y deshecho hasta quedar vacío; deshacer tras restaurar; copia v1 real; ajustes de plugins restaurados; plugins ausentes ofrecidos para instalar |
| Host con runtime y Authority | Handshake, API, frames y correlaciones inválidas; flood; caída; residual de setup; deadline seguido de respuesta tardía o reap; parada y reap; selección distinta |
| Demanda | Metadata sin proceso, primera demanda, reutilización, última baja, reactivación durante la parada, preparación administrativa retenida |
| Credenciales | Revisión antigua y nueva, captura antes de esperar, Unknown, release con consumidor vivo, limpieza y ACK, desconexión |
| SDK y salida | Salida parcial o sin lector, concurrencia máxima sin cola creciente, baja y cierre, vigencia vencida en el host, fuente sana sin cambios, renovación sin carga |
| Operación | Clases de salida, tope de reinicios, modo recuperación, plugins e hijos que mueren con el runtime, aplicaciones que sobreviven en su scope, servicio instalado y arrancado sin autoinicio |
| Dispositivo | Detección por VID/PID en cualquier puerto, ausencia y reconexión, rejilla desde el modelo |
| Escritorio | Entrada del menú e ícono instalados y reescritos solo si cambian; bandeja registrada también cuando aparece tarde; un solo editor abierto; «Salir» con parada limpia; ninguna ventana flotante |
| Capas | Test de arquitectura contra la tabla y sus invariantes |
| Audio, Media, OBS, Metrics | `pactl` colgado o con flood, con kill y reap; bus privado y epochs; backpressure y sin replay; CPU y RAM sin NVML y lector terminal |
| Instalación y paquetes | Configuración conservada al desinstalar y reinstalar, journal y reinicio, confianza explícita, cambio de origen, actualización desde el origen registrado, framing, límites y EOF |
