# 0001 · Un runtime único como autoridad y un proceso por plugin

**Estado:** aceptada, 2026-10-20.

RAD conserva su arquitectura de procesos. Un runtime sin GUI es la única autoridad
de configuración, ejecución, navegación y dispositivo; el editor y `rad-cli` son
clientes del socket local; cada plugin activo corre en su propio proceso, arrancado
por demanda; los datos viven en SQLite con publicación transaccional.

Se evaluaron reescribir, fusionar crates e introducir frameworks, y se descartaron:
la propiedad del estado es clara, el dominio es puro y testeable, y un proceso por
plugin contiene caídas y errores de protocolo sin necesidad de un sandbox. Lo que
debe prepararse para crecer son los bordes (contrato de plugins, API de clientes,
formatos y modelo de dispositivo), no el núcleo.

**Consecuencias:** el deck funciona con el editor cerrado. La UI y los plugins
crecen en sus capas, y la base estable cambia solo por decisión registrada.

En la admisión de un plan, Engine decide el orden de los rechazos por permiso o por botón
ocupado. Si falla la preparación de un plugin, Authority pasa un conjunto vacío de ejecutores
al motor. Así conserva el mismo orden de errores y evita publicar una intención antes de
validar el plan completo. El motor comprueba que el reloj puede representar todos los plazos
de respuesta y espera antes de emitir el primer paso. Las esperas se completan con ese reloj y
se pueden retirar sin dejar trabajo externo pendiente.
