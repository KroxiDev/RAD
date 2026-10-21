# API local de clientes · versión 1

La definición estructural vive en `rad-api`: `Request`, `Operation`, las vistas,
`Response`, `ResponseData`, `Notice`, `NoticeKind`, `ErrorCode` y los códigos de salida. Es una capa pura:
no abre sockets ni depende del dominio. `rad-protocol` traduce hacia Authority;
el runtime compone los adapters. Nunca se aceptan identidades de origen del JSON.

## Transporte y compatibilidad

Sockets Unix privados `rad/editor.sock` y `rad/automation.sock` bajo el directorio
runtime del usuario. Cada frame lleva longitud u32 big-endian seguida de JSON UTF-8
(1–65536 bytes). Una conexión empieza con `hello`; v1 se conserva compatible con
los clientes de automatización y fixtures anteriores. No hay replay automático.

```json
{"v":1,"id":"1","op":"hello","args":{}}
```

La respuesta correlaciona `v`, `id`, `session_id`, `status`, `data`, `error`.
`status` es `ok`, `applied`, `admitted` o `error`. IDs y contadores se representan
como cadenas decimales para evitar pérdidas en consumidores JavaScript. El
`session_id` identifica un arranque; cambiarlo invalida todas las vistas previas.
Las capacidades de `hello` anuncian únicamente operaciones implementadas y
permitidas en ese endpoint.

`Request` enum usa `op` como discriminante y `args` como contenido. Los objetos de
configuración arbitrarios pertenecen al esquema de cada plugin; no son un setter
universal del runtime. La validación semántica y de permisos sigue en Authority.

## Vistas compuestas del editor

- `board {folder_id: null | string}`: carpeta solicitada o activa, `grid` descrita
  por el runtime, `keys` (vacías o entidad + apariencia + estado visual) y
  `view_revision`. No se infieren 32 teclas ni dimensiones en el cliente.
- `inspector {id: string}`: entidad, apariencia cuando corresponde, todas las
  acciones y configuración del plugin cuando existe, con una `view_revision`.
- `overview {}`: `storage`, `deck`, `plugins` que requieren atención,
  `last_restart`, `previous_exit` (opcional), carpeta activa, ajustes y permisos
  globales e historial.

Cada proyección se construye sin ceder el actor ni hacer I/O entre componentes.
El cliente no debe mezclar silenciosamente board e inspector de revisiones
incompatibles: invalida/relee. Las imágenes se leen aparte (`media_read` o
`plugin_visual_read`); las vistas solo llevan referencias y metadatos.
Una vista que supera el presupuesto de respuesta de 48 KiB produce
`resource_exhausted`, nunca una vista truncada o parcialmente coherente.

`deck` distingue `disabled`, `connected`, `disconnected` e `inaccessible`.
`inaccessible` incluye el deck que el runtime no pudo iniciar: no termina por ello.
`storage` distingue `ready`, `uncertain`, `recovery_required`. El modo recuperación
de arranque es otra cosa: ver «Modo recuperación».
`last_restart` es `clean` o `recovered_previous` según la reconciliación de inicio.

`previous_exit` aparece en `overview` y en el `status` del Editor solo cuando el
arranque anterior terminó de forma inesperada (clase «inesperada» de
[ADR 0011](adr/0011-salidas-y-modo-recuperacion.md)); si no, el campo no existe.
Es `{code, at_unix_ms}`: `code` es un código estable, nunca prosa (`startup`,
`storage`, `transport`, `clock`, `protocol` o `panic`) y `at_unix_ms` los
milisegundos Unix del suceso, como cadena decimal. El runtime lo lee del archivo
privado `last-exit.json` de su directorio de datos y lo borra al arrancar: se
informa una vez, y un archivo ilegible se ignora. Las salidas limpias y las
bloqueantes no se registran; de ellas informa el estado de la unidad. Los
códigos de salida del proceso no forman parte de esta API: su tabla está en
`crates/rad-runtime/src/exit.rs`.

