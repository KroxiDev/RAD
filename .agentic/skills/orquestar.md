# Orquestar

Coordina una tarea **ya especificada** desde el encargo hasta una entrega
comprobable. Fuente única del procedimiento; los adaptadores de cada herramienta
apuntan aquí.

## Cuándo aplica

La entrada es una especificación que ya existe: un archivo, un issue, o la
conclusión de una sesión previa de entendimiento compartido.

Una tarea ambigua no se orquesta: se aclara primero. Una tarea simple se resuelve
directo, sin roles ni run dir.

## Preparar

1. Crear `.agentic/run/<id>/plan.md`, donde `<id>` es un nombre corto de la tarea.
   La carpeta está fuera de control de versiones y se borra al cerrar.
2. Volcar en `plan.md`: objetivo, criterios de aceptación observables, alcance de
   archivos autorizado, y los pasos previstos con su dependencia.
3. `plan.md` registra **intención, nunca evidencia**. Un paso marcado como hecho no
   demuestra que se hizo: la evidencia vive en la salida de las herramientas.

## Ejecutar

1. **Investigador** — solo si el plan tiene un hueco concreto. Recibe una pregunta
   cerrada; devuelve hallazgos con fuentes.
2. **Implementador** — recibe el brief completo: objetivo, criterios, alcance de
   archivos y convenciones aplicables. Lo que no esté en el brief, no lo sabe.
3. **Revisor** — recibe el diff y los criterios de aceptación. Devuelve un
   veredicto por criterio.

Si el entorno no admite subagentes, el orquestador asume los roles en secuencia.
En ese caso no describir el resultado como revisión independiente.

## Cerrar

El orquestador **no re-revisa el código**. Si lee el diff completo, el aislamiento
de contexto que justificaba delegar desaparece. Comprueba tres señales:

- cada criterio de aceptación tiene una evaluación, no una impresión general;
- ningún `NO_VERIFICADO` fue presentado como `CUMPLE`;
- los archivos tocados están dentro del alcance autorizado.

Con las tres en orden: informa resultado, evidencia y pendientes, y borra
`.agentic/run/<id>/`. Si alguna falla, vuelve al rol que corresponda con el
hallazgo concreto, sin rehacer el trabajo que sí cumple.

## Retomar una tarea interrumpida

Un `run/` que sobrevivió a una interrupción es sospechoso, no autoritativo.
Contrastar su contenido con los artefactos actuales antes de confiar en él.
