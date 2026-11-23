# 0010 · API de clientes tipada con avisos

**Estado:** aceptada, 2026-10-20.

Hoy las peticiones viajan como una cadena de operación más JSON y se interpretan a
mano en tres sitios. El editor consulta el estado cada 250 ms y, si cambió la
revisión, recarga una carpeta con hasta 66 peticiones; además repinta cada 50 ms y
asume 32 teclas. Se decidió:

- **`rad-api`** como única definición de peticiones, respuestas, vistas, avisos y
  códigos de error, con enums en vez de cadenas; la usan `rad-protocol`, `rad-client`
  y el editor.
- **Vistas compuestas y coherentes:** el tablero (carpeta, teclas y estado visual) y
  el inspector llegan en una respuesta cada uno, con su revisión; las imágenes,
  aparte. La rejilla la describe el runtime.
- **Vista de estado general** para la barra del editor: deck (conectado,
  desconectado, inaccesible), plugins que necesitan algo, almacenamiento (incluido el
  modo recuperación) y último reinicio, cada estado con su código.
- **Avisos:** el runtime envía al cliente suscrito un aviso coalescido de qué cambió,
  y el cliente vuelve a pedir solo eso. Se conserva el último aviso de cada tipo, sin
  cola ilimitada. El worker despierta la GUI cuando llega un resultado o un aviso, sin
  sondeo ni repintado periódico.
- **Códigos, no prosa:** el runtime entrega códigos y el editor los traduce por
  catálogo ([ux.md](../ux.md)).
- **Resultados inciertos:** ante Unknown, el editor vuelve a leer el estado; la
  acción se repite solo si el usuario la pide.
- **`rad-cli` estable:** su salida JSON y sus códigos de salida son un contrato con
  versión. La CLI sigue mostrando IDs y detalle técnico.

**Consecuencias:** las pantallas nuevas que necesiten datos nuevos se construyen
sobre `rad-api`. La especificación vivirá en `docs/client-api.md`.

Las operaciones salen de una sola declaración con su nombre y clase de mutación. Las
respuestas v1 se decodifican según la petición y el estado del envelope porque dos listas de
tipos distintos pueden estar vacías. El control de forma contrasta el JSON con su
representación tipada: rechaza arrays posicionales para estructuras, acepta la omisión de
campos opcionales y usa variantes de estructura vacías para que Serde también rechace los
campos extra de una alternativa sin datos.

El transporte reanuda por plazo la aceptación después de agotar recursos del proceso: una
conexión que ya está en la cola puede no producir otra señal de disponibilidad. Un error al
registrar un solo par se contiene en ese par. El transporte reserva un token para los avisos
de workers y retiene una señal consumida durante `drive` para que la espera posterior no
pierda el despertar. Una operación rechazada al leer su envelope sólo recibe un resultado
correlacionado si el parser acotado puede identificarla de forma inequívoca.

Las opciones dinámicas pueden revelar el estado de aplicaciones externas. Consultarlas por
tipo requiere Consulta global. La consulta por entidad aplica los permisos de esa entidad y
combina las dependencias guardadas que aún acepta el esquema con los valores válidos del
borrador. La consulta captura la identidad de la selección instalada para detectar un
reemplazo antes del despacho. El runtime resuelve además la generación del proceso que
atenderá la petición. Un reinicio con el mismo paquete y un cambio de paquete invalidan
identificadores distintos.
