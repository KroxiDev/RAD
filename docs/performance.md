# Medición de RAD · línea base sintética

## Alcance

Primera entrega autónoma de la fase 4. No modifica el comportamiento del producto,
los contratos, los límites ni la UX. No certifica hardware ni el escritorio real.
La fase UX queda para después por decisión del usuario.

La [continuación autónoma](performance-extended.md) amplía esta línea base con
oficiales sintéticos, atribución de memoria, fallos, medios grandes y builds.
Las tablas de esta primera entrega permanecen históricas, sin mezclar corridas.
El [ensayo físico posterior](performance-physical.md) usa un XL real con autorización
independiente y documenta el límite reporte consumido → proyección, no píxeles.
La [continuación con escritorio real](performance-desktop.md) mide editor/XL y
métricas con GPU reales, además de cargas y fallos integrados adicionales.
La [siguiente continuación](performance-services.md) amplía los fallos compuestos
y documenta contadores del kernel y audio/multimedia de solo lectura, con parada
explícita antes de OBS real. Cada informe mantiene sus límites y resultados propios.

El arnés usa exclusivamente procesos nuevos propios y datos privados nuevos bajo
`target/performance/`. No abre el Stream Deck ni ventanas, no usa systemd personal,
no consulta sensores, audio, OBS o buses de sesión reales, ni descarga dependencias.
Los plugins del escenario runtime son ecos sintéticos inertes, **no los oficiales**.

## Reproducción

Desde la raíz, con las dependencias ya disponibles en la caché:

```bash
bash scripts/performance/run.sh --seconds 15 --repetitions 3
```

`--seconds` admite 3–20 s por fase; `--repetitions`, 1–10; `--interval`,
0,05–1 s. El lease sintético de 60 s cubre las fases y el calentamiento previstos.

Requiere Linux con `/proc` legible y pidfd para procesos propios, Python estándar, Cargo y
sockets Unix privados permitidos. No instala herramientas. Compila antes de medir,
en release, offline, locked, all-features, jobs=2 e incremental=0, con el target en
el disco del repo. Se usa el perfil release actual, sin añadir strip, LTO ni flags.
No ejecutar builds u otros benchmarks a la vez. La carga del resto de la máquina
no está controlada: los números son una observación local, no una garantía universal.

La salida se conserva ignorada en un directorio nuevo de `target/performance/`:

- `metadata.json`: plataforma sin hostname, parámetros, commit base y SHA-256 de los
  cuatro archivos instrumentales. Los hashes identifican el arnés aunque aún no
  tenga commit. No se capturan variables de entorno personales.
- `r*-*/events.jsonl`: muestras de latencia, fases, iteraciones y turnos.
- `r*-*/samples.jsonl`: recursos por PID, identidad de arranque, hilo y fase.
- `resources.json` y `runtime-timings.json`: resúmenes por escenario/repetición.
- `domain.jsonl` y `domain-timings.json`: muestras de dominio/persistencia.
- Logs y directorios de trabajo privados: conservados para auditar el resultado;
  no se versionan ni se mezclan con la configuración del usuario.

Una salida fallida no es una medición válida: revisar stderr y código de salida.
Si un sandbox impide sockets o namespaces, usar permisos autorizados o informar
NO_VERIFICADO; no sustituir resultados ni tocar el sistema personal.

## Método

### Runtime, helper y demanda

`runtime_profile` compone el `Server` y helper reales mediante sus APIs públicas,
sin adapter de hardware. Instala metadata de 0, 1 o 4 plugins sintéticos locales.
Confirma ausencia inicial, mide la primera acción de cada plugin y mantiene luego
su demanda mediante visuales con lease de 60 s. No tienen lectores ni heartbeats
periódicos reales: su coste no representa audio, multimedia, OBS o métricas.

Hay tres repeticiones independientes por cantidad de plugins. Cada una incluye
calentamiento, 100 acciones calientes, 15 s de reposo y 15 s de carga sostenida.
La carga consulta estado y ejecuta acciones; se conservan las primeras 1.000
muestras de carga y el número total de iteraciones. No es una distribución aleatoria
ni mide toda la cola temporal de la carga.

El máximo de `Server::wait` es largo; readiness y un socket de coordinación cierran
las fases sin introducir un tick artificial. Se acumulan cantidad, suma y máximo de
`Server::turn`, excluyendo `wait`. El cliente corre en un hilo del fixture: su CPU,
memoria y cambios de contexto **sí están incluidos** en el PID runtime instrumental.
Tampoco equivale exactamente al binario de producto, que instala señales y adapters.

