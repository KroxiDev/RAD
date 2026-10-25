# Documentación técnica de RAD

Entrada para **desarrolladores y agentes** que mantienen RAD o integran sus
contratos. No es un manual de uso de la aplicación. El español es el idioma
canónico de esta documentación; los README públicos se conservan en inglés y
español.

## Por dónde empezar

| Necesidad | Leer |
|---|---|
| Incorporarse al desarrollo | [Contribuir](../CONTRIBUTING.md) |
| Entender los términos del producto | [Glosario](../CONTEXT.md) |
| Entender relaciones, invariantes y casos límite | [Modelo de dominio](domain.md) |
| Ubicar responsabilidades y dependencias | [Arquitectura](architecture.md), en particular [capas](architecture.md#capas) y [dónde vive cada cambio](architecture.md#dónde-vive-cada-cambio) |
| Comprender por qué se eligió un diseño | [Índice de ADR](architecture.md#decisiones-registradas) |
| Cambiar pantallas o textos | [Norma de UX](ux.md) y [pendientes](roadmap.md#ux-pendiente) |
| Crear un cliente o automatizar RAD | [API local y CLI](client-api.md) |
| Cambiar persistencia, migraciones o copias | [Formatos](formats.md) y [fixtures históricos](fixtures/v1/README.md) ([v2](fixtures/v2/README.md), [v3](fixtures/v3/README.md)) |
| Desarrollar un plugin | [Entrada del workspace de plugins](../plugins/README.md) y [arnés de compatibilidad](plugin-check.md) |
| Empaquetar, distribuir o recuperar una actualización | [Distribución e integración](../packaging/README.md) y [ADR 0018](adr/0018-distribucion-y-actualizacion-de-la-instalacion.md) |
| Compilar y verificar cambios | [Pruebas y validación](testing.md) |
| Consultar presupuestos y evidencia de rendimiento | [Rendimiento](performance.md) |
| Trabajar como agente | [AGENTS.md](../AGENTS.md) y [capa agéntica](../.agentic/README.md) |
| Elegir trabajo pendiente | [Pendientes de RAD](roadmap.md) |
| Crear o mantener documentación | [Estándar documental](documentation.md) |

## Documentación junto a sus artefactos

- [Distribución e integración de escritorio](../packaging/README.md): paquete Arch, releases, confianza y recuperación.
- [Fixture del host](../crates/rad-plugin-host/fixture/README.md) y
  [fixture de métricas](../scripts/conformance/metrics/fixture/README.md): instrumentos
  de prueba, no paquetes de producción.
- [Licencias de terceros](../plugins/notices/README.md): textos conservados, no
  documentación técnica que deba traducirse o reescribirse.
