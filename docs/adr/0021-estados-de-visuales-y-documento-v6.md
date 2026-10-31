# 0021 · Estados de visuales, vínculo con acciones y documento v6

**Estado:** aceptada, 2026-10-31.

Una acción con estado debe mostrar la fuente observada sin configurar otro visual.
El estado no cuenta pulsaciones: el plugin conserva la autoridad sobre la fuente;
el runtime conserva la autoridad sobre configuración y proyección.

La capacidad aditiva `visual_states` declara exactamente dos estados ordenados por
visual, con identificador único, etiqueta traducible e icono PNG opcional, cuadrado
hasta 256 px y 8 KiB. El manifest completo sigue limitado a 64 KiB. Una acción
puede vincular un visual del mismo plugin con campos compatibles; el editor copia
solo esos campos y publica acción y visual juntos. Un visual externo ajeno no se
reemplaza. Las acciones existentes adquieren el vínculo al guardarlas, sin migración
automática. Los visuales vinculados no se ofrecen por separado en la paleta.

El contenido `state` se transmite solo si ambos pares negociaron la capacidad.
La imagen propia del estado prima sobre el icono del manifest, y este sobre la
imagen del plugin. Sin texto propio no aparece texto, incluidas las escenas. El
segundo estado representa el reposo cuando la fuente no está disponible, con la
marca roja; la vigencia vencida mantiene la marca ámbar. La decodificación de los
iconos pertenece a la selección del plugin, no al frame ni a cada reinicio.

`Appearance.states` contiene hasta dos `StateFace`, cada uno con texto e imagen;
la posición del texto sigue siendo compartida. Documento **v6**, importador cerrado
de v5 y golden histórico: Store conserva `.document-v5-backup` antes de convertir
revisiones. Los lectores previos requieren actualizar; no interpretan v6 como
corrupción ni aceptan estados bajo una etiqueta anterior. RADPKG y Store conservan
envelope v2. Las imágenes de ambas caras son raíces de medios, incluso cuando no
se muestran: viajan en copias y sobreviven al historial y la restauración.

La API local conserva framing v1. `Edit` incorpora altas y bajas de acciones para
publicar el vínculo en una sola transacción y deshacerlo de una vez. `MediaRead`
admite una referencia explícita solo si pertenece a la apariencia consultable del
botón, incluidas sus caras; se vuelve a comprobar antes de entregar cada respuesta.
Los lectores cerrados anteriores necesitan actualizarse para estas variantes.
No hay una autoridad de configuración paralela en el editor.

Dos imágenes preparadas pueden compartir una publicación: la segunda importación
se admite solo después de preparar la primera, dentro de la cuota global de dos.
El lease agrupado se retiene antes de encolar y se resuelve entero ante éxito,
fallo o incertidumbre. No amplía concurrencia para restauraciones ni subidas activas.

La comparación de vínculos usa tipo y todos los parámetros derivados, no solo el
nombre del visual. Una acción nueva no reemplaza un vínculo existente; editar o
retirar una acción no altera el vínculo si otra acción lo comparte. El worker
compone lecturas acotadas del Inspector existente, sin agrandar su respuesta ni
persistir otro dueño. Todas comparten el `view_revision` del origen/sesión, y
`Edit.expected_revision` opcional comprueba ese token al turno de Authority,
antes de preparar candidatos, IDs o persistencia. No es el número durable de
SQLite: un cambio visible también puede rechazar con `cursor_expired` sin efecto.
Clientes sin precondición conservan su comportamiento; no hay CAS para otras
operaciones ni replay automático. El formulario queda disponible para Aplicar
otra vez y releer.
