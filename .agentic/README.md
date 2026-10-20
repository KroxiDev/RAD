# Capa agéntica

El contenido real vive una sola vez en `.agentic/`. Los archivos bajo `.claude/`
y `.codex/` son adaptadores de 3 a 6 líneas que apuntan acá, para que las dos
herramientas lean lo mismo.

```text
.agentic/
  golden-rules.md                # reglas siempre vigentes
  skills/orquestar.md            # el procedimiento de orquestación
  skills/completar-contrato.md    # llenar los [PENDIENTE] de AGENTS.md
  skills/tdd.md                  # ciclo rojo–verde–refactor
  skills/code-review.md          # estándares y especificación
  skills/usar-engram.md          # contexto entre sesiones, si está disponible
  skills/usar-codegraph.md       # índice de código, si está disponible
  skills/references/             # revisión, tests y dobles de prueba
  agents/                       # personas de los roles
  run/                          # workspace efímero, fuera de git
```

Para retirar la capa: borrar `.agentic/`, `.claude/`, `.codex/`, y el bloque
`AGENTIC_TEMPLATE` de `AGENTS.md` y `CLAUDE.md`.

## Primer paso: completar el contrato

`AGENTS.md` llega con campos `[PENDIENTE]`. Para llenarlos:

- **Claude** → `/completar-contrato`
- **Codex** → `Leé .agentic/skills/completar-contrato.md y aplicalo`

Recorre el repositorio, propone lo que puede inferir con su evidencia, y pregunta
solo lo que no. Lo que no se confirma queda `[PENDIENTE]`: un hueco declarado se
ve, un valor inventado no.

Sirve igual más adelante, cada vez que agregues un campo nuevo al contrato.

## Procedimientos incluidos

`AGENTS.md` indica cuándo aplicar cada skill, tanto en tareas directas como
delegadas; los roles implementador y revisor también enlazan su procedimiento.

- [TDD](skills/tdd.md): para cambios de comportamiento ejecutable. Usa la
  política de tests del contrato; las guías de tests y mocking se consultan al
  diseñar casos o aislar dependencias.
- [Revisión](skills/code-review.md): contrasta estándares y especificación con
  evidencia del estado exacto revisado, incluidos cambios locales y archivos nuevos.
- [Engram](skills/usar-engram.md): recupera y guarda contexto siguiendo el
  protocolo y las autorizaciones del entorno.
- [CodeGraph](skills/usar-codegraph.md): consulta el índice antes de explorar
  código cuando está disponible; de lo contrario, usa búsqueda y lectura directa.

Engram y CodeGraph son integraciones opcionales del entorno: `init` no instala
herramientas, configura servidores MCP ni crea índices. Los procedimientos
compartidos no necesitan adaptadores nuevos; se alcanzan desde `AGENTS.md`
(Claude lo importa mediante `CLAUDE.md`).

## Dónde va cada cosa

| Quiero agregar… | Va en… | Lo lee |
| --- | --- | --- |
| Un hecho del proyecto (propósito, comandos, puertos) | `AGENTS.md` | Codex directo; Claude vía `@AGENTS.md` |
| Una regla que aplica siempre | `AGENTS.md`, o `.agentic/golden-rules.md` si es universal | ambas |
| Una regla solo para ciertos archivos | `.claude/rules/<tema>.md` con frontmatter `paths:` | Claude |
| Un procedimiento repetible | `.agentic/skills/<nombre>.md` + línea en `AGENTS.md` | ambas |
| Un rol delegable | `.agentic/agents/<nombre>.md` + adaptadores | ambas |
| Un servidor MCP | `.mcp.json` (Claude) y `.codex/config.toml` (Codex) | cada una la suya |
| Un comando CLI que los agentes pueden ejecutar | permiso en `.claude/settings.json` / `sandbox_mode`, y cuándo usarlo en `AGENTS.md` | ambas |
| Un comando slash | `.claude/commands/<nombre>.md` | Claude |
| Documentación del proyecto | donde ya vive. **Enlazarla** desde `AGENTS.md`, no copiarla | ambas, a demanda |
| Memoria entre sesiones | el mecanismo del entorno, no archivos versionados | — |

## Tools: casi nunca son una carpeta

Para un agente, una "tool" es una de tres cosas, y ninguna necesita un directorio
propio:

1. **Un comando CLI** — necesita un permiso y una línea que diga cuándo usarlo.
2. **Un servidor MCP** — necesita configuración del cliente.
3. **Un procedimiento que envuelve un comando** — eso es una skill.

La mayoría de lo que parece una tool es en realidad el caso 3. Lo que importa no
es el comando, sino cuándo seleccionarlo, cómo leer su resultado y qué evidencia
devolver. Eso va en `.agentic/skills/`.

### Ejemplo: agregar un control de calidad

```text
.agentic/skills/calidad-python.md
```

Con el procedimiento: cuándo se selecciona el control, el comando exacto, cómo
interpretar cada estado del resultado, y qué evidencia debe acompañar al informe.
Después, una línea en `AGENTS.md`:

> Para control de calidad Python, aplicar `.agentic/skills/calidad-python.md`
> cuando se solicite.

Con eso funciona en Claude y en Codex sin configuración extra. El adaptador en
`.claude/skills/` se agrega solo si querés que Claude la descubra por su cuenta.

## Cómo crecer

Por defecto la capa es chica. Crece cuando el proyecto lo pide, no antes.

| Cuando el proyecto… | Agregar |
| --- | --- |
| repite un procedimiento | una skill en `.agentic/skills/` |
| tiene reglas que solo aplican a parte del repo | `.claude/rules/<tema>.md` con `paths:` |
| ve `AGENTS.md` pasar las ~200 líneas | mover secciones a rules por path |
| usa MCP | `.mcp.json` y/o `.codex/config.toml` |
| necesita paralelizar trabajo sin dependencias | otro rol en `.agentic/agents/` |
| es un monorepo | `AGENTS.md` anidados por subdirectorio |

Regla para agregar un rol: **se gana el lugar solo si aísla contexto o necesita
otro conjunto de permisos.** No por ser "un especialista en X".

## El workspace efímero

`.agentic/run/<id>/plan.md` existe mientras dura una orquestación y se borra al
cerrar. Se ignora solo, mediante `.agentic/run/.gitignore`, así que no hace falta
tocar el `.gitignore` del proyecto.

Sin kernel, sin schemas, sin protocolo: un markdown que el orquestador escribe y
relee. Registra intención, nunca evidencia.
