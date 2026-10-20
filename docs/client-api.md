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
  `last_restart`, carpeta activa, ajustes y permisos globales e historial.

Cada proyección se construye sin ceder el actor ni hacer I/O entre componentes.
El cliente no debe mezclar silenciosamente board e inspector de revisiones
incompatibles: invalida/relee. Las imágenes se leen aparte (`media_read` o
`plugin_visual_read`); las vistas solo llevan referencias y metadatos.
Una vista que supera el presupuesto de respuesta de 48 KiB produce
`resource_exhausted`, nunca una vista truncada o parcialmente coherente.

`deck` distingue `disabled`, `connected`, `disconnected` e `inaccessible`.
`storage` distingue `ready`, `uncertain`, `recovery_required`. Estos códigos no
prometen que el modo recuperación de arranque (fase 3) ya esté implementado.
`last_restart` es `clean` o `recovered_previous` según la reconciliación de inicio.

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

Board and Overview notices invalidate independent views. The editor fetches only the
invalidated view, while Board and Inspector still require matching revisions before
being presented together.

Plugin details load the existing `PluginManifest` only on explicit demand. The
optional resource declarations are rendered from the canonical enum, at most five,
with an informational/not-enforced notice; they do not grant permissions. The
editor caches this metadata by plugin and selected version, invalidating it on
plugin notices and reconnect, without polling.
