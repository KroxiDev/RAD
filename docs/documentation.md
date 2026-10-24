# Estándar documental

Convenciones para mantener la documentación de RAD útil a desarrolladores y
agentes. El [índice](README.md) es el punto de entrada; cada significado tiene una
sola fuente canónica y los demás documentos la enlazan. El árbol de trabajo
documenta el estado vigente: comportamiento, decisiones, compatibilidad,
procedimientos y pendientes reales. La crónica de entregas queda en Git.

## Ubicación y responsabilidad

| Contenido | Fuente canónica | Qué queda fuera |
|---|---|---|
| Presentación pública | `README.md` y `README.es.md` | Gate detallado e instrucciones internas |
| Incorporación y recorrido de trabajo | `CONTRIBUTING.md` | Copias de arquitectura, contratos o comandos de validación |
| Términos del dominio | `CONTEXT.md` | Código, protocolos, estados de implementación y decisiones |
| Relaciones e invariantes del dominio | `docs/domain.md` | Una segunda especificación de formatos o API |
| Dueños, capas y límites de diseño | `docs/architecture.md` | Historial de tareas y resultados de medición |
| Motivo de una decisión arquitectónica | `docs/adr/NNNN-descripcion.md` | Actas de sesión o decisiones editoriales triviales |
| Contratos locales | `docs/client-api.md`, `docs/formats.md` | Duplicaciones manuales de todos los tipos Rust |
| Contrato y guía para autores de plugins | Documentación publicada del SDK, enlazada desde `plugins/README.md` | Otra especificación independiente en este repo |
| Diseño de pantallas y textos | `docs/ux.md` | Manual de usuario o afirmación de que todo está implementado |
| Gate, aislamiento y conservación de tests | `docs/testing.md` | Resultados de una ejecución concreta |
| Trabajo realmente pendiente | `docs/roadmap.md` | Fases completadas, crónica de entregas y reglas permanentes de validación |
| Presupuestos, método y evidencia pertinente de rendimiento | `docs/performance.md` | Informes históricos duplicados, garantías universales o permiso para repetir pruebas reales |
| Reglas de trabajo y permisos de agentes | `AGENTS.md` | Manual técnico completo cargado en cada turno |
| Procedimientos y roles de agentes | `.agentic/` | Configuración personal, memoria o secretos |

La documentación específica de un artefacto permanece junto a él: README de
workspace, plugin, fixture o `packaging/`. Los nombres existentes se conservan
cuando moverlos no mejora la navegación o rompería referencias de evidencia.

## Lenguaje y estructura

- Escribir el cuerpo técnico nuevo o actualizado en español. Conservar nombres
  de símbolos, campos, comandos y códigos tal como existen. Los README públicos
  inglés/español deben expresar los mismos hechos. Los documentos externos del
  SDK y las licencias conservan su idioma y texto originales.
- Usar los términos de [CONTEXT.md](../CONTEXT.md). Si un término resulta ambiguo,
  contrastarlo con escenarios y código; consultar una decisión no deducible antes
  de imponer otra interpretación.
- En `CONTEXT.md`, cada término tiene una definición de una o dos frases.
  `_Evitar_` lista solo sinónimos descartados del mismo concepto; los conceptos
  distintos que suelen mezclarse van en `_No confundir con_`.
- Un título de nivel 1, una apertura que diga propósito y alcance y encabezados
  por tarea o concepto. Separar pasos de trabajo de referencia extensa.
- Usar rutas relativas enlazadas para fuentes locales y código en línea para
  símbolos. Preferir archivo y símbolo a números de línea volátiles; en evidencia
  histórica, conservar el commit o fecha al que corresponden.
- Los bloques de comandos indican contexto, prerequisitos, efectos y límite de
  seguridad. Un ejemplo no concede permiso para instalar, publicar o tocar datos
  personales. Datos de ejemplo sintéticos; nunca secretos ni rutas personales.

## Norma, implementación e historia

Identificar qué afirma cada sección:

- **Norma:** lo aprobado que el sistema debe respetar. Enlazar su ADR cuando
  corresponda. La ausencia de implementación se anota en el roadmap.
- **Implementación actual:** comportamiento comprobado contra las fuentes
  vigentes. Enlazar código y tests en los puntos donde ayudan a verificarlo.
- **Propuesta o pendiente:** trabajo aún no aprobado o no concluido; explicitar
  cuál de los dos. No presentarlo como una capacidad disponible.
- **Evidencia pertinente:** conservar en la referencia vigente fecha o revisión,
  método, escenario y límites necesarios para interpretar un criterio actual.
  El detalle histórico se enlaza a una revisión inmutable de Git. No convertir
  un resultado sintético en validación física ni un caso excluido en una
  comprobación aprobada.

Al terminar un trabajo, retirar su narrativa del roadmap y actualizar la fuente
que describe el comportamiento vigente. No trasladar las fases completadas a otro
documento de archivo local. Git conserva el desarrollo histórico; los ADR conservan
las decisiones y razones necesarias para mantener el sistema, y las referencias
de formatos y fixtures conservan toda compatibilidad soportada.

En contratos, distinguir versión del documento, del contenedor, de la API y del
producto. Documentar compatibilidad, importación y rechazos donde corresponden,
sin inferir que números iguales identifican el mismo contrato.

Los ADR se crean cuando hay una decisión costosa de revertir, sorprendente sin
contexto y nacida de un trade-off real. Usar el número siguiente, un título, estado
y fecha, y en pocas frases el contexto, la decisión y su motivo. Alternativas y
consecuencias se agregan solo cuando aportan algo que no es obvio. Conservar los ADR
aceptados como registro de decisiones, no como crónica de ejecución; una decisión
que los contradiga necesita un ADR nuevo y enlaces entre ambos. Ordenar
documentos no necesita un ADR.

## Mantener un cambio

1. Encontrar la fuente canónica en la tabla; actualizar allí el significado.
   Añadir un documento solo si tiene una responsabilidad propia y un lector claro.
2. Contrastar las afirmaciones de comportamiento con código/tests y las de diseño
   con arquitectura/ADR. Si divergen, explicar la discrepancia y resolverla dentro
   del alcance autorizado, sin modificar producto por inferencia.
3. Actualizar el índice y los enlaces afectados. Si cambia una ruta o encabezado
   referenciado, actualizar sus consumidores o conservar un ancla de compatibilidad.
   Para agentes, añadir un puntero condicionado en `AGENTS.md` solo si necesitan
   alcanzar ese contenido para trabajar.
4. Validar con la [comprobación documental](testing.md#elegir-la-comprobación):
   archivos y anclas locales existentes, bloques cerrados, ejemplos consistentes,
   diff sin errores y lectura de los archivos nuevos. Informar aparte los enlaces
   remotos no verificados.
5. Entregar qué cambió y qué se comprobó. Un plan, checklist marcado o documento
   escrito no sustituye la evidencia de validación.
