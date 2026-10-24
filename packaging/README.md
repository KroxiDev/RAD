# Recursos de escritorio

Referencia para mantener la integración de RAD con el escritorio. El
[índice técnico](../docs/README.md) describe el resto del proyecto.

`rad.png` es la única fuente gráfica de la ventana, el menú de aplicaciones y la
bandeja de RAD. Es la K provisional del GitHub del autor; para sustituirla por la
imagen definitiva, reemplazar este archivo por el PNG y recompilar, sin editar
Rust ni la entrada de escritorio.

El ID estable de aplicación es `io.github.KroxiDev.RAD`. El editor instala la
plantilla `.desktop` y un ícono de 256 px con ese nombre en `$XDG_DATA_HOME` (o
`~/.local/share`), los reescribe solo cuando cambian y hace que `Exec=` apunte al
editor en uso.

Compilar `rad-editor`, `rad-runtime` y `rad-tray` juntos para disponer de la bandeja.
El editor instala ambas unidades de usuario solo cuando cambia su contenido; una
compilación parcial sin `rad-tray` sigue ejecutando la autoridad. No hace falta un
autoinicio independiente de la bandeja. Decisión y límites:
[ADR 0016](../docs/adr/0016-presencia-de-escritorio.md).