El fixture empaqueta e instala copias de su propio ejecutable antes de medir. Su
historial de asignaciones e inventario cambia con N. Los buffers temporales ya han
caído al medir, pero el allocator podría conservar páginas: es una hipótesis, no
una causa demostrada por PSS. La diferencia de memoria N=0/1/4 **no demuestra coste
incremental de plugins reales ni una fuga**; para atribuirlo hace falta comparar
estados con el mismo historial de preparación.

Las latencias se definen así:

- **Admisión IPC:** desde enviar `run` hasta recibir su respuesta.
- **Terminación observada:** desde antes de `run` hasta que `result` informa éxito;
  incluye serialización, transporte y el sondeo acotado del cliente.
- **Fría:** primera ejecución con proceso ausente, una por plugin/repetición. No
  implica caché de disco fría ni un sistema recién arrancado.
- **Caliente:** ejecución con demanda establecida. No es latencia física tecla→acción.

En N=0 la acción es Wait de 0 ms: «cold» es solo su primera llamada nativa, no el
arranque de un plugin.

### Recursos

El observador Python está fuera de los PIDs medidos. Cada 200 ms recorre solamente
el árbol de descendientes del proceso que acaba de lanzar, incluyendo hijos de todos
sus hilos. Nunca enumera procesos ajenos. Verifica identidad por PID/starttime.
El cierre normal exige reap; si falla, termina únicamente identidades propias vistas,
verificadas por sesión y starttime y señaladas mediante pidfd para evitar PID reciclados.

Se registran RSS, PSS y memoria privada de `smaps_rollup`, ticks de CPU, hilos y
file descriptors. RSS sumada cuenta páginas compartidas varias veces: preferir PSS.
Los picos son **máximos muestreados**, no máximos absolutos. Las lecturas no son una
instantánea atómica y el observador introduce un coste externo pequeño no descontado.
CPU usa un núcleo=100 %, con resolución `1/CLK_TCK`; cero no prueba ausencia de CPU.

Se suman los cambios de contexto observados de todos los hilos vivos, con identidad
de hilo y deltas entre muestras. Hilos que desaparecen pueden dejar trabajo sin
observar: es una cota inferior, **no despertares exactos por segundo**. Los cambios de
contexto tampoco identifican por sí solos una causa de wakeup.

### Dominio y persistencia

`domain_profile` construye directamente configuraciones validadas de 32, 7.000 y
10.000 identidades **vigentes**, no solo un ledger histórico. Usa 12 muestras por
operación y dos variantes: sin medios, y catálogo compartido de hasta 16 PNG
sintéticos de un píxel. Esta última no caracteriza imágenes o catálogos grandes.

Distingue construcción de Authority/proyecciones, `prepare_edit`, admisión de una
mutación, commit real en Store, publicación tras commit, carga y navegación de
Authority. La navegación no incluye render ni subida de imágenes al deck. La
persistencia mantiene validación, checksums, sync y reapertura real; no se vacían
cachés del sistema ni se eluden garantías para mejorar cifras.

El máximo de crecimiento actual es 7.000 identidades. Aunque el dominio admite
10.000 por compatibilidad histórica, los documentos concretos generados a ese tamaño
superan 1 MiB: las operaciones no persistibles se emiten como `unmeasured`, sin
forzar límites. El perfil de 7.000 sí se publica y guarda.

Los percentiles usan nearest-rank, sin interpolación. Con 12 muestras p95 y p99 son
el máximo: no son estimaciones sólidas de una cola de producción. Las operaciones
se muestran separadas; no sumar percentiles para inventar una latencia extremo a extremo.

## Resultados

Corrida final **2026-10-23**, base `486f2f2`, Linux `7.2.8-9-omarchy`, x86_64,
16 CPU lógicas, Python 3.15.1, CLK_TCK=100, páginas de 4 KiB. Evidencia local:
`target/performance/20261023T104807-60475/`. Tres repeticiones independientes por
escenario, sin builds concurrentes. Las huellas de los cuatro archivos coinciden
con `metadata.json`; la corrida anterior `20261023T104247-54838` es solo piloto,
anterior al endurecimiento del observador, y no alimenta estas tablas.

Huellas del arnés medido:

| Archivo | SHA-256 |
|---|---|
| `scripts/performance/profile.py` | `e80c33b47d5874b1e3ff616be6ec5b0900fa9701465bfff35f97f7dd7f093b3c` |
| `scripts/performance/run.sh` | `4687d7672c53afd8b227616615720adb2a54cf927589d21fde1294d63282f3b2` |
| `runtime_profile.rs` | `b69f36dc75bf5d5ea8bd5fdafab8aaf48a914fcfe6ed7f9ec379d6d11d009c31` |
| `domain_profile.rs` | `e32a8c965ff3672fc822f827b4e54d9367134ae6d8f5fba609cee44eb0cb8a29` |