Los plugins instalados incompatibles aparecen una sola vez en `overview.plugins`
con `code: "needs_update"`, aunque no puedan registrarse ni ejecutarse. Su aparición
y desaparición invalidan Overview y Plugins; la vista conserva el límite de 48 KiB.

## Suscripción sin sondeo

El editor abre una **conexión dedicada** y solicita `subscribe {}`. Recibe primero
la respuesta correlacionada y después frames de aviso independientes:

```json
{"v":1,"session_id":"00000000000000000000000000000001","kind":"board","revision":"12"}
```

`kind` es `board`, `inspector`, `overview`, `catalog`, `dependencies`, `execution`
o `plugins`. Board incluye cambios de visuales/frescura; execution invalida el
resultado de ejecución observado; plugins invalida el estado administrativo.
Se emite un aviso inicial por tipo para cerrar la carrera suscribir/leer.
`revision` del aviso es una época de invalidación, no una revisión durable ni un
log: solo importa la igualdad, puede coalescer y no debe usarse para replay.

Transport conserva el último pendiente por tipo, con un espacio fijo de ocho
claves y payload máximo de 4096 bytes; hay siete tipos públicos. Un frame empezado
no se sustituye. La salida RPC conserva sus límites de 16 frames y 1 MiB; un
peer que no consume un frame empezado sigue sujeto al plazo de escritura de dos
segundos. Una suscripción quieta no expira por ausencia de peticiones.

`Client::subscribe` consume la conexión. `Subscription::recv` bloquea al worker
sin timeout periódico. `Subscription::cancellation` permite detenerlo con shutdown
y una bandera compartida, incluso si quedan frames buffered. El worker llama al
wake de la GUI al recibir aviso o resultado. Nunca se comparte esta lectura con
la lectura RPC síncrona. Tras una desconexión se abre una sesión nueva y se releen
vistas; no se repiten órdenes anteriores.

## Resultados inciertos y errores

`not_applied` asegura que la mutación no se aplicó. `unknown` significa que el
cliente no puede asegurar el resultado: vuelve a leer el estado o consulta el
resultado de ejecución conocido. **No se reintenta la acción automáticamente.**
Solo una nueva decisión del usuario puede repetirla. Los errores conservan código
y causa técnica; el editor traduce códigos con su catálogo, no usa prosa del servidor.

## Modo recuperación

Si el runtime no puede abrir la configuración (ADR 0011) no sale: conserva el lock de
instancia y los sockets, no toma el deck ni lanza plugins, y solo atiende lo siguiente.
El editor nunca escribe la configuración por su cuenta.

- `hello` se responde igual (mismos `limits`) con solo las operaciones de recuperación:
  Editor `hello`, `recovery`, `recovery_reset`, `transfer_begin`, `transfer_write`,
  `transfer_finish`, `transfer_drop`, `restore_commit`; Automation `hello`.
- `recovery {}` (solo Editor; Automation recibe `permission_denied`) devuelve
  `{cause, actions, detail:{code, counters_read}}`.

| `cause` | Cuándo | `detail.code` | `actions` |
|---|---|---|---|
| `unreadable` | La base no tiene ninguna revisión válida | `corrupt`, `invalid` | `restore`, `reset`, `details` |
| `newer_version` | La escribió una versión más nueva de RAD | `unsupported_version` | `details` |
| `unsafe_folder` | La carpeta de datos o `state.sqlite3` no es privado y propio (ni enlace) | `unsafe_directory`, `unsafe_database_file` | `details` |

  `counters_read` es `true` si se pudieron leer los contadores de identidad del archivo
  dañado (solo se intenta con `unreadable`).
