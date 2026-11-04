# Golden de documento v6

`profile.json` es sintético, generado con el escritor v6 anterior al cambio a v7,
en la base `b3f0499ea3ed57f13785558a767498f26466785f`. El generador conservado en
[generate.rs.txt](generate.rs.txt) parte del golden v5 y añade dos caras y la
posición explícita `above_image`; no usa datos personales. Se ejecutó como test
de `rad-codec` antes de modificar el escritor. No se regenera con el escritor actual.

`profile.radpkg` conserva el contenedor RADPKG v2 con ese documento v6, exportado
por el código anterior al cambio mediante [generate-package.rs.txt](generate-package.rs.txt)
como test de `rad-package`.
