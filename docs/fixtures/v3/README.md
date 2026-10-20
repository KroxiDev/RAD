# Fixture canónico v3

`profile.json` se generó con el escritor v3 usando el golden v2 real como origen,
selección `long_press` en el botón y umbral largo de 750 ms. Conserva los ajustes
opacos y la dependencia de plugin originales. `generate.rs.txt` conserva el
generador (ejecutado como test temporal de `rad-codec`); no se regenera en la suite.
Los golden v2 originales siguen intactos en `../v2`, presentes en la base `e86a86e`.

SHA-256 `profile.json`:
`23bfdc2e887e40cf39ad3c155c1279fe31a83198ae26a469e162009f229b003d`.
