# Usar CodeGraph

CodeGraph responde preguntas de relación: cómo llega un flujo de un punto a otro,
quién llama a un símbolo, qué afecta cambiarlo o qué contiene un área desconocida.
Esta skill no instala ni configura la integración; crear o reconstruir el índice
requiere autorización explícita.

1. Elegir la consulta más angosta que responda la pregunta, dentro del proyecto y
   alcance autorizados. La salida crece con cada término: un símbolo devuelve unos
   4K caracteres y una lista de siete términos, unos 25K.
   - Archivo o línea ya ubicados (por la tarea, el diff o un resultado anterior):
     leer ese tramo directo con `rg` o `sed -n`.
   - Un símbolo y sus relaciones: `codegraph_node`, `codegraph_callers` o
     `codegraph_impact` (MCP), o `codegraph node|callers|impact <símbolo>` en la
     terminal. Devuelven cientos de caracteres.
   - Un flujo o un área desconocida: `codegraph_explore` (MCP) o
     `codegraph explore`, con una sola pregunta, de uno a tres símbolos y
     `maxFiles` bajo.
2. Tratar lo devuelto como ya leído: contrastar con el código actual solo el
   detalle que importa y leer directo lo omitido o desactualizado. Si falta algo,
   la consulta siguiente nombra el símbolo concreto que falta. Respetar las rutas
   protegidas y secretos también en los resultados del índice.
3. Sin `.codegraph/`, sin la herramienta o si la consulta falla, usar `rg` y
   lectura directa e informar la limitación.
