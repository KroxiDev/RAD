# 0002 · El límite de identidades se aplica a lo vigente

**Estado:** aceptada, 2026-10-20.

Un ShortId (`f1`, `b12`, `a7`) nunca vuelve a nombrar otra entidad, porque los
scripts de `rad-cli` dependen de ello. Hoy esa garantía se consigue guardando todas
las identidades creadas alguna vez, con un tope de 10.000 que cuenta también lo
borrado, lo deshecho y lo reemplazado. Al agotarse, ningún guardado que cree algo
vuelve a funcionar, y cada guardado recorre el registro completo.

Se decidió:

- el límite lógico se aplica a las entidades vigentes; su valor se fija tras medir
  cuántas caben en el documento de 1 MiB, con 10.000 como máximo, y se valida al
  preparar la edición;
- la no reutilización se garantiza con contadores monotónicos persistentes por clase
  y por serial, que nunca retroceden: tampoco con deshacer, recuperación ni
  restauración;
- las identidades retiradas se conservan solo mientras algo pueda traerlas de vuelta
  (historial, revisiones retenidas, exportaciones en curso); después se podan;
- restaurar en la misma instalación puede reactivar un ShortId retirado si sus
  números no superan los contadores y no choca con una entidad vigente.

**Consecuencias:** el guardado y la memoria pasan a ser proporcionales a lo vigente
más el historial reciente, y desaparece el techo permanente. Podar sin contadores
persistentes rompería la garantía.