### Reposo · runtime instrumental + helper + ecos

Rangos de las medianas por repetición; MiB = 1.048.576 bytes. Procesos, hilos y
FD son totales del árbol propio, estables en las tres repeticiones.

| Ecos demandados | RSS MiB | PSS MiB | Privada MiB | Procesos / hilos / FD |
|---|---:|---:|---:|---:|
| 0 | 17,29–17,45 | 9,15–9,47 | 8,77–9,09 | 2 / 8 / 30 |
| 1 | 32,42–32,70 | 22,20–22,51 | 21,84–22,14 | 3 / 9 / 37 |
| 4 | 84,14–85,11 | 67,28–68,32 | 66,86–67,88 | 6 / 12 / 58 |

En las nueve ventanas estables de reposo no se observaron incrementos de ticks de
CPU ni cambios de contexto. Es **por debajo de la resolución de observación**, no
una garantía de CPU o despertares exactamente cero. El contador de turnos registró
un solo turno de frontera por ventana de 15 s, sin tick artificial periódico.
El deck está deshabilitado: esto no evalúa el sondeo de un dispositivo ausente.

La memoria de los escenarios con ecos incluye la preparación instrumental descrita
arriba: **no usar las diferencias de esta tabla como RAM incremental de plugins de
producto**. El PID del runtime representa la mayor parte de ese crecimiento; no
es una prueba de fuga ni permite evaluar el presupuesto del plugin de métricas.

### Carga sostenida

Carga intencionalmente saturada, no una sesión humana de uso. CPU es el rango de
las tres observaciones, incluyendo cliente interno y plugins; 100 % = un núcleo.

| Ecos | CPU % | Mayor PSS muestreada MiB | Iteraciones/s aprox. | Mayor turno observado ms |
|---|---:|---:|---:|---:|
| 0 | 117,04–117,17 | 9,73 | 2.572–2.579 | 2,076 |
| 1 | 110,92–111,14 | 22,81 | 1.296–1.308 | 2,067 |
| 4 | 107,50–107,74 | 68,41 | 842–850 | 1,897 |

No son presupuestos ni límites superiores del producto. El máximo de turno aquí
corresponde a una configuración pequeña con ecos, no al perfil de 7.000 entidades.
Los 18 grupos de recursos conservaron conjuntos estables de procesos y los nueve
casos terminaron con `clean_shutdown=true`, sin supervivientes propios detectados.

### Latencia IPC · terminación observada

Rangos entre repeticiones: p50 y p95 calculados **por repetición**, no percentiles
de percentiles ni intervalos de confianza. Caliente: 100 muestras por repetición.
Fría: N=1 tiene una muestra por repetición; N=4, cuatro; N=0 es Wait nativo.

| Ecos | Fría p50 ms | Caliente p50 ms | Caliente p95 ms |
|---|---:|---:|---:|
| 0 | 0,123–0,165 | 0,126–0,134 | 0,156–0,202 |
| 1 | 1,237–1,480 | 0,264–0,265 | 0,320–0,344 |
| 4 | 1,202–1,365 | 0,506–0,525 | 0,639–0,703 |

Para N=1, la admisión caliente tuvo p50 0,169–0,172 ms. Durante la carga, la
terminación observada tuvo p95 0,688–0,747 ms (N=1) y 1,065–1,133 ms (N=4), sobre
las primeras 1.000 iteraciones por repetición. No extrapolar a acciones de terceros,
latencia física ni cachés frías de disco.

### Dominio y Store

408 muestras en 34 grupos; 12 por operación/variante/tamaño. Tabla sin medios,
**p50 / máximo**, en milisegundos (con n=12, p95 coincide con máximo).

| Operación | 32 identidades | 7.000 identidades | 10.000 históricas |
|---|---:|---:|---:|
| Construir Authority y proyecciones | 0,020 / 0,047 | 54,560 / 60,988 | 123,803 / 134,407 |
| Preparar edición | 0,015 / 0,022 | 22,611 / 24,627 | 47,517 / 50,917 |
| Admitir mutación en Authority | 0,038 / 0,049 | 27,182 / 28,755 | No persistible |
| Commit Store durable | 5,028 / 6,098 | 65,762 / 73,807 | No persistible |
| Publicar tras commit | 0,021 / 0,032 | 52,214 / 57,797 | No persistible |
| Cargar Store | 0,194 / 0,286 | 23,582 / 25,502 | No persistible |
| Navegar en Authority | 0,011 / 0,014 | 26,344 / 28,631 | 62,069 / 64,804 |

