# Golden de documento v5

`profile.json` es sintético, generado con el escritor v5 anterior al cambio a v6,
en la base `b83f65f3deab93850dc4a18fc252942adee317a5`. El generador conservado en
[generate.rs.txt](generate.rs.txt) parte del golden v4 y añade Brightness; no usa
datos personales. Se ejecutó antes de modificar el escritor.

Lo prueban el importador cerrado de [v6](../../../crates/rad-codec/tests/v6.rs) y
la migración de [Store](../../../crates/rad-store/tests/v2.rs), incluido el respaldo
exacto `.document-v5-backup`. No se regenera con el escritor actual.
