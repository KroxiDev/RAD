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