Con 7.000 identidades y 16 PNG compartidos de 1 píxel (1.168 bytes de medios), el
commit dio p50 **67,640 ms**, máximo **77,945 ms**; publicar, p50 **53,113 ms**.
No es una prueba del coste de medios grandes ni permite descartar M2.

Documentos canónicos: 7.000 sin medios **1.006.725 B**; con medios **1.029.169 B**.
El perfil concreto de 10.000 supera el máximo de **1 MiB** y Authority rechaza la
publicación como `resource_exhausted(document_bytes)`. No se fabricó una cifra de
guardado para ese caso. No significa que todo documento histórico de 10.000 sea
inválido: el tamaño depende de su contenido.

### Lectura y siguiente experimento

- El reposo del núcleo aislado no muestra aquí actividad periódica observable;
  no se justifica optimizarlo por conjetura a partir de la evidencia vieja de M3.
- La publicación y navegación con muchas identidades sí merecen profundizar M1:
  en este perfil pasan de centésimas de ms a decenas de ms. Aislar proyecciones,
  sincronización de entrada y validación antes de elegir índices o refactors.
- Store añade un coste durable medible, pero no se deben quitar sync/checksums.
  La tabla no identifica todavía qué subpaso de M2 domina.
- Separar preparación/inventario de demanda para atribuir memoria, y medir después
  los oficiales con backends sintéticos de su implementación real. No confundir
  eso con certificar NVML, OBS, audio o el escritorio personales.
- No se fijaron nuevos presupuestos ni se aplicaron optimizaciones. Primero
  acordar escenarios representativos y objetivos de respuesta con estos límites.

## Validación de la entrega

Contratos instrumentales temporales: muestreo/stat y CPU, límite CLI antes de
efectos, rechazo de sesión ajena antes de memoria/hilos, handshake dinámico,
salida de dominio y rutas privadas. Hubo rojo–verde para cálculo, CLI, handshake
y salida ausente; comprobaciones de aislamiento adicionales verdes. No se cambia
ningún contrato del producto ni se añaden tests permanentes no críticos.

Gate completo de AGENTS.md **superado en serie**, después de la medición: fmt,
build release offline locked, tests y clippy `-D warnings` de ambos workspaces;
**1.184 + 269 tests pasados**, con 3 + 2 ignorados en las invocaciones ordinarias;
y conformidades audio/media, OBS y métricas verdes con fixtures privados. Logs en
`target/performance/gate-final/`. Sockets/namespaces requirieron permisos fuera del
sandbox. No se operaron hardware, ventanas ni servicios personales.

Es validación funcional del artefacto, no sustituto de las mediciones de fallos
pendientes ni certificación del hardware o de todas las distribuciones.

Revisión independiente de estándares, aislamiento, método, huellas y tablas sin
hallazgos bloqueantes; acepta esta primera entrega, no el cierre de la fase.
Los tests temporales y el plan de ejecución se enviaron a la papelera, conservando
logs y resultados. Ambas suites definitivas se repitieron y pasaron después de
esa retirada, sin añadir tests permanentes no críticos.

## Pendientes de la fase 4

- Editor real con foco, sin foco y cerrado: medido después con XL en el escritorio
  real. No mide CPU del compositor ni memoria GPU, ni equivale a todos los drivers.
- Latencia completa de contacto físico→acción/píxeles. El ensayo posterior ya
  verifica navegación y Sleep/Wake reales bajo control exclusivo, con tiempo de
  reporte consumido→Desired, sin timestamp HID ni confirmación de píxeles.
- Coste real de audio/multimedia/OBS: pendiente de servicios autorizados. Métricas
  con procfs/NVML reales ya se midió después: 28,496–28,855 MiB RSS con 32 visuales.
  No cumplió los 20 MiB originales; el usuario aprobó después **30 MiB RSS**, que
  sí cumple ese escenario, sin optimización ni certificación universal.
- Despertares exactos y sus causas: no se dispone de un trazador aprobado en este
  entorno; no instalarlo por inferencia.
- La continuación ya perfila downloader retenido, host que no lee, conversor
  retenido, worker muerto y escritura incierta en componentes controlados. Sigue
  sin certificar toda la composición Server/GUI ni un cuelgue real de codec/driver.
- Catálogo grande sintético ya medido hasta 128 PNG y unos 32 MiB; falta validar
  representatividad en otros contenidos/cargas, sin leer configuraciones personales
  por inferencia.
- Fijar presupuestos tras discutir la línea base; no aplicar optimizaciones por
  inferencia. M1–M4 son candidatos, no una lista de problemas ya demostrados.

La fase queda **en curso**. Cada nuevo ensayo físico requiere autorización y
control exclusivo; el previo no autoriza otros dispositivos o sensores.
