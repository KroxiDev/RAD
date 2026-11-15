# 0023 · Identidad de actualización y entorno de construcción

**Estado:** aceptada, 2026-11-14. Amplía los entornos de construcción de
[ADR 0018](0018-distribucion-y-actualizacion-de-la-instalacion.md).

El entorno de construcción ha pasado de Omarchy 4.0.6 a 4.5.0. Los actualizadores
ya distribuidos comparan literalmente el campo `base` con su identidad compilada;
cambiarlo impediría que reconocieran la nueva release. La versión de Omarchy
tampoco fija sus bibliotecas rolling ni garantiza compatibilidad binaria.

Se admite construir en Omarchy 4.0.6 y 4.5.0 sobre x86-64, comprobando el
`/etc/os-release` real y rechazando otras bases. Se conservan el esquema 1 y la
identidad de compatibilidad `base: "omarchy-4.0.6"` en `release.json` y en la
metadata de actualización. `build-environment.txt` registra `build_base` como
entorno real, además de la identidad de compatibilidad y las versiones de las
dependencias. No se añaden campos a la metadata que leen los clientes existentes.

La ABI mínima medida en los binarios y las dependencias de pacman siguen
determinando los requisitos del paquete; la etiqueta de compatibilidad no promete
ejecución sobre un snapshot antiguo. Congelar y validar un entorno reproducible
sigue pendiente. Se mantienen la fuente limpia, los controles de identidad y
licencias, y la publicación exclusiva desde el `main` vigente.
