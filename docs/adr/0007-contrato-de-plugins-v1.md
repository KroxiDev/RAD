# 0007 · Contrato de plugins v1: capacidades y una sola definición

**Estado:** aceptada, 2026-10-20.

Los plugins de terceros son un objetivo, y lo que no quepa en el contrato obligará a
tocar la base estable. Hoy la versión de API es lineal (cada ampliación crea otra
versión y suma condicionales en host y SDK), los tipos están definidos dos veces y
ya difieren, y faltan eventos de soltar, ajustes, diagnóstico y una forma de ampliar
los tipos de campo. El contrato v1 define:

- **Versiones:** una versión mayor más capacidades intercambiadas en `hello` y
  `ready`. Lo aditivo entra como capacidad sin subir la mayor, y un campo o mensaje
  nuevo solo se envía si ambos lados lo declararon; el parseo sigue siendo estricto.
  El host acepta la mayor actual y la anterior. Los plugins oficiales 0.1.x se
  republican una vez en v1.
- **Una sola definición:** host y SDK usan el mismo crate de contrato, con vectores
  JSON de referencia probados en ambos lados. El código del SDK sigue en este repo,
  junto al host, para que ambos cambien a la vez; los autores lo obtienen con una
  etiqueta fija.
- **Controles y gestos:** cada acción declara qué gestos acepta: pulsar, mantener
  (con inicio y fin) o pulsación larga. El umbral global aplica solo a «pulsar». El
  evento lleva un `control` genérico ([ADR 0006](0006-dispositivo-por-modelo.md)).
- **Ajustes:** un esquema `settings` en el manifest, con el mismo sistema de campos
  ([ADR 0005](0005-tres-tipos-de-ajustes.md)).
- **Tipos de campo ampliables:** cada tipo nuevo es una capacidad; en v1 entran solo
  los que pida un plugin concreto.
- **Diagnóstico:** el host captura el stderr de cada plugin, acotado en tamaño de
  línea y ritmo, y lo registra con su id en journald y en «Avanzado → Registro».
- **Peticiones del plugin al host:** ninguna en v1; el espacio de nombres queda
  reservado.
- **Salida bajo presión:** el SDK escribe sin bloquear, con presupuesto y cierre
  terminal.
- **Frescura de los visuales:** [ADR 0008](0008-frescura-declarada.md). **Confianza
  y estándar de compatibilidad:** [ADR 0009](0009-confianza-en-plugins.md).

La especificación y la guía para autores viven en `KroxiDev/rad-plugins`, junto a
las versiones de los plugins oficiales; declaran qué versión del contrato describen
y se actualizan en la misma entrega que la publica. La parte verificable (tipos y
vectores) sigue con el código en este repo. Plataforma: Linux x86_64 nativo por
ahora; el campo `targets` deja la puerta abierta.
