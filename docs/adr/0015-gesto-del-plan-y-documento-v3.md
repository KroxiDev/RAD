# 0015 · Gesto del plan y documento v3

**Estado:** aceptada, 2026-10-20.

El usuario autorizó documento v3 con importador de v2 para guardar la selección
explícita del gesto; v2 no reservaba ese dato. También aprobó un gesto por
botón/plan y un umbral global de pulsación larga configurable, inicialmente 500 ms.
Esto precisa [ADR 0007](0007-contrato-de-plugins-v1.md) sin esconder configuración
del host dentro de los parámetros opacos de un plugin.

## Selección y ejecución

- Cada botón selecciona `press`, `hold` o `long_press`. Las acciones de un plan
  comparten ese gesto; mantienen su orden y modo secuencial/paralelo.
- `press` conserva el umbral de pulsación existente. `hold` comienza con la presión
  física y termina al soltar, sin ese umbral. `long_press` usa exclusivamente el
  umbral largo, capturado al presionar, y se emite una vez al alcanzarlo.
- Mantener solo se admite para planes formados por acciones externas que declaren
  ese gesto. No se repiten lanzadores, esperas ni navegación al soltar. Las opciones
  ofrecidas por el editor respetan la intersección de capacidades de las acciones.
- Soltar antes de terminar un plan cancela los inicios todavía no enviados. El fin
  se entrega exclusivamente a las acciones cuyo inicio se envió, con los parámetros,
  ajustes, credenciales, selección y generación originales. Nunca vuelve a resolver
  la tecla contra la carpeta actual.
- Navegación, cambio de asignación, reposo y desconexión terminan el mantener
  original. Los eventos consumidos para despertar no crean inicios.
- CLI y «Probar» conservan su ejecución ordinaria; no simulan un gesto físico ni
  esperan el umbral largo.

## Dueños y límites

El dominio transforma informes físicos y reloj explícito en candidatos, y detiene
pasos pendientes al soltar. La autoridad admite con la configuración actual y
conserva las reservas del snapshot hasta completar el fin. El adapter distingue
inicios realmente enviados de los que aún esperaban proceso o credenciales.

Host y SDK reservan capacidad de fin separada de los trabajos ordinarios: un inicio
no se envía si no puede reservarse su fin. Un ticket interno identifica la captura
original; no autoriza un inicio nuevo ni puede cambiar de proceso. El fin mantiene
respuesta y plazo acotados. Caída, presión de salida o expiración cierran el intento;
no hay reintentos ni replay hacia una generación posterior. El contrato de control
sigue siendo único entre host y SDK. Los planes pueden repetir el mismo tipo de
acción: los pares idénticos usan correspondencia FIFO acotada, no una prohibición
de duplicar acciones.

Mientras un Hold o su fin estén pendientes, la autoridad retiene el resultado
original fuera de la caducidad ordinaria y de su expulsión por capacidad. Las
retenciones físicas están acotadas por el número de teclas. Un plan cuyos inicios
terminaron correctamente se proyecta `Running` hasta cerrar todos sus fines; un
fallo de fin cambia el resultado original a `Failed` y publica revisión de
execution, sin reabrir pasos. Un fallo anterior nunca se convierte en éxito por
un fin correcto. Tras liberar la retención comienza la caducidad ordinaria.

## Persistencia

Solo cambia la versión del documento. Store SQLite y RADPKG mantienen su envelope
v2 porque sus estructuras no cambian. Los importadores de v1 y v2 se conservan y
asignan `press` y 500 ms. El escritor v3 omite esos dos valores por defecto para
no aumentar el tamaño de documentos históricos en el límite de 1 MiB. Los campos
nuevos no se aceptan bajo una etiqueta v2.

Abrir un Store v2 con documentos antiguos conserva primero el archivo original
como `.document-v2-backup`, sin sobrescribir evidencia, y convierte las revisiones
válidas en una transacción. La migración de Store v1 conserva su `.v1-backup` y
avanza directamente al documento actual. Registro, máximos, medios, historial,
ajustes y dependencias conservan sus dueños y contratos. No hay downgrade y una
versión futura exige actualizar RAD, nunca volver a valores iniciales.
