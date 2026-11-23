# 0025 · Observación y recursos de los plugins oficiales

**Estado** — Aceptada. Registra decisiones vigentes al 23 de noviembre de 2026.

Una tecla puede desaparecer y volver mientras una llamada externa sigue en curso.
Los plugins comparten recursos por fuente y distinguen cada período de demanda.
Así pueden reutilizar el trabajo costoso sin presentar un resultado de la demanda
anterior como una observación actual. El SDK renueva la vigencia únicamente al
recibir una observación, según el [ADR 0008](0008-frescura-declarada.md).

## Audio

La salida predeterminada se resuelve en el servidor al ejecutar la orden. Para
el micrófono se resuelve primero el nombre y se descartan las fuentes que
monitorizan una salida. El alias de fuente predeterminada de PulseAudio podría
silenciar la salida que el usuario escucha. Se reconocen los distintos campos
de monitor de `pactl` y el sufijo de nombre habitual. Al leer el JSON se exige un
objeto por dispositivo porque Serde también puede rellenar una estructura desde
un array posicional. [Selección y lectura](../../plugins/audio/src/devices.rs).

Un monitor comparte un único `pactl subscribe` entre los botones. Se suscribe
antes de tomar la primera muestra para cubrir cambios durante el arranque.
Agrupa ráfagas con un máximo de espera e ignora los eventos de clientes, que
producen las propias lecturas. Tras un fallo del servidor o un pánico del
observador, cierra y recoge el hijo y espera antes de reintentar. La espera
crece hasta un límite y solo vuelve al mínimo después de publicar con éxito.
La decisión de retirar el hilo y su estado visible al siguiente suscriptor se
actualizan bajo el mismo bloqueo. [Monitor](../../plugins/audio/src/monitor.rs).

La etiqueta de dispositivo normaliza espacios y caracteres de control. Se acorta
por un límite de bytes respetando UTF-8. Si la descripción queda vacía se usa
el nombre del dispositivo que admite el host.

## Multimedia

Una consulta o acción puede compartir una conexión con otras operaciones, cada
una con su plazo. El monitor de señales necesita una conexión asociada a su propio
contexto GLib para recibir la notificación de cierre. Perder el bus debe dejar al
plugin en condiciones de reconectar. Las consultas D-Bus desactivan el arranque de
servicios para que un botón dirigido a un reproductor cerrado no lo abra. Una
respuesta `NoReply` del bus al desaparecer el destinatario se trata como
`unavailable`. El vencimiento del plazo local se trata como `timed_out`.
[Conexión](../../plugins/media/src/bus.rs) y [monitor](../../plugins/media/src/monitor.rs).

Los botones de una aplicación comparten lector. Mantenerlo brevemente después
de la última baja evita crear un hilo por cada cambio de demanda. Cuando todos
los lectores están ocupados, la aplicación espera el siguiente que quede libre.
La suscripción nueva solicita una observación aunque sobreviva el lector de la
anterior. [Suscripciones](../../plugins/media/src/watch.rs).

Cada lector abre las carátulas en modo no bloqueante, exige un archivo regular
y limita también los bytes leídos después de consultar su tamaño. Una tubería o
un archivo que crezca durante la lectura podrían retener el lector o superar su
presupuesto. La decodificación se concentra en un hilo para evitar que cada
aplicación conserve su propia arena de memoria. El canal sin cola limita los datos
pendientes y deja el acceso al archivo en el lector de cada aplicación. El
contenido determina el formato, aun cuando el archivo carezca de extensión.
[Conversor](../../plugins/media/src/cover.rs).

Multimedia fija las versiones de GIO y GLib que usa la aplicación. Sus conexiones
D-Bus funcionan con las características predeterminadas de Cargo desactivadas.

## Métricas

La apertura, las lecturas y la descarga de NVML pertenecen al mismo hilo. Un
error que devuelve el driver permite reabrir en el siguiente ciclo. Si el hilo
supera el plazo, se considera terminal antes de examinar su respuesta y nunca
se lanza otro para reemplazarlo. Esto acota el número de hilos que pueden quedar
dentro del driver. Se consulta únicamente el dispositivo de índice cero. Los
cambios de demanda registran la última baja de CPU o NVML aunque un alta
inmediata oculte la transición al lector. Así se descarta la muestra previa de
CPU y se liberan los recursos de NVML cuando corresponde. La demanda vacía
también llega al hilo. [Muestreo](../../plugins/metrics/src/polling.rs) y
[lector](../../plugins/metrics/src/reader.rs).

La barra calcula la distancia al segmento central con la raíz cuadrada de la
suma de cuadrados. Para esas coordenadas acotadas conserva el resultado de
`hypot` sin enlazar `libm` solo para dibujar la barra.
[Renderizado](../../plugins/metrics/src/bar.rs).

## OBS y SDK

Un evento de OBS indica qué estado releer. Una identidad incompleta invalida
los candidatos de ese tipo. Cuando cambia la colección se descartan las
correlaciones antiguas para que una respuesta tardía no confirme un objetivo
nuevo. Las órdenes se serializan por sesión y se confirman mediante una
lectura posterior. Si la biblioteca WebSocket ya encoló el frame y su envío
devuelve `WouldBlock`, el transporte drena lo pendiente sin volver a enviar la
orden. El guardado de una repetición admite que todavía no exista un archivo
anterior y exige observar uno nuevo. [ADR 0020](0020-textos-de-plugins-y-plazo-fijo.md),
[transporte](../../plugins/obs/src/transport.rs) y [acciones](../../plugins/obs/src/workflow.rs).

El SDK reserva el crédito para el final de un gesto al admitir su inicio, antes
de invocar al plugin. Una lista de opciones válida puede sobrepasar el límite de
frame al escapar los caracteres JSON. En ese caso el SDK responde
`unavailable` para que el host pueda terminar la consulta. La salida parcial es
terminal para todos sus productores. El empaquetador comprueba los tamaños
antes de cargar los archivos y publica desde un temporal propio en el mismo
directorio para conservar el destino si falla la escritura.
[Gestos](../../plugins/sdk/src/holds.rs), [respuestas](../../plugins/sdk/src/reply.rs)
y [empaquetador](../../plugins/sdk/src/bin/radp-pack.rs).

El SDK contiene el pánico de un handler de acción. Al destruir `Reply` resuelve
ese pedido y el plugin puede seguir atendiendo. Si falla `credentials_released`,
el SDK termina el protocolo, porque ya no puede confirmar la liberación de las
credenciales.

## Verificación

Los fixtures permiten provocar fallos del backend de forma aislada. El bus
privado usa servicios sintéticos para demostrar que el plugin no activa
reproductores. Los helpers de audio distinguen un proceso vivo de uno zombi y
la desaparición completa del PID cuando la prueba exige que se lo haya
recogido. Las medidas de hilos se toman después de que el plugin responda, en un
proceso de prueba independiente cuando se mide su total. Una espera establece el
orden de dos eventos cuando el oráculo necesita observar cuál ocurrió primero.
En los tests del SDK el host construye sus propios frames y el peer de OBS usa
un vector de autenticación independiente del plugin. El relay de pruebas espera
la autenticación antes de abrir su conexión protegida, limita las esperas para
detectar la cancelación y reserva el canal de control para resultados acotados.
[Pruebas](../testing.md), [controlador OBS](../../plugins/obs/examples/readonly-relay.rs)
y [host de prueba](../../plugins/sdk/tests/support/mod.rs).
