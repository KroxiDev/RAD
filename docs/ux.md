# Experiencia de usuario

Norma de cómo se comporta y cómo habla el editor. El estilo visual (simple, oscuro
y profesional, cercano a Elgato sin clonar su identidad) está en
[AGENTS.md](../AGENTS.md); la arquitectura que lo sostiene, en
[architecture.md](architecture.md).

## Principio

El usuario ve el funcionamiento, no el mecanismo. RAD se usa sin conocimientos
técnicos; los detalles técnicos existen y están a mano para quien los busque, fuera
del uso normal. Vale para todo el programa. La CLI es la vía técnica y conserva el
detalle.

- **S1 · Funcionamiento, no mecanismo.** En el uso normal no aparecen IDs,
  revisiones, generaciones, códigos, rutas, bytes ni nombres de procesos.
- **S2 · Lo técnico, en un solo lugar.** «Avanzado», en Ajustes y oculto por
  defecto, reúne los IDs con botón de copiar (para scripts), los permisos de
  automatización, el registro reciente, las versiones y la información para soporte.
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
- El idioma por defecto es el del sistema; se cambia en las preferencias del editor.
- La API entrega códigos y el editor los traduce por catálogo. Un código desconocido
  se muestra como «Algo salió mal» con «Ver detalles».

## Vocabulario

Referencia en español; el catálogo inglés usa sus equivalentes.

- **Copia:** lo que el usuario exporta y restaura. Lleva todo, salvo las contraseñas
  y lo propio del equipo.
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
- **Restaurar** es una sola acción, sin preguntas sobre IDs, y se puede deshacer.
- **Plugins.** Los oficiales se instalan desde una lista, con un botón; uno de
  terceros, pegando su enlace de GitHub. Actualizar usa el origen registrado. Mientras
  un plugin funcione, no hay avisos.
- **Gestos.** El editor ofrece pulsar, mantener o pulsación larga solo si la acción
  los admite.
- **Progreso.** Las transferencias muestran una barra.
- **Puesta en marcha.** El editor arranca RAD y ofrece «Iniciar RAD con el equipo»;
  el usuario nunca usa la terminal ([ADR 0012](adr/0012-servicio-gestionado-por-el-editor.md)).

## Checklist para cambios en la interfaz

- [ ] En el uso normal no aparecen IDs, códigos, rutas, bytes, nombres de procesos
      ni estados internos (S1).
- [ ] El detalle técnico está en «Avanzado» o en «Ver detalles» (S2).
- [ ] Cada error dice qué pasó en lenguaje llano y ofrece una acción (S3).
- [ ] Al usuario solo se le pide lo que el sistema no puede decidir (S4).
- [ ] Cada control aparece solo cuando se puede usar (S5).
- [ ] Todo texto visible sale del catálogo y existe en todos los idiomas disponibles.
