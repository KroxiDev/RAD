# Experiencia de usuario

Norma de cómo se comporta y cómo habla el editor. El estilo visual (simple, oscuro
y profesional, cercano a Elgato sin clonar su identidad) está en
[AGENTS.md](../AGENTS.md); la arquitectura que lo sostiene, en
[architecture.md](architecture.md). Es una norma de diseño para quienes mantienen
la interfaz, no un manual de usuario ni una afirmación de que todo esté terminado.
Los pendientes y su aplazamiento están en el [roadmap](roadmap.md#ux-pendiente).

## Principio

El usuario ve el funcionamiento, no el mecanismo. RAD se usa sin conocimientos
técnicos; los detalles técnicos existen y están a mano para quien los busque, fuera
del uso normal. Vale para todo el programa. La CLI es la vía técnica y conserva el
detalle.

- **S1 · Funcionamiento, no mecanismo.** En el uso normal no aparecen IDs,
  revisiones, generaciones, códigos, rutas, bytes ni nombres de procesos.
- **S2 · Lo técnico, en un solo lugar.** «Avanzado», en Ajustes y oculto por
  defecto, reúne los IDs con botón de copiar (para scripts), los permisos de
  automatización, el registro reciente y la información para soporte. La versión
  actual del producto se muestra directamente en Actualizaciones, sin desplegable.
  Cada error tiene «Ver detalles».
- **S3 · Mensajes llanos con una acción.** Cada error dice qué pasó y ofrece qué
  hacer; siempre hay una salida.
- **S4 · El sistema decide lo que puede decidir solo.** Conserva los IDs al restaurar
  cuando es seguro, resuelve un resultado incierto volviendo a leer el estado, usa el
  idioma del sistema y aplica valores por defecto sensatos. Al usuario solo se le
  pregunta lo que cambia su resultado.
- **S5 · Cada control aparece solo cuando se puede usar.**

## Textos e idiomas

- Cada texto visible se pide por clave a un catálogo por idioma, embebido; hoy,
  inglés y español, con el inglés como respaldo. Añadir un idioma es añadir un
  catálogo.
- Los textos de un plugin (acciones, visuales, campos y opciones) vienen de su
  manifest con sus traducciones; sin traducción se muestra el texto del plugin
  ([ADR 0020](adr/0020-textos-de-plugins-y-plazo-fijo.md)). El editor nunca
  traduce a un plugin con claves propias.
- El idioma por defecto es el del sistema; se cambia en las preferencias del editor.
- La API entrega códigos y el editor los traduce por catálogo. Un código desconocido
  se muestra como «Algo salió mal» con «Ver detalles».

## Vocabulario

Referencia en español; el catálogo inglés usa sus equivalentes.

- **Copia:** usar este término del [glosario](../CONTEXT.md) para exportar y
  restaurar; el contenido portable y sus exclusiones están en [formatos](formats.md#copias-radpkg).
- **Tecla:** en la interfaz nombra tanto la tecla física como el botón que la
  ocupa; el usuario no necesita distinguirlos. El código y la documentación
  técnica usan los términos del glosario.
- **Contraseña:** nombre visible de una credencial.
- **Acciones:** Reintentar · Instalar · Actualizar · Escribir la contraseña ·
  Restaurar una copia · Empezar de nuevo · Deshacer · Ver detalles.
- **Estados de un plugin,** solo cuatro: listo · necesita la contraseña · necesita
  actualización · no funciona, con [Reintentar] [Ver detalles]. Selección,
  generación, residuales e incertidumbre van a «Ver detalles».

| Situación | Mensaje | Acciones |
|---|---|---|
| Antes de restaurar | «Se reemplazará tu configuración actual por la de esta copia» | Confirmar |
| Restauración hecha | «Configuración restaurada» | Deshacer |
| La copia usa plugins que faltan | Nombra los plugins | Instalar |
| Falta una contraseña | «Necesita la contraseña» | Escribir la contraseña |
| Límite de elementos | «No se pueden crear más botones: se alcanzó el máximo» | — |
| Configuración ilegible | «No se pudo abrir tu configuración» | Restaurar una copia · Empezar de nuevo · Ver detalles |
| Configuración de una versión más nueva | «Actualiza RAD para abrir esta configuración» | Ver detalles |
| Carpeta de datos insegura | «RAD no puede usar su carpeta de datos» | Ver detalles |
| El runtime no arranca | «RAD no pudo arrancar» | Reintentar · Ver detalles |
| Tope de reinicios alcanzado | «RAD se detuvo por un error» | Reintentar · Ver detalles |
| Instalar un plugin | Qué se instala y de dónde, con «Instala solo plugins de fuentes en las que confíes» | Instalar · Ver detalles |
| Actualización desde otro origen | Ambos orígenes, en lenguaje llano | Confirmar |

## Comportamientos

- **Resolver antes de preguntar.** Ante un resultado desconocido, el editor vuelve a
  leer el estado y muestra lo que ocurrió; solo si no puede saberlo lo dice en
  lenguaje llano. La acción se repite solo si el usuario la pide.
- **Copias.** En Ajustes → Copias, «Export Backup» («Exportar copia» en español)
  guarda con un clic la configuración publicada en `~/RAD Backups`, fuera de los
  datos de RAD, con un nombre nuevo sin sobrescribir otras copias. No exporta al
  abrir la sección y pide guardar borradores antes de iniciar. La exportación usa
  el worker cliente, muestra la ubicación bajo «Ver detalles» al terminar y la deja
  seleccionada para la restauración manual. No incluye contraseñas ni ejecutables
  de plugins; no equivale a un respaldo completo de la instalación.
- **Restaurar** es una sola acción, sin preguntas sobre IDs, y se puede deshacer.
- **Plugins.** Los oficiales se instalan desde una lista, con un botón; uno de
  terceros, pegando su enlace de GitHub. Actualizar usa el origen registrado. Mientras
  un plugin funcione, no se muestran mensajes.
- **Paleta de acciones.** Las nativas se reúnen en «RAD»: Carpeta, Esperar,
  Suspender, Brillo y Abrir aplicación. «Aplicaciones» conserva los accesos directos y
  empieza colapsado. Cada plugin tiene su propio grupo con sus acciones y visuales;
  arrastrar un visual configura la apariencia, no agrega un paso al plan. La
  búsqueda general dice «Buscar acciones» y filtra la paleta. Abrir aplicación
  ofrece un desplegable con buscador en el inspector de la tecla; solo se guarda
  la acción al elegir la aplicación, sin elegir ninguna por defecto. En una
  posición vacía, la tecla se crea recién al elegir, con el nombre de la aplicación y
  su ícono cuando está disponible; cancelar no deja una tecla vacía. Los cambios de aplicación de un
  lanzador existente se guardan con «Guardar», también desde «Avanzado».
- **Suspender.** Acción nativa con media luna en el grupo RAD. Activa
  inmediatamente el mismo reposo configurado por inactividad, incluso
  cuando el reposo automático está desactivado. Cualquier tecla despierta sin
  ejecutar su acción; conserva carpeta y brillo. La luna predeterminada no
  reemplaza una imagen propia ni un visual de plugin.
- **Brillo.** El ajuste admite de 10% a 100%. La acción nativa «Brightness»
  («Brillo» en español), en el grupo RAD, solo permite elegir subir o bajar:
  cada ejecución cambia el brillo vigente en diez puntos porcentuales, con
  saturación en 10% y 100%. El valor se guarda y se refleja en Ajustes; no hay
  cantidad ni paso configurables, y alcanzar un límite no genera otra escritura.
- **Opciones de los plugins.** Cada opción se muestra con su nombre, nunca con
  su valor interno. Una opción obligatoria empieza en la primera, que el plugin
  pone como recomendada; en OBS y Audio es «Alternar».
- **Gestos.** El editor ofrece pulsar, mantener o pulsación larga solo si la acción
  los admite.
- **Pantallas.** Ajustes y los formularios ocupan la ventana o el panel de la tecla,
  sin ventanas flotantes; «Volver» y Esc regresan. Con cambios sin aplicar, la
  pantalla se queda y ofrece «Descartar».
- **Cambios sin recarga visible.** Lo que cambia se reemplaza de una vez, sin
  «Cargando…» ni huecos intermedios: el panel de la tecla, sus imágenes y las caras
  de plugins siguen visibles hasta que llega su versión nueva. Al cambiar de
  carpeta, la nueva se muestra al instante. El editor y el runtime conservan las
  imágenes de las carpetas visitadas y precargan las de la carpeta superior y las
  subcarpetas, con un tope de 8 MB cada uno fuera de la carpeta visible. Una imagen
  que aún no llegó aparece en el editor con un fundido de 0,15 s, junto con las
  demás; en el Stream Deck, todas a la vez y nunca marcadas como no disponibles
  mientras cargan. Al seleccionar una tecla, su panel aparece con un fundido corto
  y dice «Cargando…» solo si tarda más de 0,4 s.
- **Borrar.** La papelera del panel o Supr borran la tecla seleccionada, o la
  carpeta a la que entra, sin confirmación; Ctrl+Z la recupera.
- **Progreso.** Las transferencias muestran una barra.
- **Puesta en marcha.** El editor arranca RAD y ofrece «Iniciar RAD con el equipo»;
  el usuario nunca usa la terminal ([ADR 0012](adr/0012-servicio-gestionado-por-el-editor.md)).

## Actualizaciones de RAD

Ajustes → Actualizaciones funciona también sin conexión con el runtime o durante
recuperación. Entrar o abrir RAD no consulta Internet. Solo «Verificar
actualizaciones» inicia una consulta; salir invalida su respuesta y su candidato.
No hay notificaciones, temporizadores ni indicadores fuera de esta sección.
La versión actual de RAD permanece visible directamente, sin «Avanzado» ni otra
flecha para descubrirla.

Antes de consultar no se afirma que RAD esté actualizado. «Verificando…» es
transitorio; cada consulta termina únicamente en uno de estos tres resultados:

| Resultado | Texto | Acción |
|---|---|---|
| Error | «No se pudo verificar si hay actualizaciones.» | Verificar actualizaciones · Ver detalles |
| Sin versión superior | «No hay actualizaciones disponibles.» | Verificar actualizaciones |
| Versión superior elegible | «Hay una actualización disponible.» | Actualizar RAD, solo si la instalación está gestionada por el paquete oficial |

«Actualizar RAD» abre una confirmación dentro de Ajustes. Explica que RAD se
cerrará brevemente y dejará de controlar el Stream Deck mientras se instala. No
hay una descarga separada. Los borradores se conservan hasta que el usuario los
guarde o descarte; las operaciones pendientes deben terminar antes del relevo.
La descarga muestra progreso y permite cancelar antes del relevo sin detener RAD.

El resultado de una instalación solicitada se muestra en esta sección tras la
reapertura; los errores tienen «Ver detalles». No se confunden esos resultados con
los tres de la consulta, ni se afirma éxito por haber lanzado un proceso. La
confianza en GitHub oficial y sus límites están en
[ADR 0018](adr/0018-distribucion-y-actualizacion-de-la-instalacion.md).

## Checklist para cambios en la interfaz

- [ ] En el uso normal no aparecen IDs, códigos, rutas, bytes, nombres de procesos
      ni estados internos (S1).
- [ ] El detalle técnico está en «Avanzado» o en «Ver detalles» (S2).
- [ ] Cada error dice qué pasó en lenguaje llano y ofrece una acción (S3).
- [ ] Al usuario solo se le pide lo que el sistema no puede decidir (S4).
- [ ] Cada control aparece solo cuando se puede usar (S5).
- [ ] Todo texto visible sale del catálogo, o del manifest si es de un plugin, y
      existe en todos los idiomas disponibles.