- `recovery_reset {confirm}` (solo Editor; mutación) es «Empezar de nuevo». Sin
  `confirm: true` responde `confirmation_required`; con una causa distinta de
  `unreadable`, `operation_not_allowed`; si falla antes de reemplazar la base,
  `storage_error` y sigue en recuperación (ambos `not_applied`). Si tiene éxito responde
  `applied` con `{changed, revision}`: el archivo dañado queda como
  `state.sqlite3.damaged-<ms>` (con sus sidecars), la configuración nueva no reutiliza los
  IDs que se pudieron leer, y el runtime pasa a modo normal **en el mismo proceso**. Las
  conexiones abiertas siguen sirviendo: el editor relee sus vistas.
- Todo lo demás, en ambos endpoints, responde `recovery_required` (`not_applied` si es una
  mutación). En modo normal `recovery` y `recovery_reset` no se anuncian ni se atienden.
- `restore` es «Restaurar una copia» y solo existe con `unreadable`. Se sube el paquete con
  las operaciones de siempre (`transfer_begin {kind:"restore"}`, `transfer_write`,
  `transfer_finish` → `restore_prepared` con su resumen, `transfer_drop`) y se confirma con
  `restore_commit {transfer_id, confirm_replace:true}`. Una operación a la vez: la siguiente
  espera en la cola. Esas operaciones con una causa distinta de `unreadable`, o
  `transfer_begin` con un `kind` distinto de `restore`, responden `operation_not_allowed`
  (`not_applied`; no se crea nada en la carpeta de datos); `transfer_read` y `media_read` siguen en
  `recovery_required`. `restore_commit` sin `confirm_replace: true` responde
  `confirmation_required`; una copia que no se puede aceptar, `invalid_request`; si falla
  antes de reemplazar la base, `storage_error` y sigue en recuperación (todos
  `not_applied`). Si tiene éxito responde `applied` con `{changed, revision}` como
  `recovery_reset`: el archivo dañado queda como `state.sqlite3.damaged-<ms>`, la base nueva
  contiene la copia y el runtime pasa a modo normal en el mismo proceso. La base nueva
  siempre es de la instalación del archivo dañado (una aleatoria si no se pudo leer) y
  nunca adopta el token de la copia: una copia **de esta instalación** conserva sus IDs y
  los contadores del archivo dañado quedan reservados (lo que se cree después no choca con
  ninguno); una copia **de otra** instalación se remapea como en una restauración normal
  (ADR 0003), o se adopta tal cual si no se pudo leer ningún contador.

## Acción nativa Sleep

`add_action {button_id, type:"sleep", parameters:null}` añade un paso nativo sin
parámetros; objetos de parámetros de otros tipos se rechazan como
`invalid_request` (`not_applied`). `get` e `inspector` lo proyectan con
`type:"sleep"` y `parameters:null`. Persiste en documento v4 (ADR 0017), se copia
y entra en el historial como cualquier acción. No depende de un plugin.

`run {id}` desde CLI o «Probar», y la entrada física, pasan por la misma admisión
con permisos vigentes y snapshot. Authority completa el paso localmente como
satisfactorio al activar exactamente el reposo de inactividad; no crea un trabajo
externo ni cambia configuración o navegación. No garantiza recepción física si
el deck está ausente. Los siguientes pasos del plan siguen normalmente. Admite
`press` y `long_press`, no `hold`. Funciona con `sleep_ms:"0"`.

Soltar la tecla que activó Sleep no despierta. Una nueva presión de cualquier
tecla despierta y se consume hasta su liberación, sin ejecutar acciones; restaura
el brillo configurado y conserva la carpeta activa. Reposar termina los Hold
originales como el reposo automático.

`get_section` de apariencia añade el item tipado `native_icon` solo cuando existe
el visual nativo; conserva los tres items anteriores y su orden.
Las apariencias compuestas del editor pueden incluir `native_icon:"sleep"`, enum tipado de
presentación, cuando la tecla contiene esta acción y no tiene imagen ni visual
externo. El texto propio y su colocación se conservan. El campo se omite para las
apariencias anteriores, preservando sus vectores v1; el cliente nuevo admite su
ausencia. Un cliente anterior con DTO cerrado necesita actualizarse para mostrar
esa apariencia nueva. Los contratos de automatización de las acciones anteriores,
el framing y JSON CLI siguen en v1.

