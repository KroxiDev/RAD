# Iconos Phosphor

Selección de 200 iconos genéricos de peso **regular**, sin modificar, procedentes de
[Phosphor](https://github.com/phosphor-icons/core) en la revisión
`2b75f3ad12b420c9504ef05df8d2564a28f8500e`. La procedencia y SHA-256 de cada SVG
están en [SOURCE.json](SOURCE.json); se conserva la [licencia MIT](LICENSE).

El editor incluye estos SVG en el binario. No los descarga durante el uso ni abre
SVG proporcionados por el usuario. El catálogo y sus etiquetas están en
[`icon_library/catalog.rs`](../../src/icon_library/catalog.rs); los nombres visibles
se traducen mediante los catálogos de texto del editor.

El resultado elegido se guarda como PNG por la ruta de medios existente. Las
copias portables conservan esa imagen; no conservan una receta vectorial editable.
