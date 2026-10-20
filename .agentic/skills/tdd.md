# TDD

Aplicar a cambios de comportamiento ejecutable, siguiendo la política de
[ciclo de vida de los tests](../../AGENTS.md#ciclo-de-vida-de-los-tests).

## Ciclo

1. Elegir un comportamiento observable a partir del encargo y sus contratos públicos, dentro del
   alcance autorizado. Preguntar solo si una ambigüedad cambia qué debe probarse.
2. **Rojo:** escribir una prueba y ejecutarla antes de cambiar producción. Comprobar que falla por
   el comportamiento ausente o incorrecto; un error del test o del entorno no demuestra ese fallo.
3. **Verde:** implementar lo mínimo para que pase y ejecutar la prueba y los tests relacionados.
4. **Refactorizar:** mejorar claridad dentro del alcance, conservando el comportamiento y
   comprobando que las pruebas siguen verdes.
5. Completar cada ciclo antes de añadir otra prueba. Al cerrar, validar con las pruebas
   temporales, retirarlas según esa política y repetir la validación con la suite definitiva.

Si no se puede ejecutar una prueba o validación, informar el impedimento y qué quedó sin
verificar; una prueba escrita pero no ejecutada no demuestra un ciclo rojo–verde.

## Calidad del test

- Comprobar el contrato público: el test debe sobrevivir cambios internos que lo respeten.
- Obtener el resultado esperado de requisitos o ejemplos independientes de la implementación.
- Controlar efectos externos para que las pruebas sean repetibles y estén aisladas.

## Referencias

- Al diseñar casos y aserciones, consultar [tests](references/tests.md).
- Al elegir dobles de prueba y dependencias reales, consultar [mocking](references/mocking.md).
