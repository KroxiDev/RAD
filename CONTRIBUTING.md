# Contribuir a RAD

Guía de incorporación para desarrolladores y agentes. El [mapa documental](docs/README.md)
permite entrar por tarea; esta página describe el recorrido de una contribución,
no el uso de la aplicación.

## Preparar el trabajo

1. Leer [AGENTS.md](AGENTS.md) y las [golden rules](.agentic/golden-rules.md):
   alcance, seguridad, Git y condiciones para operar recursos externos.
2. Comprobar rama, `git status` y diff; preservar cambios ajenos. Acordar objetivo,
   criterios de aceptación y archivos afectados antes de editar.
3. Leer el [glosario](CONTEXT.md), el [modelo de dominio](docs/domain.md) y la
   [arquitectura](docs/architecture.md). Para una tarea concreta, usar su tabla de
   [dónde vive cada cambio](docs/architecture.md#dónde-vive-cada-cambio) y los ADR
   afectados. Consultar los [pendientes](docs/roadmap.md) antes de elegir trabajo:
   un ítem pendiente no autoriza por sí solo su ejecución.

## Entorno de desarrollo

RAD se desarrolla en Linux con un toolchain Rust que cumpla el `rust-version`
declarado en los manifiestos (hoy 1.92, por `rad-launcher`); todos los crates usan
la edición 2024.
La aplicación y los plugins son **dos workspaces Cargo**:
[raíz](Cargo.toml) y [plugins](plugins/Cargo.toml), cada uno con su lockfile.
Los manifiestos son la fuente de dependencias y targets; los límites de capas
están en [arquitectura](docs/architecture.md#capas).

La [validación completa](docs/testing.md#validación-completa) fija caché, target,
concurrencia, flags y orden de build/tests. Usar ese entorno también para checks
focales y compilar primero los helpers y ejemplos. Si la caché o una dependencia
del sistema falta, informar el impedimento: no descargar ni instalar por inferencia.

Compilar y ejecutar la aplicación son operaciones distintas. Abrir el editor puede
instalar recursos de escritorio, arrancar el servicio y usar datos de la sesión.
Los tests habituales se ejecutan sin ventanas ni hardware y con servicios privados;
una comprobación real requiere autorización y un plan de aislamiento específico.

## Implementar

Elegir los documentos de la tarea en el [índice técnico](docs/README.md). Un
cambio de API, formato o contrato de plugins declara su compatibilidad y actualiza
tipos, vectores y referencia canónica en la misma entrega.

## Verificar y entregar

1. Ejecutar las [comprobaciones correspondientes](docs/testing.md) sobre el estado
   final. Declarar qué no pudo verificarse y por qué; los tests sintéticos no
   certifican otras distribuciones, el hardware ni los servicios personales.
2. Revisar [estándares y especificación](.agentic/skills/code-review.md), incluyendo
   archivos nuevos y cambios del working tree, no solo commits. La evidencia debe
   corresponder al contenido entregado.
3. Actualizar documentación afectada y retirar únicamente temporales propios según
   la política de conservación y seguridad. Conservar evidencia histórica relevante.
4. Entregar alcance, validación y pendientes. Seguir las instrucciones vigentes de
   [Git y GitHub](AGENTS.md#git-y-github) y del encargo: la preparación local no
   implica publicación, PR ni permiso para reescribir historia.
