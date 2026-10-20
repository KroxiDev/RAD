# 0014 · El residual de un plugin vencido pasa al Host

**Estado:** aceptada, 2026-10-20.

Al vencer el plazo de un trabajo de plugin, el Host entrega un fallo y conserva el
residual, pero el runtime lo convierte en `Completion::Failed` y Authority retira
`jobs` y `plugin_jobs`, lo que incumple la precondición de `Engine::complete`.
Corregirlo al pie de la letra, reteniendo el trabajo en Engine hasta la terminación
real, empeoraría el aislamiento: dos plugins colgados, con hasta 16 residuales cada
uno, ocuparían casi todos los 32 trabajos de Engine y bloquearían toda ejecución,
incluidas las del lanzador y la navegación. Se formaliza la transferencia:

- al vencer el plazo, la ejecución termina para Engine y el resultado TimedOut se
  muestra una sola vez, con `FailureCode::TimedOut`;
- el residual pasa a ser del Host, que ya lo limita por plugin y en total, conserva
  las credenciales y bloquea la parada normal hasta la respuesta tardía o el reap;
- la interface de `Engine::complete` documenta esa transferencia para los adapters
  dueños de su residual;
- si el trabajo despachado todavía necesita su revisión, el Host retiene la raíz; si
  no, se libera. Se confirma al diseñar el cambio.
