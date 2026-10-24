# Plugin de OBS para RAD

Referencia de la integración oficial para desarrolladores y agentes. Plugin
independiente `org.rad.obs` (`rad-plugin-obs`), versión 0.2.0, contrato de plugins v1
con capacidades negociadas de credenciales, ajustes, opciones y plazo de respuesta.
Consultar el [contrato y la guía para autores](../README.md).

Se conecta únicamente a **127.0.0.1** mediante obs-websocket 5.x, RPC 1 y WebSocket
JSON/texto. El ajuste opcional del plugin `port` es un entero de 1 a 65535
(predeterminado **4455**); debe coincidir con el puerto de OBS. Viaja en las copias
del deck. Cada operación captura su puerto: cambiarlo no traslada trabajos ni
consultas activos a otra conexión. Los visuales se vuelven a suscribir con los
ajustes nuevos. No admite hosts remotos, TLS, control de Studio/preview, creación
de objetos ni varias instancias de OBS.

El slot opcional de credenciales `password` contiene la contraseña del WebSocket
de OBS. Una conexión que exige autenticación y no tiene contraseña queda no
disponible; un servidor sin desafío de autenticación no recibe la contraseña.
Las credenciales se aíslan por revisión: guardar otra contraseña no vuelve a
autenticar trabajos antiguos.

## Controles y visuales

| Control | Operaciones | Texto visual |
|---|---|---|
| Escena de programa | Seleccionar escena existente | Program / Inactive (escena seleccionada) |
| Filtro | On / Off / Toggle | Enabled / Disabled |
| Elemento de escena | Show / Hide | Visible / Hidden |
| Grabación | Start / Stop / Pause / Resume | Recording / Paused / Stopped |
| Emisión | Start / Stop | Streaming / Stopped |
| Esperar conexión | `timeout_seconds` opcional: entero 1–120, predeterminado 30 | Ninguno |

Las demás acciones tienen un timeout del host de cinco segundos. Los visuales
observan también los cambios hechos fuera de RAD. Una selección desconectada,
incompatible, ausente o desactualizada se declara no disponible en vez de
adivinarla. La respuesta de una acción se confirma leyendo el estado real de OBS;
aceptar una orden no basta para indicar éxito. Una acción ordinaria puede hacer
un intento inicial de conexión (hasta dos segundos dentro de su timeout), pero
falla durante la espera entre reintentos y nunca espera un intento posterior.
Las órdenes nunca se repiten tras reconectar. La reconexión refresca observaciones.

Las opciones dinámicas enumeran fuentes y filtros, escenas y elementos de escena
existentes, incluidos los hijos de grupos. La identidad del elemento incluye su
contenedor y su ID numérico, no solo el nombre de la fuente. Las selecciones
guardadas incluyen también colección y nombres: renombrar o reemplazar objetos
puede exigir seleccionarlos otra vez. No se redirigen selecciones entre
colecciones. Los frames/mensajes de OBS superiores a 64 KiB se rechazan sin
truncarlos (límite defensivo de compatibilidad independiente del framing del SDK).
Las listas de más de 64 opciones o las identidades/etiquetas de más de 256 bytes
UTF-8 quedan no disponibles, nunca truncadas.

Las peticiones se detienen mientras OBS cambia de colección de escenas. La
compatibilidad se comprueba contra las peticiones que OBS anuncia realmente;
los controles no soportados se rechazan sin intentar ejecutarlos. Esto no afirma
compatibilidad con todas las compilaciones ni distribuciones de OBS.

## Validación aislada

`cargo test --offline --locked --release -p rad-plugin-obs`, desde el workspace de
plugins, ejecuta contratos críticos con peers falsos. Los tests de red solo usan
endpoints loopback efímeros inyectados, nunca el puerto de producción 4455.
Los ejemplos `obs-fixture` y `obs-conformance-server`, habilitados con
`test-support`, son helpers para pruebas aisladas, no configuración pública del
endpoint del plugin.
`obs-fixture` admite un puerto de prueba como argumento; sin argumentos, lee un
archivo privado regular `0600` en `$XDG_RUNTIME_DIR/rad-obs-fixture.port`. Rechaza
el puerto 4455 y nunca recurre a producción como alternativa.
Compilar primero los helpers según [Pruebas y validación](../../docs/testing.md).

La [evidencia real disponible](../../docs/performance.md#evidencia-y-límites)
delimita las consultas OBS medidas mediante relay de solo lectura; no certifica
acciones, autenticación directa ni todas las versiones, compilaciones o
distribuciones de OBS.
`--licenses` imprime las licencias incorporadas.
