# Referencia de revisión

## Alcance y estado

1. Tomar el alcance y las fuentes del encargo vigente: brief, criterios o especificación
   autorizada. Un enlace a un tracker solo se consulta si hace falta y el acceso está autorizado.
   Aclarar una base o un requisito solo cuando falte y la decisión cambie lo que se revisa.
2. Identificar repositorio, archivos incluidos, base y objetivo. Resolver las referencias Git a
   sus SHA para que una rama móvil no vuelva ambigua la evidencia. Para trabajo sin commit,
   registrar también el estado de los archivos; `HEAD` por sí solo no identifica ese contenido.
3. Elegir la comparación según la entrega. En los ejemplos, `<rutas>` representa los archivos o
   directorios autorizados, `<base>` el punto acordado y `<objetivo>` la versión a revisar.

| Entrega | Comparación de lectura |
| --- | --- |
| Dos versiones exactas | `git diff <base> <objetivo> -- <rutas>` |
| Cambios de una rama desde su divergencia | `git diff <base>...<objetivo> -- <rutas>` |
| Solo cambios staged | `git diff --cached -- <rutas>` |
| Solo cambios locales fuera del índice | `git diff -- <rutas>` |
| Estado local completo de archivos rastreados | `git diff HEAD -- <rutas>` |
| Commits y trabajo local desde una base exacta | `git diff <base> -- <rutas>` |

Los tres puntos comparan el ancestro común con el objetivo y solo incluyen commits. Si el
encargo incluye también trabajo local de esa rama, obtener el SHA con
`git merge-base <base> HEAD` y usarlo como base exacta para comparar con el árbol de trabajo.

Para cualquier entrega local, consultar `git status --short --untracked-files=all -- <rutas>`.
Los diffs anteriores omiten archivos nuevos no rastreados: enumerarlos y leer su contenido
autorizado como parte del cambio, sin agregarlo al índice. Respetar las rutas protegidas y los
secretos. Si la entrega nombra archivos ignorados, comprobar esos archivos explícitos también;
el estado de Git no representa por sí solo todos los artefactos posibles.

La comparación contra `HEAD` representa el resultado local combinado; los diffs con y sin
`--cached` permiten entender sus partes. Revisar el estado solicitado y evitar contar dos veces
el mismo cambio. Si el objetivo es lo staged, la evidencia debe corresponder al índice aunque
el archivo de trabajo tenga cambios posteriores. Si se revisa un commit distinto del árbol
actual, leer su contenido y limitar la evidencia de ejecución a la versión que realmente se probó.

Registrar el SHA y, para archivos locales, huellas de contenido u otra evidencia equivalente
que identifique lo revisado. Comprobar al cerrar que ese estado sigue vigente; cambios posteriores
requieren revisar lo afectado o declarar que quedaron fuera de la revisión.

### Diff vacío o incompleto

Comprobar base, objetivo, rutas, índice y archivos nuevos antes de declarar que no hay cambios.
Si el encargo es una revisión del estado actual, leer los archivos autorizados aunque no haya
diff. Si no existe un artefacto que revisar, informar esa limitación; la ausencia de diferencias
no demuestra el cumplimiento de requisitos. Si falta acceso o una versión no está disponible,
conservar lo verificable y señalar lo que falta, según los veredictos del rol.

## Estándares

Relacionar cada hallazgo con una regla documentada y aplicable al archivo: reglas del proyecto,
convenciones de su componente o decisiones arquitectónicas vigentes. Leer el contexto suficiente
para comprobar el efecto del cambio, sin extender la revisión a una limpieza general del repo.
Separar los problemas introducidos o agravados por el cambio de los preexistentes pertinentes.

### Heurísticas de diseño

Los olores de diseño orientan preguntas; no son incumplimientos automáticos. Los estándares del
repositorio prevalecen. Una sugerencia requiere una consecuencia concreta para este cambio,
como divergencia de reglas, acoplamiento que dificulta una modificación prevista o pérdida de
claridad. No prescribir refactors por preferencia personal ni implementarlos durante la revisión.

| Indicio | Cuándo merece un hallazgo |
| --- | --- |
| Nombres opacos | Ocultan una responsabilidad o permiten confundir conceptos del dominio. |
| Duplicación | Repite una misma regla que puede divergir al cambiarla; no solo texto similar. |
| Feature envy | Una operación depende de detalles ajenos y rompe un límite útil del módulo. |
| Data clumps | Los mismos datos viajan juntos y se pierde una invariante común entre ellos. |
| Primitive obsession | Valores genéricos dejan mezclar conceptos o aceptar estados inválidos. |
| Switches repetidos | Una misma clasificación exige cambios coordinados en varios lugares. |
| Cambios dispersos | Una modificación del dominio obliga a tocar componentes no relacionados. |
| Múltiples razones de cambio | Un módulo mezcla responsabilidades con evoluciones distintas. |
| Generalidad especulativa | Abstracciones para casos sin requisito agregan coste al caso real. |
| Message chains | Un consumidor conoce la estructura interna de varios objetos encadenados. |
| Middle man | Una capa solo reenvía llamadas sin aportar un contrato o aislamiento útil. |
| Refused bequest | Un subtipo rechaza parte del contrato que sus consumidores esperan. |

Una fachada que aísla un proveedor puede justificar delegación; dos reglas distintas pueden
tener código parecido. Considerar estas razones antes de recomendar eliminar una capa o unificar
implementaciones. Si no hay regla incumplida ni defecto demostrado, clasificar la propuesta como
mejora opcional y explicar cuándo aportaría valor.

## Especificación

1. Extraer requisitos observables de las fuentes autorizadas, incluida la conversación o el
   brief ya recibido. Distinguir obligaciones, ejemplos ilustrativos y decisiones pendientes.
2. Relacionar cada requisito con implementación y evidencia: ubicación, prueba o comprobación
   ejecutada, resultado y versión. Una intención en el plan no demuestra que se haya implementado.
3. Buscar **omisiones**: requisitos sin implementación o casos obligatorios sin cubrir;
   **ampliaciones de alcance**: comportamiento agregado sin respaldo en el encargo; e
   **implementaciones incorrectas**: existe la función, pero su contrato contradice lo pedido.
4. Si falta especificación, evaluar los estándares disponibles y delimitar los requisitos que
   no se pueden verificar. Los tests y el código ayudan a entender el comportamiento, pero no
   sustituyen la intención autorizada ni prueban por sí solos conformidad con ella.

## Evidencia y entrega

Usar los veredictos y límites del [rol Revisor](../../agents/revisor.md). Presentar ambos ejes
por separado, con ubicación precisa, fuente aplicable y consecuencia para cada hallazgo. Por
ejemplo: «Especificación, requisito 3: `archivo:línea` admite un estado excluido por el brief;
la comprobación X reproduce la respuesta incorrecta». Vincular los hallazgos que afecten ambos
ejes sin duplicar su explicación.

Indicar las comprobaciones ejecutadas, su resultado real y las que quedaron pendientes. La
evidencia de otra versión, una suite no ejecutada o una revisión parcial deben figurar como
límites. Si no hay hallazgos, informar igualmente qué se revisó y hasta dónde llega la evidencia.
