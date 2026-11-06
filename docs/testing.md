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
- **Ver un cambio en el editor:** [capturas](#capturas) sin ventana; para usarlo
  con datos nuevos, [probar a mano](#probar-a-mano). No sustituyen al gate.
- **Hardware, escritorio o servicios reales:** requieren un encargo específico,
  aislamiento y control exclusivo. La [evidencia de rendimiento](performance.md#evidencia-y-límites)
  documenta escenarios medidos; no es permiso para repetirlos.

Registrar comandos, resultado, alcance y estado exacto probado. Un impedimento
se informa como no verificado, nunca como un pase. Los checks focales ayudan a
iterar, pero no sustituyen la validación completa de cambios ejecutables.

## Validación completa

Ejecutar `bash scripts/gate.sh`: es el gate canónico. Prepara los artefactos en
serie y ejecuta las suites independientes en paralelo, sin hardware ni ventanas.
Reutiliza la caché y no descarga dependencias ni instala herramientas.
El target queda en el disco del repo: los builds en tmpfs agotaron su cuota.

- Cada corrida deja en `target/gate/<corrida>/` un log por paso, `resumen.tsv`
  (paso, código de salida, segundos) y `estado.txt` con el commit y el árbol
  probados. Termina en `GATE OK`, o con código distinto de 0 si falló un paso o el
  árbol cambió durante la corrida.
- Compila con hasta 8 trabajos simultáneos, el mismo límite que `make up`;
  `CARGO_BUILD_JOBS` lo cambia.
- Compila los binarios, ejemplos y tests de ambos workspaces y el fixture de
  fallos de Metrics antes de ejecutar las suites. Los ejecutables de test se
  obtienen de la salida de Cargo de esa preparación, con el directorio del
  paquete como directorio de trabajo; no se eligen por fecha ni por un glob de
  artefactos anteriores. Los doctests se ejecutan también.
- Los cuatro verificadores de plugins corren en paralelo y conservan sus
  ventanas reales de vigencia y cancelación. Las conformidades integradas de
  Audio/Media, OBS y Metrics se ejecutan una vez cada una, conservando sus casos.
  Cada etapa de ejecución tiene directorios XDG privados. Los temporales son
  privados por etapa o los reserva el propio fixture bajo `/tmp`; sus rutas se
  mantienen cortas para respetar el límite de los sockets Unix.
- El entorno vive en [`scripts/gate-env.sh`](../scripts/gate-env.sh): caché,
  target y estado aislado de los tests. En un worktree secundario usa la caché
  del checkout principal y siembra `target/` desde él con copias reflink, así que
  solo recompila los crates del repo.

El objetivo de duración es **hasta 180 segundos con caché de compilación
reutilizable**. Medir el tiempo total de pared: los segundos de etapas paralelas
no se suman para obtenerlo. La compilación desde cero y la preparación o espera
de un runner de CI se informan por separado; un límite de tiempo que interrumpe
las comprobaciones no certifica un pase. Al cambiar la orquestación, comprobar
también que los fallos de procesos hijos y los cambios del árbol rechazan el gate.

Un gate completo en verde certifica el árbol de su `estado.txt`. Antes de
repetirlo, comparar con `git diff --stat <árbol> HEAD`:

- Sin diferencias, como al llevar la misma rama a `beta` y a `main`: citar ese
  resumen o el [check de la CI](#integración-continua) de ese commit.
- Solo documentación o `packaging/release.json`: la comprobación de documentación
  y `bash scripts/gate.sh version`, que prueba `rad-update` (compila
  `release.json`) y `packaging/tests`.
- Cualquier otro cambio: el gate completo.

Los binarios y ejemplos/helpers de **ambos workspaces** deben compilarse antes de
los tests de la raíz o focalizados: el runtime también usa fixtures de plugins,
como `held-converter-fixture`. No depender de artefactos de una ejecución anterior.
Las pruebas focales corren en el mismo entorno: `source scripts/gate-env.sh` en una
shell dedicada y, para el workspace de plugins, `CARGO_TARGET_DIR="$CARGO_TARGET_DIR/plugins"`.
Una comprobación
focal crítica es `cargo test --offline --locked --release -p rad-core --test permissions`.
Las conformidades usan fixtures de ambos workspaces y guardan salidas ignoradas
en `target/conformance/`.

Requisitos de tests: `bwrap`, `pactl`, `dbus-daemon` y namespaces Linux disponibles.
Audio usa un `pactl` falso salvo la prueba de producción sin servidor, que ejecuta
el real dentro de `bwrap`, sin acceso al audio personal. Multimedia usa un bus
D-Bus privado y reproductores falsos; OBS, servidores loopback efímeros; métricas,
archivos y NVML falsos. Si el sandbox no permite ejecutarlos, informar lo no
verificado: una suite verde no certifica hardware real ni todas las distribuciones.

## Integración continua

[`.github/workflows/gate.yml`](../.github/workflows/gate.yml) ejecuta
`bash scripts/gate.sh` en cada PR a `beta` o `main` y en cada push a esas ramas,
dentro de un contenedor de Arch con un usuario sin privilegios. Usa el toolchain
fijado en `RUST_TOOLCHAIN` del workflow, que se sube junto con el local. Conserva
ese toolchain en una caché propia y actualiza la caché de compilación por commit,
con respaldo por lockfiles y toolchain. Antes del Gate,
[`cache_mtimes.py`](../scripts/cache_mtimes.py) recupera las fechas de archivos
rastreados solo si su contenido SHA-256 y sus permisos coinciden con la estampa
guardada junto a los artefactos. Conserva también fechas de directorios con el
mismo inventario completo, como los catálogos que observa `build.rs`. Los archivos
nuevos, modificados, no rastreados y los enlaces no recuperan fechas antiguas.
La estampa anterior se consume antes de compilar y se registra una nueva solo
tras un Gate en verde: una caché de un build parcial fallido no lleva una estampa
que pueda ocultar cambios. Una caché anterior sin estampa sigue siendo utilizable,
pero puede requerir recompilar los crates del checkout.
La instalación de herramientas y la
descarga de dependencias tienen un paso separado del Gate. Los logs de
`target/gate/` quedan como artefacto `gate-logs`, tanto si pasa como si falla.

El check **Gate** en verde certifica el commit que probó, igual que un gate local:
al revisar ese mismo commit, citar el check en vez de repetir el gate. Otro commit
o un árbol con cambios locales siguen las reglas de la sección anterior.

## Probar a mano

`bash scripts/dev-run.sh` compila el checkout donde está el script (el principal o
un worktree) y abre su editor contra un runtime propio, con un perfil nuevo y sin
el Stream Deck. Requiere la sesión gráfica. Al cerrar la ventana se detiene ese
runtime; datos y logs (`runtime.log`, `editor.log`) quedan en
`target/dev-run/<corrida>/`.

- **Aislamiento:** runtime, datos, configuración, estado y caché usan directorios
  XDG propios. El editor corre en un bus D-Bus privado, así que convive con el RAD
  instalado abierto, y un `systemctl` que siempre falla le impide instalar o
  arrancar servicios. La entrada de menú y las unidades que escribe quedan dentro
  de la corrida.
- **Plugins:** `--plugin audio|media|obs|metrics` instala antes el plugin de este
  checkout, uno por corrida. Audio y multimedia actúan sobre el audio y los
  reproductores reales de la sesión; OBS y métricas usan fixtures salvo `--real`.
- **Límite:** «Usar el Stream Deck con RAD» empieza apagado. Encenderlo hace que
  la prueba compita por el deck con el RAD instalado; requiere control exclusivo.

### Capturas

`bash scripts/dev-run.sh --capture` arranca el mismo runtime y, en vez de abrir la
ventana, dibuja el editor real a PNG en `target/dev-run/<corrida>/capturas/`:
tablero, tecla seleccionada y Ajustes. No necesita sesión gráfica ni GPU y acepta
`--plugin`. Las escenas viven en [`app/capture.rs`](../crates/rad-editor/src/app/capture.rs),
un `#[test]` ignorado que el gate compila pero no ejecuta. Crean tres teclas por la
API pública y no comparan contra imágenes guardadas: un cambio visual no rompe
nada. Una tarea que necesite otra pantalla agrega su escena ahí mientras dura y la
retira al cerrar, como un test temporal.

## Contratos del actualizador

Las pruebas críticas de distribución son sintéticas y no certifican una sesión
personal ni una publicación:

| Comportamiento | Pruebas de referencia |
|---|---|
| Identidad/metadata, selección estable y comparación Arch | [`contract.rs`](../crates/rad-update/tests/contract.rs), [`releases.rs`](../crates/rad-update/tests/releases.rs) |
| URL/bytes/tamaño/digest/cancelación | [`download.rs`](../crates/rad-update/tests/download.rs) |
| Revalidación root, configuración y transacción limitada | [`installer.rs`](../crates/rad-update/tests/installer.rs) |
| Recolección de temporales, dueños/namespace, enlaces y leases | [`staging_gc.rs`](../crates/rad-update/tests/staging_gc.rs) y pruebas sintéticas de IPC/herencia en `coordinator.rs`/`installer.rs` |
| Exclusión de arranque, relevo/consentimiento y recuperación | [`lifecycle_gate.rs`](../crates/rad-update/tests/lifecycle_gate.rs), [`lifecycle_transaction.rs`](../crates/rad-update/tests/lifecycle_transaction.rs), [`lifecycle_recovery.rs`](../crates/rad-update/tests/lifecycle_recovery.rs) |
| Clic explícito, duplicados, cancelación y respuesta tardía | [`updates.rs`](../crates/rad-editor/tests/updates.rs) y pruebas headless de [`app/updates.rs`](../crates/rad-editor/src/app/updates.rs) |
| Layout, identidad del paquete, notices, entorno de build y publicación solo desde el `main` vigente | [`test_release.py`](../packaging/tests/test_release.py) |

## Ciclo de vida de los tests

Tests nativos de Rust; contratos públicos críticos en `crates/<paquete>/tests/`.
Controlar reloj, identidades e I/O en el dominio puro.

- Probar resultados y errores observables por la interface del módulo; un
  refactor que conserve el contrato debe conservar las pruebas. Mantener casos
  precisos por regla y recorridos integrados que comprueben la composición.
- Verificar archivos directamente cuando sean el contrato: manifests,
  formatos persistidos, traducciones, paquetes y dependencias permitidas.
  Contrastar identidades y versiones entre sus fuentes, sin fijar la versión
  actual en una prueba cuyo propósito sea otro. Evitar aserciones sobre nombres
  privados, organización del código o secuencias internas no contractuales.

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
| Iconos integrados | PNG opaco y antialias idéntico al preview ([renderer](../crates/rad-editor/tests/icon_library.rs)); captura del destino y borrador completo, rechazo antes de subir, fallo de upload/finish y Unknown sin replay ([publicación](../crates/rad-editor/tests/icon_save.rs)); cancelar, cambio de tecla, render/respuesta tardíos, conservación del borrador y ancho de la columna derecha estable durante 50 redibujados y devuelto a la paleta al cerrar ([estado local](../crates/rad-editor/src/app/icon_library.rs)); licencia Phosphor distribuida en el paquete |
| Dos estados | Manifest binario estricto y negociación; observación real y reposo sin texto; edición/vínculo atómicos y undo; dos imágenes retenidas juntas ante desconexión, GC/copias/reinicio; destino de biblioteca capturado por estado |
| Textos | Todas las claves existen en cada catálogo; ningún código en bruto en la UI; respaldo en inglés; cada texto de los plugins oficiales tiene traducción al español |
| Publicación y Store | Fallos de escritura, sync o rollback; before/after; reinicio; hold de evidencia corrupta; medios cambiados en la segunda pasada |
| Formatos | Golden real de cada versión; importadores de v1, v2, v3, v4, v5 y v6 en el Store y en la restauración; rechazo de versiones futuras y de campos v7 bajo etiqueta v6; migración de la posición base a las caras v6; brillo histórico 0–9 normalizado a 10 sin aceptar valores mayores de 100 |
| Estilo del texto | Misma tabla de resolución en dominio y API; Hidden conserva el texto guardado; estilo independiente por cara; campos base omitidos conservados y null restablecido; mapa de estados completo con revisión; caracterización RGB predeterminada del deck y escala de preview/rejilla |
| Iconos de aplicaciones | Demanda solo visible en Applications, límites de carga y reducción, caducidad de resultados positivos y negativos a los 30 s sin mostrarse |
| Brillo | Ajuste 10–100 y acción nativa subir/bajar diez puntos sobre el valor vigente, con saturación, sin escritura en el límite; publicación solo tras Applied, fallo e incertidumbre sin doble delta; cola vencida sin efecto tardío; Ajustes actualizado sin descartar borradores |
| Identidades | Contadores tras recuperación y restauración; poda tras el historial; límite sobre lo vigente con mensaje claro; un ShortId nunca reasignado |
| Copias | Exportación de un clic solo en Copias; sin inicio automático, duplicados ni borradores omitidos; destino privado sin sobrescritura y limpieza de incompletos ([pruebas de archivos](../crates/rad-editor/tests/backups.rs)); los tres destinos; reserva de ambos rangos; destino editado y deshecho hasta quedar vacío; deshacer tras restaurar; copia v1 real; ajustes de plugins restaurados; plugins ausentes ofrecidos para instalar |
| Host con runtime y Authority | Handshake, API, frames y correlaciones inválidas; flood; caída; residual de setup; deadline seguido de respuesta tardía o reap; parada y reap; selección distinta |
| Demanda | Metadata sin proceso, primera demanda, reutilización, última baja, reactivación durante la parada, preparación administrativa retenida |
| Credenciales | Revisión antigua y nueva, captura antes de esperar, Unknown, release con consumidor vivo, limpieza y ACK, desconexión |
| SDK y salida | Salida parcial o sin lector, concurrencia máxima sin cola creciente, baja y cierre, vigencia vencida en el host, fuente sana sin cambios, renovación sin carga |
| Operación | Clases de salida, tope de reinicios, modo recuperación, plugins e hijos que mueren con el runtime, aplicaciones que sobreviven en su scope, servicio instalado y arrancado sin autoinicio |
| Dispositivo | Detección por VID/PID en cualquier puerto, ausencia y reconexión, rejilla desde el modelo |
| Escritorio | Entrada del menú e ícono instalados y reescritos solo si cambian; bandeja registrada también cuando aparece tarde; un solo editor abierto; «Salir» con parada limpia; ninguna ventana flotante; editor oculto sin bloquearse; redibujado limitado a 60 fps |
| Capas | Test de arquitectura contra la tabla y sus invariantes |
| Audio, Media, OBS, Metrics | `pactl` colgado o con flood, con kill y reap; bus privado y epochs; backpressure y sin replay; CPU y RAM sin NVML y lector terminal |
| Distribución de RAD | Metadata enlazada a bytes/identidad Arch; layout sin fixtures ni datos; rechazo de fuente sucia para release; consulta manual y cancelación; relevo y privilegios probados con dobles |
| Instalación y paquetes | Configuración conservada al desinstalar y reinstalar, journal y reinicio, confianza explícita, cambio de origen, actualización desde el origen registrado, framing, límites y EOF |
