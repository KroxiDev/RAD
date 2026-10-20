# Usar CodeGraph

Antes de localizar o comprender código, seguir la documentación de CodeGraph disponible
en el entorno. Esta skill no instala ni configura la integración.

1. Si existe `.codegraph/` y la herramienta está disponible, consultar `codegraph_explore`
   (MCP) o `codegraph explore "<consulta>"` antes de buscar o leer código. Limitar la consulta
   al proyecto y alcance autorizados; indicar archivos o símbolos concretos.
2. Aprovechar las fuentes y relaciones devueltas; contrastar el detalle relevante con el código
   actual y leer directamente lo omitido o desactualizado. Respetar las rutas protegidas y
   secretos también en los resultados del índice.
3. Sin índice o herramienta disponible, o si la consulta falla, usar `rg` y lectura directa e
   informar la limitación. Crear o reconstruir el índice requiere autorización explícita.
