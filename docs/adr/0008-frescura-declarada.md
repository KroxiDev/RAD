# 0008 · Frescura declarada y renovada por quien observa la fuente

**Estado:** aceptada, 2026-10-20.

Que el proceso de un plugin responda no demuestra que sus datos estén frescos. Hoy
el runtime fija un TTL de 1.000 ms y el SDK repite el estado completo cada 400 ms,
imágenes incluidas: unos 2,5 mensajes por segundo por visual aunque nada cambie, y un
lector bloqueado sigue mostrando un valor viejo como vigente (B8 en el
[plan original](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/roadmap.md)).

- Cada estado visual declara su validez (`valid_for_ms`), acotada por el host (por
  ejemplo, de 1 a 60 s). El host lo marca stale al vencer, con su propio reloj.
- La renovación la emite el mismo camino que publicaría un cambio, es decir, el bucle
  que observa la fuente; nunca un temporizador aparte. Repetir sin observar es lo que
  oculta B8.
- En el SDK, `publish(estado)` envía el estado si cambió y, si no, una renovación
  liviana, sin imagen.

Ejemplo: multimedia solo publica cuando el estado cambia y relee cada 10 s. Con unos
25 s de validez, una canción en pausa sigue vigente y un lector bloqueado pasa a
stale en menos de 30 s.

**Consecuencias:** B8 se da por resuelto solo con tres pruebas: fuente sana sin
cambios, lector bloqueado y renovación sin carga.
