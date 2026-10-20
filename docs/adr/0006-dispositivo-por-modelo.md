# 0006 · Dispositivo descrito por modelo, sin tener en cuenta otros programas

**Estado:** aceptada, 2026-10-20.

Hoy solo existe el Stream Deck XL; el camino a otros modelos queda abierto sin
implementarlos:

- el deck se encuentra por VID/PID (y número de serie) en `/sys/class/hidraw`, en
  cualquier puerto, conservando las comprobaciones de interfaz, driver, char-device,
  ioctl y lock. Hoy está fijado al puerto USB `1-5`;
- el modelo (rejilla, tamaño de tecla, formato de imagen y controles) lo describe el
  adapter de `rad-deck`. El formato guarda la rejilla y el dominio agrupa las
  constantes del XL en un tipo `Grid`; los arrays fijos se mantienen mientras solo
  exista el XL;
- el contrato de plugins modela eventos de «control» genéricos, aunque hoy solo haya
  teclas. Se controla un deck a la vez.

RAD no detecta otros programas que usen el deck ni los tiene en cuenta al usarse o
iniciarse: un conflicto con otra aplicación es responsabilidad del usuario. El
consentimiento para controlar el dispositivo es un ajuste de la máquina, «Usar el
Stream Deck con RAD», que reemplaza al argumento `--xl-exclusive`. RAD nunca lee,
importa ni modifica datos de otras aplicaciones.
