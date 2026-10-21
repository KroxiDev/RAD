# 0017 · Acción nativa Sleep y documento v4

**Estado:** aceptada, 2026-10-21.

El usuario pide activar con un botón el mismo reposo que ya aplica RAD por
inactividad. Es una acción nativa, no un plugin ni una suspensión del equipo.
Precisa la base estable sin cambiar sus dueños ni añadir ajustes.

## Ejecución y presentación

`Sleep` es un paso ordinario del plan, admitido por el motor con los permisos y
snapshot habituales. Authority lo completa localmente y comparte una única
transición con el vencimiento de inactividad: cancela candidatos y el mantener
original, emite los fines pendientes y proyecta el mismo estado dormido al adapter.
No cambia brillo configurado, navegación, revisión ni ajustes; funciona con reposo
automático cero. Los demás pasos del plan conservan su orden y no se cancelan por
esta acción. Se admiten pulsar y pulsación larga, nunca mantener; CLI y «Probar»
no simulan gestos físicos ([ADR 0015](0015-gesto-del-plan-y-documento-v3.md)).

Soltar la tecla que suspendió no despierta. La siguiente presión de cualquier
tecla despierta, conserva carpeta y brillo anteriores y se consume hasta soltar:
no ejecuta la acción de esa tecla. Se reutiliza la regla de entrada existente.
Los candidatos aún no admitidos del mismo informe o tick que activó Sleep se
consumen también; los fines de mantener ya iniciados siempre se entregan.

La media luna es presentación dibujada por código, sin medios persistidos. Es el
visual predeterminado de una tecla con Sleep cuando no tiene imagen personalizada
ni visual de plugin; un texto propio se conserva. La API del editor lo comunica
con `native_icon: "sleep"` opcional, omitido en las apariencias anteriores. Los
clientes v1 anteriores siguen operando las acciones anteriores; para mostrar esa
nueva apariencia, un lector cerrado anterior necesita actualizarse junto al
runtime. No cambia la versión del framing, CLI ni contrato de plugins.

## Persistencia y compatibilidad

La variante nueva `"sleep"` exige documento **v4**: un lector v3 no la entiende y
debe detectar una versión posterior, no tratar el documento como corrupto. Se
conservan los importadores cerrados de v1/v2 y se añade uno cerrado de v3 con su
[golden real existente](../fixtures/v3/README.md). Ninguna etiqueta anterior acepta
la acción nueva. El escritor v4 solo cambia la versión en perfiles sin Sleep.

Store SQLite y RADPKG mantienen envelope v2: sus estructuras no cambian. Abrir un
Store con documentos v3 preserva primero `.document-v3-backup` sin sobrescribir
evidencia; convierte las revisiones válidas atómicamente y conserva la evidencia
corrupta y su retención. Con documentos v1/v2 se mantiene `.document-v2-backup` y
la migración de Store v1 conserva `.v1-backup`. Una versión futura exige actualizar
RAD, sin downgrade ni sustitución por defaults (ADR 0004 y 0011).
