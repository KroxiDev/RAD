# Fixture canónico v2

`profile.json` se generó con el escritor v2 desde el perfil v1 sintético importado,
añadiendo ajustes opacos `00ff2a` de `fixture.plugin` y el hint de dependencia
versión 1.2.3, origen `fixture/plugin`. No son un plugin real ni credenciales.
`generate.rs.txt` conserva el generador usado; no forma parte de la suite.
La prueba `rad-codec/tests/v2.rs` valida su semántica y los bytes canónicos exactos.

`profile.sqlite3` y `profile.radpkg` se generaron con los escritores v2, desde ese
JSON y el registro completo del perfil v1 pequeño importado. Conservan instalación
77, el botón `b2` retirado y sus reservas (botones=2, serial=4). El Store nuevo tiene
una revisión; la copia no contiene medios, binarios, preferencias ni credenciales.
`generate_storage.rs.txt` conserva el generador exacto: copiarlo temporalmente a
`crates/rad-package/tests/generate_v2.rs` y ejecutar
`cargo test --offline --locked --release -p rad-package --test generate_v2 generate_current_v2_fixtures`.
Los archivos de destino deben estar ausentes; nunca regenerar un golden durante
la suite. Las pruebas copian el Store antes de abrirlo para no alterar el original.

SHA-256:

- `profile.json`: `db9ce4bae16000194ae96bd73517ffee7123a1f32d3f4750732c19b10d3e83db`
- `profile.radpkg`: `6be2a0dc8329d1a6003d07fa996e9426e9cd3b8c5c96cd101d3b47e30e7d7fe2`
- `profile.sqlite3`: `e1771323b6ce5cbcf980409c5bef16dd01cd5274c4627e1af666a2c08024c3f5`

La rejilla documental es `[8,4]`; los mapas de plugins se omiten solo cuando
están vacíos. Así importar v1 no aumenta el documento y no rompe perfiles en el
límite original de 1 MiB. La vista API de rejilla sigue siendo un objeto.
