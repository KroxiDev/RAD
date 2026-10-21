# Plan por fases

Trabajo pendiente para llegar a la [arquitectura](architecture.md). Cada fase se
ejecuta por encargo del usuario y empieza por su diseño; después se aplican el ciclo
TDD y la validación completa de [AGENTS.md](../AGENTS.md). Un ítem está terminado
cuando pasan sus pruebas; al cerrarlo, se añade «— hecho (commit)» a su título.

La evidencia cita el código en `662115c`; las líneas pueden haberse movido.

## Fase 0 · Decisiones — hecha (2026-10-20)

Arquitectura, ADR, norma de interfaz y este plan en el repo, con AGENTS.md alineado.

## Fase 1 · Datos y formato — hecha (d7424f0)

En esta fase se congeló el formato v2, documentado en
[Formatos](formats.md). Validación completa de ambos workspaces y las tres
conformidades superada sin hardware ni ventanas.

### A1 · Restaurar una copia en otra instalación — hecho (d7424f0)

- **Evidencia:** en una instalación nueva, `prepare_restore` devuelve
  `Identity(ShortIdCollision)`: toda instalación nace con `f1` y una identidad
  aleatoria (`crates/rad-runtime/src/bootstrap.rs:57`) y el registro rechaza un `f1`
  con otra identidad (`crates/rad-core/src/identity.rs:221`). En la misma instalación
  funciona.
- **Decisión:** [ADR 0003](adr/0003-copias-portables.md).
- **Pruebas:** los tres destinos; IDs conservados en un destino nunca editado; IDs
  nuevos por encima de ambos rangos cuando hay contenido, sin preguntas, de modo que
  ningún script del destino ni del backup actúa sobre otro botón; un destino editado
  y deshecho hasta quedar vacío cuenta como destino con contenido; deshacer tras
  restaurar; una copia v1 real (golden); ajustes de plugins restaurados; plugins
  ausentes ofrecidos para instalar.

### A2 · Límite de identidades sobre lo vigente — hecho (d7424f0)

- **Evidencia:** tope de 10.000 que cuenta el histórico
  (`crates/rad-store/src/ledger.rs:5`). Cada guardado lee, compara e inserta el
  registro completo, con una comparación cuadrática
  (`crates/rad-store/src/commit.rs:26–29,53,60`). Crear una carpeta consume 5
  identidades (`crates/rad-core/src/configuration/edits.rs:69–71,280,304–308,347`).
  El agotamiento llega al cliente como `NotApplied` sin causa
  (`crates/rad-runtime/src/persistence.rs:342`).
- **Decisión:** [ADR 0002](adr/0002-identidades-sobre-lo-vigente.md). Además:
  comparar las listas ordenadas por mezcla, insertar solo las identidades nuevas y
  propagar `resource_exhausted` con su causa.
- **Pruebas:** contadores correctos tras recuperación y restauración; la poda nunca
  permite reasignar un ShortId; el límite produce un mensaje claro; coste del guardado
  con 10.000 identidades, antes y después.
- **Medición:** perfil sintético con 10.000 identidades retenidas, una vigente y
  diez guardados de brillo: 540,609 ms en v1 frente a 249,976 ms en v2, tiempo total.
  Una muestra por versión; no representa 10.000 entidades vigentes ni fija un
  presupuesto de rendimiento.

### A5 · Tres tipos de ajustes — hecho (d7424f0)

- **Evidencia:** el idioma vive en `crates/rad-core/src/configuration/settings.rs`
  (`gui_language`) y el runtime nunca lo usa.
- **Decisión:** [ADR 0005](adr/0005-tres-tipos-de-ajustes.md). En esta fase el idioma
  pasa a las preferencias del editor, existe el almacén de ajustes de la máquina y el
  formato reserva el lugar de los ajustes de plugins. «Usar el Stream Deck con RAD»
  reemplaza a `--xl-exclusive` en la fase 3, junto con el servicio (B2).
- **Pruebas:** el idioma no está en el documento ni en las copias; las preferencias
  del editor no viajan; golden con el lugar de los ajustes de plugins.

### A6 · Rejilla en el formato — hecho (d7424f0)