## CLI JSON y salida de proceso

La versión de JSON/exit codes es `rad_api::CLI_VERSION = 1`; el campo
`cli_version: 1` identifica esa versión, independientemente de `v` (API local). `--json` emite un objeto por respuesta y un salto de
línea. Las claves del envelope anterior se conservan; el detalle técnico e IDs
son intencionales para automatización.

| Salida | Significado |
|---|---|
| 0 | éxito/admitida/aplicada |
| 2 | argumentos, protocolo, configuración o cursor inválidos |
| 3 | permisos u operación no permitida |
| 4 | entidad o resultado inexistente/expirado |
| 5 | capacidad, ocupado o servicio/ejecutor no disponible |
| 6 | acción fallida o vencida |
| 7 | almacenamiento |
| 8 | resultado incierto, desconexión u otro fallo técnico |
| 9 | venció la espera del cliente, sin cancelar la ejecución |

`Unknown` tiene prioridad sobre cualquier otro mapeo. El contrato no garantiza
hardware real ni compatibilidad con todas las distribuciones por pasar tests
sintéticos.

## Migración en curso

La fase 2 no se considera terminada hasta que las respuestas históricas y todos
los consumidores utilicen la definición única de `rad-api`; los parsers legacy
restantes de `rad-protocol` no son una segunda especificación aceptada.
Los parsers estructurales de peticiones de `rad-protocol` se retiraron: su dispatch
recibe variantes `Request`. `rad-wire` conserva framing y límites de JSON, no
respuestas de aplicación. El cliente decodifica `ResponseData` según la operación
correlacionada; nunca infiere una clase de página por una lista vacía.
`PluginRetry` y `PluginLog` se anuncian solo en el endpoint del editor; sus
handlers no reinstalan ni ejecutan acciones. Véanse las precondiciones abajo.

## Ajustes y contrato publicado de plugins

Operaciones exclusivas del editor:

- `plugin_manifest {plugin_id}` devuelve `plugin_id` y `document`: el manifiesto
  completo del contrato de plugins separado, incluyendo capacidades, recursos,
  gestos y esquema de ajustes. `rad-api` no duplica ese manifiesto.
- `plugin_settings {plugin_id}` devuelve `plugin_id`, `encoding: "hex"` y `hex`
  (cadena o null). La lectura preserva bytes opacos incluso sin paquete instalado
  o con un esquema incompatible; no devuelve credenciales.
- `set_plugin_settings {plugin_id, parameters}` guarda parámetros tipados
  `{schema_version, values}` o retira el valor si `parameters` es null. La
  autoridad revalida el esquema publicado en su turno antes de persistir; el
  dominio conserva bytes opacos y el historial normal. No se sobrescriben datos
  incompatibles al abrir un formulario.

`FieldEdit::Gesture` selecciona `press`, `hold` o `long_press` para un botón.
Las vistas del botón llevan `gesture`; el catálogo de acciones lleva `gestures`.
`long_press_threshold_ms` se representa como cadena decimal en ajustes.

Una ejecución `hold` conserva el estado `running` mientras el control sigue
retenido o su `End` está pendiente. El TTL ordinario del resultado comienza tras
la liberación. La liberación envía `End` al mismo trabajo; su fallo se refleja en
el resultado original y cambia el aviso `execution`. No se crea ni se repite otra
ejecución para compensarlo. Los clientes releen ese resultado al invalidarse.

### Administración de plugins

`plugin_admin` es exclusiva del editor (otro origen recibe `permission_denied`) y es
una mutación: un rechazo lleva `outcome: not_applied`.

| Campo | Uso |
|---|---|
| `operation` | `install`, `update`, `reinstall`, `rollback` o `uninstall` |
| `plugin_id` | obligatorio salvo en `install` |
| `source` | enlace público de GitHub (repositorio o release); obligatorio en `install`, `update` y `reinstall`, y prohibido en el resto |
| `trusted` | debe ser `true` cuando hay `source`; si no, `confirmation_required` |
| `replaces_source` | solo en `update` y `reinstall`: el origen registrado que el usuario vio y confirmó |

