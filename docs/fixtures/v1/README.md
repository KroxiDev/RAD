# Fixtures originales v1

`profile.json`, `profile.sqlite3` y `profile.radpkg` se generaron **antes de
modificar producción** desde `41b7f761399589fb87fbcd144db4c0d562fb69ce`.
El perfil sintético usa instalación 77, un botón `Fixture launch`, acción
`fixture.desktop` (no se ejecuta) y un segundo botón borrado. El Store conserva
cinco revisiones y el registro del botón retirado. No contiene datos personales,
medios, credenciales ni rutas del equipo.

`generate.rs.txt` conserva el generador exacto como documentación reproducible:
copiar a `crates/rad-package/tests/generate_v1.rs` en ese commit y ejecutar
`cargo test --offline --locked --release -p rad-package --test generate_v1 generate_original_v1_fixture`.
Los archivos de destino deben estar ausentes. SQLite usa journal DELETE; se cerró
antes de conservar el archivo. No reescribir estos golden con el escritor v2.

SHA-256:

- `profile.json`: `0a5286068f3ab2fb59cadbe809a7906bdfc3ec72fd586009340ee77eb29ec7d5`
- `profile.radpkg`: `4ac7309141053f2daec63c16af3f8cd922b51621162f3e4796b2e670142cdb3b`
- `profile.sqlite3`: `9b6f3cfbfe28c4e05ef6893ad1b1faa06297b22c3eba2acbb0602e6eaca0dc19`

## Fixture grande en el límite

`large.json`, `large.sqlite3` y `large.radpkg` se generaron también con las fuentes
originales de `41b7f761399589fb87fbcd144db4c0d562fb69ce`, extraídas con `git show`
sin cambiar el checkout de desarrollo. Se compilaron core, codec, Store y package
originales con `rustc --edition=2024 --crate-type rlib`, reutilizando las dependencias
ya compiladas del repositorio. `generate_large.rs.txt` es el programa ejecutado
contra esas bibliotecas originales (no el escritor v2).

Tiene 7200 entidades válidas en 17 carpetas, acciones Wait(0), sin medios. El
JSON mínimo original ocupa 1035314 bytes; nombres sintéticos `x` lo completan a
**1048576 bytes exactos**, el máximo original. La instalación sintética es 1.
Prueba tanto migración/restauración de datos existentes mayores que el nuevo techo
de creación de 7000 como la ausencia de crecimiento por overhead al convertir a v2.

SHA-256:

- `large.json`: `d412dee216a8e9607b55266c4655b18391cc795e98734e0a3e03cdc15e2fecff`
- `large.radpkg`: `fe46727c37c97042c7839ec324a2ed8232369f330a6b17e4915c5a3dc0d23417`
- `large.sqlite3`: `33d7282993266ea4751b09cf5b10e628cd0bdc95200dcd51fb54c976dcbbbf00`
