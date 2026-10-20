# 0004 · Corte limpio del formato e importadores por versión

**Estado:** aceptada, 2026-10-20.

Antes de declarar estable la v1, el formato canónico (documento v2, Store y RADPKG
v2) queda sin compatibilidades prerelease (`pauses_ms`) y sin el idioma, que pasa al
editor. El corte no puede romper los datos existentes: `Settings` rechaza campos
desconocidos y exige `gui_language`, y el codec rechaza toda versión distinta de 1,
así que sin un lector de v1 tampoco abriría la configuración que hoy guarda el
Store.

Por eso lo heredado vive en un único importador por versión, aislado en el codec y
usado por los dos caminos que leen datos viejos:

- la apertura del Store, con una migración única y atómica que conserva el archivo
  anterior;
- la restauración de una copia antigua.

El importador de v1 copia el resto tal cual, sigue rechazando pausas distintas de 0
y descarta el idioma: el editor usa el del sistema hasta que el usuario lo cambie.
Los contadores de identidad salen del registro completo en el Store y, en una copia
v1, que no los trae, de los IDs presentes.

Los importadores no se retiran: cada uno es pequeño y tiene su golden real, generado
con la versión de origen desde un perfil de prueba. Después del corte, solo
migraciones hacia adelante y sin downgrade. Una configuración de una versión más
nueva lleva al modo recuperación con «Actualiza RAD»
([ADR 0011](0011-salidas-y-modo-recuperacion.md)).

**Consecuencias:** con una copia v1, un script que ya fallaba en el origen, por
apuntar a un ID borrado por encima del máximo presente, podría acertar a un botón
nuevo. Es un límite declarado de v1.
