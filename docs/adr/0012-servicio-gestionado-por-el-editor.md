# 0012 · Servicio gestionado por el editor

**Estado:** aceptada, 2026-10-20.

Hoy usar RAD exige lanzar el runtime desde una terminal con `--xl-exclusive`, y la
unidad systemd se instala a mano; el usuario nunca debería usar la terminal. La forma
soportada de ejecutar el runtime es un servicio systemd de usuario, gestionado por el
editor con tres operaciones internas y un solo interruptor visible:

- **instalar:** el editor instala la unidad cuando la necesita y la reescribe si una
  versión nueva de RAD la cambió;
- **arrancar:** si el runtime no está activo, el editor arranca la unidad, haya o no
  autoinicio;
- **autoinicio:** «Iniciar RAD con el equipo» es un ajuste de la máquina que solo
  habilita o deshabilita la unidad. Se activa junto con «Usar el Stream Deck con
  RAD» ([ADR 0006](0006-dispositivo-por-modelo.md)) y se puede desactivar.

Separar instalar de habilitar evita que, sin autoinicio, no haya unidad que arrancar.
Al cerrar el editor, el runtime sigue y el deck funciona.

## Migración del autoinicio y fallos parciales

Cambiar la sección `[Install]` de una unidad deja en su sitio los enlaces de
habilitación existentes. Para pasar del arranque temprano a la sesión gráfica,
el editor registra primero si había un enlace en `default.target.wants`. Conserva
esa intención entre arranques, porque `systemctl reenable` puede haber retirado
el enlace antiguo antes de fallar. La migración queda completa cuando el enlace
de `graphical-session.target.wants` existe y el antiguo ha desaparecido. Una
petición explícita de deshabilitar el autoinicio tiene prioridad. Si la migración
falla, el usuario recibe el aviso y puede seguir arrancando la unidad instalada.
[Gestión del servicio](../../crates/rad-editor/src/service.rs).

La publicación del ajuste de autoinicio sigue a la operación de systemd. Un
fallo puede dejar un enlace cambiado, incluso si el comando devolvió error.
El editor relee el ajuste observado en el runtime antes de corregir systemd.
Un cambio limitado al uso del deck solo necesita reconciliar el estado del
runtime. Si una unidad ya entró en fallo tras `start`, el editor lo informa,
aunque el comando haya devuelto éxito. [Ajustes de máquina](../../crates/rad-editor/src/app/machine.rs)
y [pruebas de servicio](../../crates/rad-editor/tests/service.rs).
