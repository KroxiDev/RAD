# Aislar dependencias en tests

Un doble de prueba reemplaza una dependencia por una versión controlada. Elegirlo por el contrato
que se está probando y el efecto que debe aislarse.

| Dependencia | Criterio |
| --- | --- |
| APIs externas, correo o mensajería | Simular el transporte o adaptador para evitar efectos reales y costos. |
| Base de datos | Usar una base de pruebas aislada para comprobar consultas, transacciones o esquema. |
| Archivos, configuración y datos del usuario | Usar rutas temporales aisladas; nunca las reales. |
| Tiempo y aleatoriedad | Inyectar valores o proveedores controlados. |
| Lógica interna determinista | Preferir la implementación real para conservar sus interacciones. |

Un doble de persistencia puede servir para probar reglas de negocio, pero no demuestra
compatibilidad con la base real ni escritura atómica. Para esos contratos, usar dependencias
reales bajo control en una prueba de integración autorizada y aislada.

## Inyectar el límite externo

Pasar la dependencia permite controlarla sin reemplazar detalles internos de la función.
Si el contrato es enviar un mensaje a un destino, inyectar un canal falso y comprobar el destino
y contenido enviados aporta evidencia. Contar llamadas a colaboradores internos por su
estructura actual suele acoplar el test a la implementación.

## Mantener dobles fiables

- Simular respuestas y errores compatibles con la dependencia; usar su interfaz como contrato.
- Preferir operaciones explícitas y configurar solo lo necesario para el caso.
- Restaurar parches y aislar estado entre pruebas.
- Simular fallos en el límite externo cuando el contrato requiera comprobar cómo se manejan.
- Usar dependencias reales bajo control cuando el doble oculte la propiedad que se quiere validar.
