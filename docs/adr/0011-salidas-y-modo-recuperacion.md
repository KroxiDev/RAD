# 0011 · Clases de salida, reinicio sin bucles y modo recuperación

**Estado:** aceptada, 2026-10-20.

Hoy cualquier error de `server.turn()` termina el runtime y la unidad de ejemplo no
lo reinicia. Se quiere reinicio automático salvo condición bloqueante, sin bucles.
Cada salida tiene una clase:

| Clase | Ejemplos | Comportamiento |
|---|---|---|
| Limpia | SIGTERM o SIGINT atendidos | Sale con 0; systemd no reinicia |
| Bloqueante | Solo lo que impide atender peticiones: otra instancia activa o un socket inutilizable | Código reservado; `RestartPreventExitStatus=` impide el reinicio; el editor lee el estado de la unidad |
| Recuperación | Configuración sin ninguna revisión válida, formato de una versión más nueva, carpeta de datos insegura | No sale: modo recuperación |
| Inesperada | Panic, invariante interna rota, worker muerto | `Restart=on-failure` con espera creciente y tope de 5 en 5 minutos; después, parar y avisar |
| Contenida | Deck ausente o inaccesible, fallo de un plugin, almacenamiento incierto reconciliable, descarga fallida | No sale; el estado se muestra en lenguaje llano |

Los errores ligados a un plugin o a un trabajo se contienen en su subsistema; solo
escala lo que compromete la autoridad o los datos. Ningún reinicio repite acciones.

**Modo recuperación.** Cuando no puede abrir la configuración, el runtime conserva el
lock y el socket, no toma el deck ni lanza plugins, y solo admite consultar el
estado, «Restaurar una copia», «Empezar de nuevo» y «Ver detalles». Así la única
autoridad sigue siendo el runtime y el editor nunca escribe la configuración por su
cuenta. «Empezar de nuevo» existe porque, sin una copia, no habría salida.

- El archivo dañado nunca se borra: queda como evidencia, con el mismo criterio que
  el hold de recuperación del Store.
- Restaurar conserva los IDs de la copia. Restaurar y empezar de nuevo respetan los
  contadores del archivo dañado si se pueden leer; si no, los scripts antiguos
  podrían apuntar a IDs reasignados (límite declarado).
- Después, el runtime pasa a modo normal en el mismo proceso, sin reiniciar.

El Store ya vuelve solo a la última revisión válida y retiene la dañada, de modo que
este modo solo aparece cuando no queda ninguna.

**Consecuencias:** el editor recibe el motivo por la API de siempre, sin leer códigos
de salida ni el journal; el último motivo de salida queda en «Ver detalles» tras
reiniciar. El registro va a journald, más un búfer acotado de eventos recientes para
«Avanzado → Registro».

Una lectura después de un retorno incierto de SQLite no demuestra durabilidad. Al reconciliar,
el Store verifica que la cabecera, el registro y los contadores corresponden al estado
anterior o al candidato. Después cambia el valor de una barrera en una transacción real: una
escritura idéntica podría omitir la sincronización. Al reabrir se comprueba la integridad y se
cambia la barrera antes de continuar.
[Reconciliación](../../crates/rad-store/src/reconciliation.rs).

Para rescatar contadores de un archivo dañado se abre como inmutable, evitando que SQLite
reproduzca un journal o modifique evidencia. Se toma el mayor valor legible de cada fuente:
una reserva demasiado alta deja un hueco admisible. Al reemplazar la base se separan primero
sus sidecars para que un journal caliente no se reproduzca sobre la nueva, y se intercambian
atómicamente los archivos. Los fallos posteriores conservan la evidencia. [Lectura de
contadores](../../crates/rad-store/src/peek.rs) y
[reemplazo](../../crates/rad-store/src/replace.rs).
