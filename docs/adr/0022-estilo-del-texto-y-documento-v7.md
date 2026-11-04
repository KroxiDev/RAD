# 0022 · Estilo del texto por cara y documento v7

**Estado:** aceptada, 2026-11-04.

Cada cara de una tecla necesita controlar todo el texto mostrado, incluido el
nombre y el enviado por un plugin. La posición compartida de
[ADR 0021](0021-estados-de-visuales-y-documento-v6.md) no permite ocultarlo en un
solo estado ni ajustar su tamaño, color o barra de fondo.

La apariencia base y cada cara de estado tienen su propio estilo: posición
opcional, tamaño de 8 a 24 px (13 por defecto), color RGB (blanco) y barra detrás
del texto (activada). `hidden` es una posición explícita: oculta todo el texto de
esa cara sin borrar lo escrito. Una cara nueva empieza con el estilo
predeterminado; no hereda el estilo base ni el de otra cara.

La posición automática considera cualquier imagen mostrada, incluidas imágenes
de plugins, iconos de estado y los iconos nativos de Suspender y Brillo. Con
imagen y sin texto propio ni texto del plugin, oculta el texto; con imagen y texto,
lo coloca abajo; sin imagen, lo centra y conserva el nombre como respaldo de la
cara base. Una cara de estado solo usa su texto propio. Elegir una posición en el
editor la fija: el menú no ofrece volver al automático. Otros clientes pueden
restablecerlo mediante `null`.

`rad-core` resuelve el texto y su estilo para el runtime. `rad-api` ofrece la misma
regla pura para el borrador del editor; ambas se comprueban con una tabla de
vectores común, sin añadir dependencias entre capas. Las vistas distinguen la
posición efectiva de su origen automático. El deck y el editor aplican tamaño,
color y barra al texto resuelto; la vista previa escala exactamente respecto de
96 px y la rejilla mantiene un mínimo de 9 px para su legibilidad.
La vista dinámica conserva aparte el texto fuente del plugin: ocultarlo en el
texto resuelto no impide previsualizar una posición visible antes de guardar.

El documento pasa a **v7**. Posición, tamaño, color y barra son opcionales en cada
cara y el escritor omite los valores predeterminados. El importador cerrado v6
rechaza los campos nuevos y `hidden`; conserva la posición base elegida en todas
las caras de estado existentes. Una posición automática sigue automática, y
tamaño, color y barra toman sus valores predeterminados. Store conserva
`.document-v6-backup` antes de convertir las revisiones; las copias portables usan
el mismo importador. Los envelopes SQLite y RADPKG siguen en v2.

Hay dos diferencias deliberadas respecto de v6: una imagen de plugin sin texto
deja de mostrar el nombre, y una cara de estado automática con texto pero sin
ninguna imagen pasa de abajo al centro. Los estados de Audio y OBS tienen iconos
propios, por lo que el segundo cambio no los afecta. Una configuración v7 no se
puede abrir con un RAD anterior; no se implementa downgrade.

En la API, un campo de estilo base omitido conserva lo guardado y `null` vuelve
al predeterminado. El mapa `states` explícito sigue reemplazándose completo;
dentro de cada cara, omitir un campo de estilo usa su predeterminado. El editor
envía ambas caras con la revisión esperada y conserva los cambios locales por
campo cuando debe releer tras un rechazo conocido.

Se descartó cambiar solo el automático porque no permite ocultar texto del plugin
o el nombre de forma explícita; un estilo compartido no permite editar cada cara
completa, y un estilo global no representa el ajuste individual de cada tecla.
