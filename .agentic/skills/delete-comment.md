# Revisar y retirar comentarios

Aplica el trinquete de [pruebas](../../docs/testing.md#comentarios) al repositorio
o a la ruta solicitada. Trabaja desde la raíz, guarda la revisión base y respeta
los cambios que ya estén en el checkout.

1. Ejecuta `python3 -B scripts/comment_lint.py --inventory`. Para obtener las
   ubicaciones originales de un commit añade `--revision <commit>`. Selecciona
   las rutas del encargo. Agrupa líneas contiguas sólo cuando explican la misma
   idea.
2. Lee cada grupo junto al código, sus consumidores y los ADR pertinentes.
   Registra ruta, líneas originales, categoría, acción y motivo. Para una
   **tautología**, comprueba dónde se ve la misma información en el código y
   bórrala. Para un comentario **obsoleto**, contrasta la afirmación con la
   implementación vigente y bórralo.
3. Para una **excusa**, identifica la deuda concreta y el comportamiento que
   permitiría verificar su corrección. Busca la incidencia correspondiente.
   Cuando el encargo autorice publicarla, abre la que falte y registra su número
   antes de retirar el comentario.
4. Para una **decisión**, identifica qué se perdería al borrar el comentario.
   Comprueba si el ADR de ese tema recoge tanto la decisión como el motivo.
   Completa ese ADR o registra uno nuevo según el [estándar documental](../../docs/documentation.md).
   Conserva por ahora cualquier justificación de seguridad que deba leerse junto
   a un bloque `unsafe`, documentación pública necesaria o ejemplo ejecutable
   que requiera una migración de su contrato. Identifica cada aplazamiento.
5. Ejecuta el lint y la validación de [pruebas](../../docs/testing.md). Verifica
   que las ediciones conservaron literales, directivas y comportamiento. Compara
   ambos inventarios por ubicación. Informa líneas exclusivas retiradas y
   pendientes, comentarios en líneas con código, ADR e incidencias.