- **Evidencia:** `KEY_COUNT = 32` (`crates/rad-core/src/input.rs:10`), carpetas de 32
  (`crates/rad-core/src/configuration.rs:87`; `configuration/edits.rs:310`), el
  editor (`crates/rad-editor/src/session.rs:106`) y arrays de 32 en el worker del deck.
- **Decisión:** [ADR 0006](adr/0006-dispositivo-por-modelo.md): la rejilla se guarda
  en el formato y las constantes del XL se agrupan en un tipo `Grid`, sin implementar
  otros modelos.
- **Pruebas:** golden del formato con la rejilla; el comportamiento del XL no cambia.

### D3 · Corte limpio e importador de v1 — hecho (d7424f0)

- **Evidencia:** `pauses_ms` (`crates/rad-codec/src/document.rs:56–58`); `Settings`
  con `deny_unknown_fields` y `gui_language` obligatorio (`document.rs:127`); el
  codec rechaza toda versión distinta de 1 (`crates/rad-codec/src/lib.rs:47`); el
  Store exige `user_version` 1 (`crates/rad-store/src/lib.rs:145–148`).
- **Decisión:** [ADR 0004](adr/0004-evolucion-de-formatos.md).
- **Pruebas:** migración de un Store v1 real que conserva el archivo anterior;
  restauración de una copia v1 real; rechazo explícito de versiones futuras; golden
  de cada formato, generados desde un perfil de prueba.

### D2 · Capas verificadas — hecho (d7424f0)

- **Evidencia:** `rad-package` depende de `rad-store` por `MediaSource`, `MediaEntry`
  y `MediaReadError` (`crates/rad-store/src/source.rs:3–16`).
