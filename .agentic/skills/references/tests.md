# Tests de comportamiento

## Elegir el contrato

Probar la interfaz pública del componente bajo cambio: una función, servicio, comando o
recorrido de interfaz puede ser ese límite. Elegirlo con los criterios del encargo y el diseño
existente; consultar al usuario solo cuando una decisión pendiente cambie lo esperado.

Cubrir resultados y errores relevantes para ese contrato. Una prueba debe expresar un caso
comprensible y fallar cuando se rompe; puede necesitar varias aserciones para describirlo.

## Resultados esperados independientes

El valor esperado proviene del requisito o de un ejemplo calculado independientemente, no de
repetir el algoritmo que se está probando. Por ejemplo, si el contrato es sumar precios:

```text
Frágil:    comprobar(total([10, 5]) == sumar([10, 5]))
Contrato:  comprobar(total([10, 5]) == 15)
```

El primer caso puede repetir el mismo error de la implementación; el segundo fija un resultado
conocido del requisito. Usar las herramientas y convenciones de tests del proyecto.

## Observar efectos por el contrato

Para comprobar que un registro creado puede recuperarse, crearlo mediante la operación pública
y consultarlo mediante su interfaz de lectura. Leer el archivo o consultar la base directamente
tiene sentido cuando se prueba el formato en disco, las transacciones o la escritura atómica,
no como dependencia incidental de una prueba de negocio.

## Señales de fragilidad

- Afirmar funciones privadas, estructura interna u orden de llamadas sin que sean parte del contrato.
- Cambiar el resultado esperado para acomodar la implementación en lugar de corregir el defecto.
- Depender del reloj, la red, datos reales o estado compartido sin controlarlos.
- Simular toda la colaboración interna y comprobar solo que los mocks se llamaron.

Cuando una interacción externa sea el comportamiento requerido, comprobar su destinatario y
contenido es válido. Elegir su aislamiento con [mocking](mocking.md).
