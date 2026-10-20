# Revisión de código

Aplicar al evaluar cambios contra estándares y requisitos. Complementa el rol
[Revisor](../agents/revisor.md): el revisor actual evalúa ambos ejes sobre el resultado,
independientemente del método usado para implementarlo.

## Procedimiento

1. Reunir el brief o especificación autorizada, criterios de aceptación y estándares aplicables.
   Reutilizar las fuentes y la base ya acordadas; aclarar solo ambigüedades que cambien la revisión.
2. Fijar archivos, base y estado exacto revisado. Para elegir la comparación y cubrir commits,
   cambios staged, locales y nuevos, consultar [alcance y estado](references/code-review.md#alcance-y-estado).
   Un diff vacío requiere comprobar su cobertura antes de concluir que no hay cambios.
3. **Estándares:** contrastar el cambio con las reglas documentadas del repositorio. Consultar
   [heurísticas](references/code-review.md#heurísticas-de-diseño) cuando haya indicios de problemas
   de diseño; justificar su consecuencia concreta y dar prioridad a los estándares locales.
4. **Especificación:** comprobar cada requisito y buscar omisiones, ampliaciones de alcance e
   implementaciones incorrectas. Consultar [trazabilidad](references/code-review.md#especificación).
   Si falta una especificación suficiente, explicitar qué requisitos quedan sin verificar.
5. Ejecutar las comprobaciones autorizadas y comprobar que la evidencia corresponde al estado
   revisado. Mantener cualquier propuesta de corrección como hallazgo dentro del alcance del rol.
6. Entregar secciones **Estándares** y **Especificación**, con los veredictos definidos por el rol.
   Cada hallazgo cita ubicación, regla o requisito y consecuencia; distinguir mejoras opcionales.
   Declarar estado, cobertura y límites reales, incluso cuando no se encuentren problemas.