- **Decisión:** mover esos tipos a `rad-media` y añadir el test de arquitectura
  ([Capas](architecture.md#capas)).
- **Pruebas:** el test corre dentro de `cargo test`, sin red, y falla ante una
  dependencia que no figura en la tabla.

## Fase 2 · Contratos — hecha (379bee2)

Contrato de clientes en [client-api.md](client-api.md), `rad-api` compartido y
editor dirigido por vistas y avisos. Contrato de plugins major 1, SDK compartido
y arnés genérico, con [especificación y guía](https://github.com/KroxiDev/rad-plugins/tree/sdk-v0.2.0/docs)
publicadas y enlazadas desde `plugins/README.md`. SDK fijado en `sdk-v0.2.0`;
plugins oficiales republicados en
[audio-v0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.2.0),
[media-v0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/media-v0.2.0),
[obs-v0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/obs-v0.2.0) y
[metrics-v0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/metrics-v0.2.0),
con paquetes de producto, manifests, licencias, checksums y reportes de alcance.

La selección de un gesto por botón exigió la evolución autorizada a documento v3,
con importadores de v1 y v2 y umbral independiente de pulsación larga de 500 ms
iniciales ([ADR 0015](adr/0015-gesto-del-plan-y-documento-v3.md)). Store y RADPKG
conservan su versión exterior 2.

Validación completa en serie de ambos workspaces y las tres conformidades
superada, sin hardware ni ventanas. Las conformidades usan backends sintéticos de
la misma implementación y manifests idénticos; los reportes de las releases
distinguen esos fixtures del ejecutable de producto y no certifican hardware real.

### A3 · Contrato de plugins v1 — hecho (379bee2)

- **Evidencia:** solo se notifica «pulsada», sin soltar ni pulsación larga
  (`crates/rad-core/src/input.rs:10–175`, mensaje `Invoke`); cinco tipos de campo
  cerrados, y uno desconocido invalida el manifest
  (`crates/rad-plugin-contract/src/manifest.rs:74–80`); sin ajustes por plugin, con
  OBS fijo en `127.0.0.1:4455` (`plugins/obs/src/plugin.rs:82`); stderr a `/dev/null`
  (`crates/rad-plugin-host/src/worker.rs:231`); versión lineal con condicionales
  `api >= 2` y `api >= 3` (`crates/rad-plugin-contract/src/lib.rs:3–5`); conformidad
  con arneses a medida (`scripts/conformance/*`; `crates/rad-runtime/tests/official_*.rs`).
- **Decisión:** [ADR 0007](adr/0007-contrato-de-plugins-v1.md),
  [ADR 0008](adr/0008-frescura-declarada.md) y [ADR 0009](adr/0009-confianza-en-plugins.md).
- **Entregables:** contrato v1, `rad-plugin-check`, especificación y guía en
  `KroxiDev/rad-plugins`, plugins oficiales republicados y `scripts/conformance/`
  sobre el arnés genérico.
- **Pruebas:** el arnés y los vectores de referencia; los plugins oficiales pasan el
  arnés.

### A4 · Una sola definición de los protocolos — hecho (379bee2)

- **Evidencia:** `crates/rad-plugin-contract/src/protocol.rs` frente a
  `plugins/sdk/src/protocol.rs`, que ya difieren (por ejemplo, en `Outcome`); las
  peticiones de clientes se interpretan a mano en `rad-protocol`
  (`crates/rad-protocol/src/lib.rs:53–554`), en el editor
  (`crates/rad-editor/src/lib.rs:3–15`) y en los `check.py` de conformidad.
- **Decisión:** [ADR 0007](adr/0007-contrato-de-plugins-v1.md) y
  [ADR 0010](adr/0010-api-de-clientes.md).
- **Pruebas:** vectores probados en ambos lados y comprobación en compilación.

### API · `rad-api`, vistas y avisos — hecho (379bee2)

- **Evidencia:** el editor consulta el estado cada 250 ms con foco y cada 1 s sin foco
  (`crates/rad-editor/src/lib.rs:36–50`), recarga una carpeta con hasta 66 peticiones
  (`crates/rad-editor/src/session.rs:87–140`) y repinta cada 50 ms
  (`crates/rad-editor/src/app.rs:704`).
- **Decisión:** [ADR 0010](adr/0010-api-de-clientes.md).
- **Pruebas:** vistas coherentes por revisión; avisos coalescidos; Unknown resuelto
  releyendo; salida JSON de `rad-cli` con versión; editor sin sondeo ni repintado
  periódico.

### B8 · Multimedia puede mostrar un estado viejo como vigente — hecho (379bee2)

- **Evidencia:** el hilo hace `wait → reader.read → publish` sin plazo para leer y
  convertir la carátula, y el heartbeat repite el último estado
  (`plugins/media/src/watch.rs:99–113`; `plugins/media/src/cover.rs:184–207`;
  `plugins/sdk/src/subscription.rs:93–108`).
- **Decisión:** [ADR 0008](adr/0008-frescura-declarada.md), conservando los límites de
  carátulas.
- **Pruebas:** fuente sana sin cambios, lector bloqueado y renovación sin carga.

### B9 · La salida del SDK bloquea — hecho (379bee2)

- **Evidencia:** `write_all` y `flush` bajo un mutex global; el heartbeat escribe
  reteniendo el registro de suscripciones (`plugins/sdk/src/writer.rs:28–31`;
  `plugins/sdk/src/subscription.rs:163–200`).
- **Decisión:** salida no bloqueante, con presupuesto y cierre terminal, como ya hace
  OBS (`plugins/obs/src/output.rs:31–83`); [ADR 0007](adr/0007-contrato-de-plugins-v1.md).
- **Pruebas:** el arnés con un host que no lee.

### B10 · Reparto de los trabajos de fondo del editor — hecho (379bee2)

- **Evidencia:** la cadena `else if` de `App::pump`
  (`crates/rad-editor/src/app.rs:644–698`) deja que el refresco periódico monopolice
  los turnos.
- **Decisión:** con avisos desaparece el refresco periódico; si el problema persiste,
  una cuota por categoría.
- **Pruebas:** tras los avisos, ningún trabajo de fondo queda postergado.

## Fase 3 · Operación y robustez — hecha (2ac29f9)

Salidas clasificadas, modo recuperación y unidad de usuario con reinicio acotado;
el editor instala y arranca el servicio, y «Usar el Stream Deck con RAD» reemplaza
a `--xl-exclusive`. El deck se busca por VID/PID y serie en cualquier puerto; los
plugins y sus descendientes terminan con el runtime y cada aplicación corre en su
propio scope.

Validación completa en serie de ambos workspaces y las tres conformidades
superada, sin hardware ni ventanas. Sin verificar con el XL, systemd ni la GUI
reales. Parte de B2 tuvo una validación mínima y queda por endurecer; los límites
conocidos están en la [PR #1](https://github.com/KroxiDev/RAD/pull/1).

### B1 · El deck está fijado al puerto USB 1-5 — hecho (cff2593)

- **Evidencia:** `crates/rad-deck/src/hidraw.rs:19`; `DeviceConfig` vacío
  (`crates/rad-deck/src/lib.rs:90–97`).
- **Decisión:** [ADR 0006](adr/0006-dispositivo-por-modelo.md): buscar en
  `/sys/class/hidraw` por VID/PID (y número de serie), conservando las comprobaciones
  actuales.
- **Pruebas:** sysfs sintético con el dispositivo en otro puerto, dos nodos, ausencia
  y driver ajeno.
- **Resultado:** con varios XL conectados se usa el de menor número de serie (decisión
  del usuario); un XL sin serial válido no es elegible. Sin verificar con el XL real.

### B2 · Salidas, modo recuperación, servicio y dispositivo como ajuste — hecho (55f67a2, a51c8b7, 6fbc403, 910335c, ce4e274, ad92e89, bb1f667)

- **Evidencia:** cualquier error de `server.turn()` termina el runtime
  (`crates/rad-runtime/src/bin/rad-runtime.rs:71`); la unidad usa `Restart=no`
  (`packaging/rad-runtime.service:16`); el editor dice «Inicia rad-runtime»
  (`crates/rad-editor/src/app/bar.rs:77–78`); el control del deck exige
  `--xl-exclusive` (`README.md:53,61`).
- **Decisión:** [ADR 0011](adr/0011-salidas-y-modo-recuperacion.md),
  [ADR 0012](adr/0012-servicio-gestionado-por-el-editor.md) y el ajuste «Usar el
  Stream Deck con RAD» de [ADR 0006](adr/0006-dispositivo-por-modelo.md).
- **Pruebas:** cada clase de salida devuelve su código; tope de reinicios; los errores
  contenidos no terminan el proceso; en modo recuperación, restaurar y empezar de
  nuevo conservan el archivo dañado y pasan a modo normal sin reiniciar; el editor
  instala la unidad y la arranca con el autoinicio desactivado; el autoinicio solo la
  habilita.

### B3 · Los plugins y sus descendientes no mueren con el runtime — hecho (eeb1c4b, 8c93228)

- **Evidencia:** `configure_command` no usa `PR_SET_PDEATHSIG` ni grupo de procesos
  (`crates/rad-plugin-host/src/worker.rs:143`); la unidad usa `KillMode=process`
  (`packaging/rad-runtime.service:14`).
- **Decisión:** [ADR 0013](adr/0013-limpieza-de-procesos.md).
- **Pruebas:** con un fixture que crea un hijo, al parar el plugin no queda el hijo;
  con un fixture que ignora el EOF, al matar el runtime no queda el plugin; la
  creación del scope de una aplicación, contra un systemd falso.

### B4 · Una actualización puede llegar de otro origen sin aviso — hecho (18a36f3)

- **Evidencia:** `crates/rad-plugin-manager/src/store.rs:253–255`.
- **Decisión:** [ADR 0009](adr/0009-confianza-en-plugins.md).
- **Pruebas:** actualizar desde otro origen exige confirmación y registra el nuevo
  origen.

### B5 · Un worker de UI muerto queda ocupado — hecho (b51b9ce)

- **Evidencia:** si `run(job)` hace panic, `poll` convierte Disconnected en None y
  `busy` no se limpia nunca, así que el editor espera para siempre
  (`crates/rad-editor/src/worker.rs:16–76`).
- **Decisión:** representar Closed y Failed de forma explícita; marcar como Unknown
  una mutación pendiente y resolverla releyendo, sin repetirla; recrear workers de
  forma acotada, sin bucles.
- **Pruebas:** un worker sintético que hace panic tras aceptar un trabajo.

### B6 · Preparar un plugin retrasa el arranque en frío de otro — hecho (ccba93a)

- **Evidencia:** el worker administrativo queda `busy` durante la preparación, que
  incluye una descarga de hasta 600 s, y el mismo worker hace los Spawn
  (`crates/rad-runtime/src/plugin_admin.rs:148–161,319–320`;
  `crates/rad-runtime/src/plugin_admin/worker.rs:53–104`;
  `crates/rad-plugin-manager/src/download.rs:26–29`). Solo ocurre durante
  instalaciones o actualizaciones.
- **Decisión:** separar la preparación de red y paquete del spawn y el reap,
  manteniendo serializados el selector, los permits y la publicación.
- **Pruebas:** un downloader falso retenido y la demanda de otro plugin, sin red.

### B7 · El deadline del Host se trata como terminación — hecho (9a1bc7f)

- **Evidencia:** `crates/rad-plugin-host/src/worker.rs:63–99,465–476`;
  `crates/rad-runtime/src/plugin_dispatch.rs:191–202`;
  `crates/rad-authority/src/lib.rs:838–849`.
- **Decisión:** [ADR 0014](adr/0014-residual-de-plugins-en-el-host.md).
- **Pruebas:** un fixture lento con el deadline vencido: el resultado visible aparece
  una sola vez, Engine libera su crédito y el Host retiene el residual hasta la
  respuesta o el reap, sin liberar credenciales ni repetir la acción; incluye la rama
  de caída del proceso.

### D1 · El bucle principal depende del orden de llamada — hecho (cb68a21)

- **Evidencia:** `admin.poll` se llama dos veces seguidas, con comentarios que
  explican el orden (`crates/rad-runtime/src/server.rs:610,621`).
- **Decisión:** repetir los pasos del turno hasta que ninguno avance, con un tope, y
  documentar las interfaces temporales de Authority (lease de export, permiso
  administrativo, avance de reloj y completion).
- **Pruebas:** secuencias intercaladas con los tests actuales de ciclo de vida.

## Fase UX · En paralelo

Norma: [ux.md](ux.md). Sobre la API actual: UX1, UX2, UX3, UX6 y UX7. Con la fase 2:
UX4 y UX8. Con la fase 3: UX5.

| Ítem | Hoy | Cambio |
|---|---|---|
| UX1 | El inspector muestra IDs (`crates/rad-editor/src/app/inspector.rs:638–660`) | Pasan a «Avanzado» |
| UX2 | Copiar o mover una tecla exige escribir el ID de una carpeta (`inspector.rs:610–636`) | Selector de carpeta por nombre, o arrastrar y soltar |
| UX3 | Los permisos de automatización aparecen en cada tecla y en Ajustes (`inspector.rs:598–608`; `crates/rad-editor/src/app/settings.rs:49–50`) | «Avanzado → Automatización», ocultos salvo que se activen |
| UX4 — hecho (379bee2) | Estados y mensajes técnicos, y códigos en bruto (`crates/rad-editor/src/app/plugins.rs:76–100`; `crates/rad-editor/src/status.rs:23,103,132`) | Los cuatro estados de plugin, mensajes con acción y resolución automática |
| UX5 — hecho (ad92e89, bb1f667) | Poner en marcha RAD exige la terminal (`crates/rad-editor/src/app/bar.rs:77–78`; `README.md:53,61,93`) | Servicio gestionado por el editor (B2) |
| UX6 | El progreso se muestra en bytes (`crates/rad-editor/src/app.rs:618`) | Barra de progreso |
| UX7 | Instalar un plugin exige pegar un enlace, también para los oficiales, y actualizar lo vuelve a pedir | Lista de oficiales y actualización desde el origen registrado |
| UX8 — hecho (379bee2) | Unos 200 pares de traducción escritos en el código (`crates/rad-editor/src/app.rs:111`) | Catálogos por idioma |

## Fase 4 · Medición

- Primero, un perfil reproducible; los presupuestos se fijan tras la primera
  medición. Se mantiene el objetivo de 20 MiB para métricas.
- Escenarios por separado: runtime con helper y sin editor; editor con foco, sin foco
  y cerrado; 0, 1 o N plugins demandados.
- Qué medir: RSS junto con PSS o memoria privada; reposo y pico; CPU y despertares por
  segundo; descriptores, hilos y procesos; latencia de tecla a acción en frío y en
  caliente; peor turno del bucle; cambio de carpeta; guardado con 10.000 identidades.
- Solo con PID propios y fixtures (downloader retenido, host que no lee, conversor
  bloqueado, worker muerto, escritura incierta), según la sección Seguridad de
  AGENTS.md.

| Ítem | Qué | Antes de cambiar |
|---|---|---|
| M1 | Authority reconstruye proyecciones completas tras cada cambio (`crates/rad-authority/src/queries.rs:208–282`) | Medir con configuraciones pequeñas y cercanas al límite antes de añadir índices |
| M2 | El guardado revalida los medios varias veces y recorre el registro entero (`crates/rad-store/src/lib.rs:192–215,341–378`; `commit.rs:25–35`; `media.rs:131–155`) | Medir con un catálogo grande, manteniendo checksums y recuperación |
| M3 | Despertares periódicos: repintado de 50 ms, sondeo de 250 ms, heartbeat de 400 ms por visual, incluso sin suscripciones (`plugins/sdk/src/runtime.rs:139`), polling de OBS cada 20 ms con sesiones abiertas (`plugins/obs/src/plugin.rs:340–353`) y sondeo de 500 ms sin deck | Gran parte desaparece con la fase 2; el sondeo sin deck es una excepción registrada |
| M4 | `Cargo.toml` sin `[profile.release]`; binarios de 14 a 16 MB | Medir memoria y arranque antes de strip o LTO: el tamaño en disco no es memoria residente |

## Documentación

- **D4 — hecho (379bee2):** README de multimedia actualizado a 0.2.0 y al límite
  de 32 aplicaciones; las cifras de RSS y CPU de 0.1.x se identifican como
  históricas, de una sola máquina, no como aceptación de la entrega actual.
- **Contratos — hecho (379bee2):** `docs/client-api.md` y especificación y guía
  de plugins publicadas con la fase 2.

## Matriz de regresiones

Casos que se conservan, o se añaden si falta cobertura.

| Contrato | Casos |
|---|---|
| Cliente y UI | Desconexión tras el envío, worker muerto, sin replay, crédito único, vistas coherentes por revisión, avisos coalescidos, Unknown resuelto releyendo, cursor inválido o parcial, salida JSON de `rad-cli` |
| Textos | Todas las claves existen en cada catálogo; ningún código en bruto en la UI; respaldo en inglés |
| Publicación y Store | Fallos de escritura, sync o rollback; before/after; reinicio; hold de evidencia corrupta; medios cambiados en la segunda pasada |
| Formatos | Golden real de cada versión; importadores de v1 y v2 en el Store y en la restauración; rechazo de versiones futuras |
| Identidades | Contadores tras recuperación y restauración; poda tras el historial; límite sobre lo vigente con mensaje claro; un ShortId nunca reasignado |
| Copias | Los tres destinos; reserva de ambos rangos; destino editado y deshecho hasta quedar vacío; deshacer tras restaurar; copia v1 real; ajustes de plugins restaurados; plugins ausentes ofrecidos para instalar |
| Host con runtime y Authority | Handshake, API, frames y correlaciones inválidas; flood; caída; residual de setup; deadline seguido de respuesta tardía o reap; parada y reap; selección distinta |
| Demanda | Metadata sin proceso, primera demanda, reutilización, última baja, reactivación durante la parada, preparación administrativa retenida |
| Credenciales | Revisión antigua y nueva, captura antes de esperar, Unknown, release con consumidor vivo, limpieza y ACK, desconexión |
| SDK y salida | Salida parcial o sin lector, concurrencia máxima sin cola creciente, baja y cierre, validez vencida en el host, fuente sana sin cambios, renovación sin carga |
| Operación | Clases de salida, tope de reinicios, modo recuperación, plugins e hijos que mueren con el runtime, aplicaciones que sobreviven en su scope, servicio instalado y arrancado sin autoinicio |
| Dispositivo | Detección por VID/PID en cualquier puerto, ausencia y reconexión, rejilla desde el modelo |
| Capas | Test de arquitectura contra la tabla y sus invariantes |
| Audio, Media, OBS, Metrics | `pactl` colgado o con flood, con kill y reap; bus privado y epochs; backpressure y sin replay; CPU y RAM sin NVML y lector terminal |
| Instalación y paquetes | Configuración conservada al desinstalar y reinstalar, journal y reinicio, confianza explícita, cambio de origen, actualización desde el origen registrado, framing, límites y EOF |
