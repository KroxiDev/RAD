# Textos de licencias recuperados

Textos originales de dependencias cuya publicación en crates.io no incluía sus
archivos de licencia, más los del toolchain Rust usado para la base actual.
Se recuperaron únicamente de los repositorios oficiales públicos, fijados a los
commits de `.cargo_vcs_info.json` o al commit del compilador, con autorización del
usuario. No hay código ejecutable remoto ni claves aquí.

[`manifest.json`](manifest.json) registra paquete/versión, repositorio, revisión y
SHA-256 de cada texto. `release.py` exige coincidencia de versión **y** revisión
del crate cacheado antes de usar un texto recuperado; el build no hace peticiones
de red. La copia instalada incluye su `SOURCE.json`. Una actualización de crate o
toolchain debe revisar esta procedencia, no reutilizar silenciosamente otro texto.

- AccessKit: MIT, Apache y avisos Chromium/autores de su revisión.
- egui/eframe: MIT y Apache de la revisión compartida de sus crates.
- gl-rs: Apache de las revisiones de gl_generator y khronos_api.
- profiling: MIT y Apache de su revisión.
- Rust estándar: MIT y Apache de la revisión exacta del compilador.

El resto de notices procede de la caché Cargo validada por el lockfile. Los textos
individuales de las fuentes Ubuntu y Hack de `epaint_default_fonts` se conservan
además de los demás avisos de fuentes, aunque sus nombres no contengan «LICENSE».
Los originales no se traducen ni se reformatean.

Esta preservación trazable no es una auditoría legal exhaustiva del toolchain,
sus dependencias internas o todas las plataformas. Antes de publicar un artefacto,
revisar el inventario efectivo y sus obligaciones; no atribuir a esta herramienta
una certificación que no ejecuta.
