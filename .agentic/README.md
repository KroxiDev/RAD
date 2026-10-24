# Capa agéntica

Procedimientos compartidos para trabajar en RAD. [AGENTS.md](../AGENTS.md) es la
entrada y especifica cuándo leer cada uno; [CLAUDE.md](../CLAUDE.md) importa esa
entrada. Los adaptadores versionados de `.claude/` apuntan a esta capa, sin
duplicar su contenido. La configuración personal de herramientas queda fuera.

La documentación técnica del producto está en el [índice de docs](../docs/README.md);
su organización y escritura siguen el [estándar documental](../docs/documentation.md).

## Procedimientos incluidos

| Procedimiento | Cuándo se usa |
|---|---|
| [Golden rules](golden-rules.md) | Antes de planificar o editar |
| [TDD](skills/tdd.md) | Cambios de comportamiento ejecutable; aplica la [política de tests](../docs/testing.md#ciclo-de-vida-de-los-tests) |
| [Revisión](skills/code-review.md) | Contrastar estándares y especificación sobre commits, cambios locales y archivos nuevos |
| [Orquestación](skills/orquestar.md) | Tareas complejas que ya tienen una especificación |
| [Engram](skills/usar-engram.md) | Recuperar o guardar contexto mediante la integración disponible |
| [CodeGraph](skills/usar-codegraph.md) | Antes de localizar código; usar el índice disponible o el fallback documentado |

Los [roles](agents/) separan investigación, implementación y revisión. Las
[referencias](skills/references/) amplían revisión, tests y dobles de prueba bajo
demanda. Engram y CodeGraph son opcionales: esta capa no instala herramientas,
configura servidores MCP ni crea índices.

## Dónde va cada cosa

| Contenido | Destino |
|---|---|
| Regla de trabajo o autorización de RAD | `AGENTS.md` |
| Regla transversal de claridad y calidad | `.agentic/golden-rules.md` |
| Procedimiento repetible | `.agentic/skills/<nombre>.md`, con condición de lectura en `AGENTS.md` |
| Material extenso de un procedimiento | `.agentic/skills/references/`, enlazado desde su paso pertinente |
| Rol que necesita contexto o permisos distintos | `.agentic/agents/<rol>.md`; adaptador solo donde la herramienta lo necesite |
| Hecho o contrato del producto | Su documento técnico canónico; enlazarlo, no copiarlo en instrucciones |
| Memoria entre sesiones | Mecanismo del entorno, no archivos versionados |
| Configuración y permisos de herramientas | Entorno del usuario, solo con autorización específica |

Un comando CLI, un servidor MCP y un procedimiento son cosas distintas. Un nuevo
procedimiento documenta cuándo usar las herramientas, cómo interpretar su salida
y qué evidencia devolver; no requiere por sí mismo otra configuración.

## Mantener la capa

- Aplicar el skill de escritura para agentes disponible en el entorno antes de
  editar instrucciones. Preferir punteros condicionados a contenido canónico.
- Añadir un rol solo si aísla contexto o necesita permisos distintos, no por una
  etiqueta de especialidad. Resolver directamente las tareas simples.
- Conservar un único cuerpo por procedimiento y revisar sus adaptadores al
  renombrarlo. Retirar una integración requiere un encargo: los directorios de
  configuración del entorno no son restos deducibles de esta documentación.

## El workspace efímero

`.agentic/run/<id>/plan.md` existe durante una orquestación y registra intención,
nunca evidencia. Se ignora mediante `.agentic/run/.gitignore`. Al cerrar, retirar
solo el directorio propio con la política de seguridad de `AGENTS.md`; un run que
sobrevive a una interrupción debe contrastarse con los artefactos actuales antes
de confiar en él.
