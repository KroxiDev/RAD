# 0009 · Confianza en plugins y estándar de compatibilidad

**Estado:** aceptada, 2026-10-20.

Se puede instalar cualquier plugin que cumpla el estándar de compatibilidad, bajo
responsabilidad del usuario y sin una capa de seguridad pesada. Los plugins son
binarios nativos del mismo usuario: no hay sandbox ni firma, y RADP valida tamaños,
framing y estructura ELF, no la autoría.

- **Estándar comprobable.** Lo forman la especificación, los vectores y el arnés
  `rad-plugin-check`, que ejecuta un `.radp` contra el host real. Lo obligatorio es
  exactamente lo que verifica el arnés; lo demás es opcional o recomendado. Los
  plugins oficiales pasan el mismo arnés.
- **Contención.** Frente a fallos y abusos dentro del protocolo (caídas, cuelgues,
  floods, mensajes inválidos, salida bloqueada), un plugin no detiene el runtime ni
  supera sus cuotas, y nada de lo que lance sobrevive al runtime
  ([ADR 0013](0013-limpieza-de-procesos.md)). Los plugins arrancan con un
  `oom_score_adj` más alto que el runtime, para que ante falta de memoria el kernel
  mate antes al plugin.
- **Lo que no se garantiza.** Un binario hostil puede actuar fuera del protocolo:
  matar procesos, consumir CPU o memoria o tocar archivos del usuario. Esa confianza
  la decide el usuario al instalar.
- **Instalar.** La confirmación dice qué se instala y de dónde, con la advertencia de
  instalar solo plugins de fuentes de confianza. Los recursos declarados (una lista
  opcional en el manifest: internet, programas del sistema, archivos fuera de su
  carpeta, dispositivos y servicios del escritorio) son informativos, aparecen en
  «Ver detalles» y no se hacen cumplir.
- **Origen.** Una actualización desde otro origen exige confirmación y muestra ambos.

Fuera de alcance, salvo pedido del usuario: sandbox, firma de paquetes, catálogo
central de terceros y ejecución de scripts no nativos.

## Contenido del estándar

**Obligatorio** (lo comprueba el arnés):

- manifest válido: id, versión, `targets`, contrato y capacidades;
- handshake e identidad coherentes entre manifest y `ready`;
- acciones, opciones, visuales y suscripciones según el manifest, con respuestas
  dentro de su timeout;
- estados visuales con validez declarada y renovada: una fuente sana sin cambios sigue
  vigente ([ADR 0008](0008-frescura-declarada.md));
- ningún secreto en stderr, visuales ni mensajes: el arnés inyecta una credencial
  conocida y la busca;
- salida no bloqueante con presupuesto, sin frames posteriores a uno truncado;
- terminar al recibir EOF sin dejar hijos vivos.

**Opcional:** los recursos declarados en el manifest.

**Recomendado** (guía para autores):

- ids con la convención `io.github.<owner>.<nombre>`;
- constructor sin I/O externo, y recursos solo por demanda, compartidos por fuente y
  liberados con el último consumidor;
- el trabajo largo, fuera del hilo de despacho;
- `stale` o `unavailable` cuando la fuente falla, y resultados tardíos que no cruzan
  epochs;
- textos visibles breves y en lenguaje llano;
- reconexión sin replay y sin sustituir en silencio una selección ausente;
- credenciales fuera de parámetros y logs, con limpieza antes del ACK o del release;
- máximo de procesos, hilos y colas explícito, con el estado idle y la última baja
  observables;
- `--licenses` sin conexiones, y fixtures que solo alcanzan servicios de prueba.

El diagnóstico del host busca las credenciales activas incluso en bytes descartados por cuota,
entre dos lecturas y en fragmentos de una credencial multilínea. Examina la línea original
para reconocer coincidencias superpuestas y retiene el indicador antes de coalescer estados.

La validación estructural del ELF comprueba que la entrada apunta a bytes presentes en un
segmento ejecutable. La memoria que el cargador completaría con ceros no contiene
instrucciones suministradas por el paquete. Las descargas resuelven DNS en el único worker
acotado, aceptan sólo direcciones públicas admitidas y limitan la espera de cada lectura antes
de TLS. Así, perder el plazo de la operación tampoco crea un segundo resolver mientras el
primero sigue bloqueado. Reinstalar la versión seleccionada conserva la versión anterior que
sigue sirviendo para rollback.

Una operación administrativa sólo puede reconciliar su propio journal. Cuando se confirma el
selector, Authority retira los tipos anteriores y mantiene la barrera mientras completa el
cambio. El administrador usa un descriptor del directorio privado para operar sobre los
archivos de la instalación. Incluso al enumerarlo por `/proc/self/fd`, sigue recorriendo el
directorio abierto aunque cambie el nombre externo.

El host envía cada línea apta a journald mediante un datagrama no bloqueante. Si el journal no
puede recibirla, descarta esa línea para seguir atendiendo el protocolo.