La respuesta es `{operation_id, state: "preparing"}`: la petición se admite y el
trabajo sigue en segundo plano. El resultado se lee en `plugin_status`
(`state`, `operation_id`, `error` e `items`, con el `source` registrado de cada
plugin); el aviso `plugins` indica cuándo releerlo. Solo corre una operación a la vez.
Mientras descarga un paquete de tamaño conocido, `progress` dice el porcentaje
recibido (0–100); cada porcentaje nuevo produce un aviso `plugins`.

Rechazos de la petición, sin descargar nada: `invalid_request` (combinación de
campos no válida, incluido `replaces_source` fuera de `update` y `reinstall`),
`confirmation_required`, `resource_exhausted` (otra operación en curso),
`plugin_in_use`, `plugin_residual`, `publication_pending` y `source_changed`.
Los fallos de la operación ya admitida aparecen en `plugin_status.error`:
`invalid_source`, `invalid_package`, `incompatible_plugin`, `network`, `deadline`,
`io`, `cancelled`, `plugin_already_installed`, `plugin_mismatch`,
`plugin_same_version`, `plugin_failed` y `recovery_required`.

**Origen.** El origen de un plugin es el `owner/repo` de GitHub de su enlace, sin
distinguir mayúsculas; otro tag o release del mismo repositorio no es otro origen.
Si un `update` o `reinstall` pide otro `owner/repo` que el registrado, el runtime
lo rechaza con `source_changed` sin descargar, salvo que `replaces_source`
identifique ese mismo origen registrado. El rechazo lleva en `details` las dos
direcciones canónicas:

```json
{"code":"source_changed","details":{"registered":"https://github.com/owner/repo","requested":"https://github.com/otro/repo/releases/tag/v2"}}
```

El cliente muestra ambos orígenes y, si el usuario confirma, repite la petición con
`replaces_source` igual a `registered`. Si el origen registrado cambió entretanto,
`replaces_source` ya no coincide y vuelve `source_changed` con el origen vigente.
Tras aplicarse, el nuevo origen queda registrado. Un plugin sin origen registrado
no pide confirmación y registra el nuevo.

### Editor plugin settings, repair and recent log

`plugin_manifest {plugin_id}` returns the opaque manifest document; consumers use
`rad-plugin-contract::Manifest`, not a second schema. `plugin_settings {plugin_id}`
returns optional hex-encoded saved bytes even when the plugin is missing.
`set_plugin_settings {plugin_id, parameters}` is a normal configuration edit (history,
undo and backup included); `null` removes the setting. Unknown or incompatible
saved values remain untouched until an explicit valid edit. Settings fields with
unsupported dynamic options remain read-only; no action-option query is fabricated.

`plugin_retry {plugin_id}` is editor-only and explicit. It is available through the
`can_retry` status flag only for the same failed selection after reap and with no
outstanding administrative/residual work. It clears that selection's attempted-start
latch and removes its reaped Host tombstone. Normal demand may then launch it again;
no package is downloaded/reinstalled, no action is replayed, and configuration and
credentials stay intact. An unavailable retry is rejected, not replaced by a query.
`has_settings` indicates that a registered manifest exposes settings.

`plugin_log {plugin_id}` is an editor-only read returning `{plugin_id, lines}` from
the Host's already sanitized, bounded stderr capture. It neither starts the plugin
nor changes logging; the response stays under 48 KiB including JSON escaping and
envelope overhead. The editor exposes it under Settings → Advanced → Log.

`runtime_log {}` is an editor-only read of the runtime's own recent events (ADR 0011),
for Advanced → Log. It returns `{entries:[{seq, at_unix_ms, code, subject?}], dropped}`,
oldest first, with every integer a decimal string. It is not a mutation and consumes
nothing: asking again returns the same entries plus whatever happened since. Automation
gets `permission_denied`.

