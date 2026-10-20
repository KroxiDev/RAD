# Fixture I3 privado

Crate aislado, no miembro de workspaces del producto. Path-deps al SDK y metrics
existentes (test-support), lock offline. No dependencia nueva de producción.
Nunca empaquetar/publicar como rad-plugin-metrics real.

Mismo manifiesto oficial y MetricsPlugin::with_reader, sólo cambia el closure:
- healthy: valores deterministas CPU25, RAM8/8GiB, GPU73, VRAM2/8GiB;
- missing: Sample::default, cuatro None;
- block: el mismo reader espera un canal que permanece abierto, sin crear hilos
  ni lectores de reemplazo; no se toca un driver real.

Control exacto en XDG_RUNTIME_DIR privado: f5-reader-mode (≤16 bytes leídos).
Contador de llamadas f5-reader-count mediante reemplazo atómico dentro del mismo
directorio. No proc, NVML, bus, sockets de servicios, hardware ni sensores.
El checker sólo alterna esos valores en su perfil nuevo y comprueba32 visuales,
host listo, status y una acción Wait del núcleo mientras permanece stale.
