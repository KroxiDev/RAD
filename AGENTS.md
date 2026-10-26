# AGENTS.md — RAD · Rust Action Deck

## Proyecto y alcance

RAD configura y usa un Stream Deck mediante un editor visual y automatización local.
El runtime sin GUI es la única autoridad de configuración, ejecución y navegación y
el dueño del deck por hidraw. El editor eframe/egui (Glow) y `rad-cli` son clientes
del socket local; cada plugin activo tiene su propio proceso.

Entrypoints: `crates/rad-runtime/src/bin/rad-runtime.rs`,
`crates/rad-editor/src/main.rs` y `crates/rad-client/src/bin/rad-cli.rs`.
El código y los tests son la fuente de verdad del comportamiento actual.

La arquitectura aprobada está en [docs/architecture.md](docs/architecture.md),
con sus decisiones en `docs/adr/` y los pendientes en
[docs/roadmap.md](docs/roadmap.md). Cada ítem requiere encargo del usuario; figurar
como pendiente no autoriza su ejecución. El editor debe ser simple, oscuro y
profesional, cercano a Elgato sin clonar su identidad; cada control visible solo
cuando se pueda usar. Lo que exceda esa arquitectura requiere pedido del usuario.
Prioridades: estabilidad, respuesta percibida, UX y después ahorro marginal de
recursos. Simple de cara al usuario; por dentro, robusto, con dueños y contratos
claros.

## Procedimientos

- Antes de planificar o editar: leer `.agentic/golden-rules.md`.
- Para incorporarse al desarrollo o encontrar documentación: `CONTRIBUTING.md` y
  `docs/README.md`.
- Antes de nombrar o cambiar conceptos del dominio: `CONTEXT.md`; para relaciones,
  invariantes y trazabilidad al código, `docs/domain.md`.
- Antes de crear, actualizar o reorganizar documentación: `docs/documentation.md`.
- Antes de diseñar o cambiar la base estable (dominio, autoridad, runtime,
  persistencia, contratos o alojamiento de plugins): leer `docs/architecture.md` y
  los ADR afectados. Un cambio que los contradiga lleva un ADR nuevo.
- Antes de trabajar en un pendiente: `docs/roadmap.md`.
- Para pantallas o textos del editor: `docs/ux.md`.
- Para comportamiento ejecutable: aplicar `.agentic/skills/tdd.md`.
- Para revisar estándares y requisitos: `.agentic/skills/code-review.md`.
- Para recuperar o guardar contexto: `.agentic/skills/usar-engram.md`.
- Antes de localizar o comprender código: `.agentic/skills/usar-codegraph.md`;
  consultar el índice disponible o recurrir a búsqueda y lectura directa.
- Para tareas complejas ya especificadas: `.agentic/skills/orquestar.md`, con los
  roles de `.agentic/agents/`. Las tareas simples se resuelven directamente.
- Para editar instrucciones de agentes: aplicar el skill de escritura para
  agentes disponible en el entorno. La capa compartida vive en `.agentic/`.

## Validación completa

Antes de compilar, probar o cerrar una entrega: aplicar
[docs/testing.md](docs/testing.md). Es la fuente única del gate en serie de ambos
workspaces, variables de helpers, conformidades, requisitos y límites de evidencia.

## Ciclo de vida de los tests

Antes de añadir o retirar tests: aplicar la
[política de conservación](docs/testing.md#ciclo-de-vida-de-los-tests).

## Git y GitHub

- Rama por defecto `main`; `origin` es `KroxiDev/RAD`, público. Antes de editar,
  comprobar rama, status y diff; preservar cambios ajenos.
- Commit y push progresivos de entregas verificadas están autorizados. Mensajes
  breves en inglés con Conventional Commits (`fix:`, `feat:`, `docs:`, etc.).
- Crear/cambiar/borrar ramas, reescribir historia o descartar cambios requiere
  autorización explícita y específica, incluso clean/reset/restore destructivos
  y force-push. No abrir PR ni configurar remotos por inferencia.
- Para GitHub usar el MCP `github`. Usar `gh` solo si falta conexión/operación o
  falla el MCP, explicando el motivo. El checkout se opera con `git`; con checkout
  local no escribir archivos directamente al remoto mediante MCP.
- Releases: título exactamente `RAD X.Y.Z-Q` y descripción vacía, sin notas
  adicionales (`X.Y.Z` es la versión y `Q` el `pkgrel`).

## Seguridad

- Preservar secretos: no leer ni mostrar tokens, claves o archivos `.env`.
- Al operar el XL en pruebas, hacerlo solo con autorización y control exclusivo:
  nunca RAD y StreamController escribiendo a la vez; preservar todos los datos de
  StreamController, sin migrarlos ni modificarlos. Esta regla es para agentes: el
  producto no tiene en cuenta otros programas (ADR 0006).
- Probar fallos con escenarios sintéticos controlados, sin sondeos generales de
  sensores ni reproducción de bloqueos del driver.
- Instalar herramientas, publicar o acceder a remotos requiere autorización.
  El repositorio es público: versionar solo contenido publicable, sin datos
  personales, rutas locales ni secretos.
- Aislamiento estricto: no tocar, reiniciar, reconstruir, migrar, detener ni leer
  secretos de otros proyectos sin aprobación explícita y específica.
- Al retirar archivos sin respaldo comprobado en un repo, usar la papelera,
  no eliminación permanente.