- The runtime keeps the last 128 events of this process; older ones are discarded and
  counted in `dropped`. `seq` grows by one per event and is never reused, so a gap
  between two reads means entries were dropped. A restart starts a new ring (what ended
  the previous run is `previous_exit`).
- `code` is a closed set of stable `snake_case` names, never prose; the editor words them.
  `subject`, when present, is an identifier of at most 128 bytes (a plugin id, an exit
  reason) and never data from a failure.
- Events are state changes and contained failures, not traffic: none per request or per
  key. Each one also leaves one line on stderr (journald), `rad-runtime: <code>[ <subject>]`.
- At most 128 entries fit well inside 48 KiB.

| Code | Subject | Meaning |
|---|---|---|
| `started` | | The runtime started serving. |
| `previous_exit` | reason | The last run ended unexpectedly (`startup`, `storage`, `transport`, `clock`, `protocol`, `panic`). |
| `shutdown_requested` | | A clean stop was asked for (SIGTERM or SIGINT). |
| `deck_connected`, `deck_disconnected` | | The deck appeared or went away. |
| `deck_inaccessible` | | The deck is present but cannot be used. |
| `deck_spawn_failed` | | The deck could not be started; the runtime serves without it. |
| `plugin_failed` | plugin id | A plugin ended without being asked to. |
| `plugin_io_unregistrable` | plugin id | A plugin's pipes could not be watched; it was stopped. |
| `launcher_lost` | | The launcher helper ended or could not be watched. |
| `storage_uncertain`, `storage_recovery_required`, `storage_ready` | | The storage state changed. |
| `display_clock_overflow` | | The deck's idle clock overflowed; it simply does not sleep by itself. |
| `response_refused`, `response_unknown_request`, `response_dropped` | | An answer was too large or empty, its request was gone, or even the generic error could not be sent. |
| `response_unbuildable` | | The answer to a resolved request could not be built; the client got a generic error, never "not applied" for a change. |
| `plugin_response_invalid`, `catalog_response_invalid` | | A plugin or the launcher answered something that does not decode; that request got an error. |
| `notice_dropped` | | A notice could not be queued; the next one recovers it. |
| `connection_failed` | count | Connections that could not be taken in (closed, or left queued while the process is out of descriptors), reported at most once a minute with how many since the previous report. |
| `machine_settings_unreadable` | | `machine.json` could not be read; defaults are in use and the file is left untouched. |
| `export_failed` | | An export's snapshot could not be made; that export got an error and its lease went back. |
| `recovery_mode` | cause | The runtime could not open the configuration and entered recovery mode (`unreadable`, `newer_version`, `unsafe_folder`). |
| `recovery_reset` | | «Empezar de nuevo» finished; the runtime went on in normal mode. |
| `recovery_restored` | | «Restaurar una copia» finished; the runtime went on in normal mode with the copy. |

`machine_settings {}` is an editor-only read of the machine's own options (ADR 0005,
0006): `{use_stream_deck, start_with_session}`, both booleans. They never travel in
copies of the configuration. `set_machine_settings {use_stream_deck?, start_with_session?}`
changes them and answers `ok` (not `applied`: it is no configuration revision) with the
resulting view; no field at all is `invalid_request`, and a value equal to the current
one writes nothing. The change is stored first: if that fails the answer is
`storage_error` with `outcome: not_applied` and nothing changes. `use_stream_deck` then
takes or lets go of the deck without restarting the runtime; `start_with_session` is only
stored, the editor applies it. Automation gets `permission_denied` on both.

Board and Overview notices invalidate independent views. The editor fetches only the
invalidated view, while Board and Inspector still require matching revisions before
being presented together.

Plugin details load the existing `PluginManifest` only on explicit demand. The
optional resource declarations are rendered from the canonical enum, at most five,
with an informational/not-enforced notice; they do not grant permissions. The
editor caches this metadata by plugin and selected version, invalidating it on
plugin notices and reconnect, without polling.
