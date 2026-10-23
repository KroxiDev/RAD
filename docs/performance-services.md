# Medición de RAD · fallos integrados, wakeups y servicios reales

Continuación de [escritorio y métricas](performance-desktop.md), sobre base
`d2e6cb3`. El usuario autorizó los tres bloques siguientes de fase 4, incluidos
los permisos necesarios, y pidió **detenerse antes de OBS real**. UX sigue
pospuesta. El objetivo aprobado de Métricas permanece en **≤30 MiB RSS**; esta
entrega no cambia otros presupuestos ni optimiza el producto.

## Alcance y seguridad

- Ejecución en serie: primero fixtures propios, luego contadores y finalmente
  servicios de audio/multimedia. Sin editor, XL, sensores, cambios de reproducción,
  volumen, mute, dispositivos predeterminados ni configuración personal.
- ROOT es el único operador. No se consulta OBS real, sus puertos, configuración
  o credenciales. La conformidad ordinaria de OBS usa únicamente servidores falsos.
- Fuentes, ejecutables e inputs se cotejan y copian **antes** de ejecutar; se
  comprueban de nuevo al terminar. Los artefactos exactos quedan ignorados bajo
  `target/performance/`, no en el repositorio público.
- Solo se publican agregados genéricos. No se retienen títulos, nombres de
  dispositivos, instancias de reproductores, artwork ni cookies personales.
  El selector genérico de aplicación puede existir temporalmente en el Store
  privado de la prueba; ese perfil se retira a la papelera tras agregar resultados.
- Los escenarios son observaciones locales y acotadas, no máximos absolutos,
  garantías universales ni una certificación de fallos físicos de disco o drivers.

## Cinco fallos compuestos

Tres repeticiones independientes, **15/15 casos aprobados**. Usan Server,
Transport y cliente tipado reales, con dependencias retenidas controladamente.
No son pruebas de GUI. Las entradas, paquetes y Stores de cada caso se conservan.

| Caso | Composición y resultado observado |
|---|---|
| Downloader retenido | Admin/Manager reales; proveedor sintético retenido sin red. Crédito único, una llamada de descarga, primera demanda fría de otro plugin sana y publicación tras liberar. |
| Host que no lee | Handshake real; 16 trabajos pendientes y cola real de escritura. Status y Wait sanos, reconexión sin replay y reap al cerrar. |
| Conversor oficial retenido | MediaPlugin/Watch/Reader/Converter reales, bus y reproductores privados. Otra lectura progresa Playing→Paused mientras se retiene una imagen; al liberar, JPEG real y normalización PNG del runtime. |
| Store incierto | Commit SQLite real seguido de pérdida controlada de clasificación del retorno. Cliente recibe Unknown; reconciliación relee un único cambio, sin replay. No simula un fsync físico fallido. |
| Worker Store perdido | Commit SQLite real seguido de salida controlada antes del ACK. Unknown/recovery_required; status sano y reinicio que relee un solo cambio, sin replay. |

En las tres repeticiones, status sano tarda **0,065–0,211 ms** y el mayor turno
observado es **0,722 ms**. La reconciliación observada del Store incierto tarda
**30,58–50,98 ms** e incluye sondeo del cliente cada 10 ms. No se descuentan
overheads para presentar esas cifras como latencia interna pura. Los agregados
de turnos incluyen configuración y observación, excluyen construcción/seed/attach
y cierre. El ACK de recursos Store caídos no equivale a un join del hilo.

Evidencia: `target/performance/f4-faults-final/` (manifest previo independiente,
8 ELF, 12 fuentes y 2 inputs; 165 registros JSONL) y perfiles `f4f-1` a `f4f-3`.
ELF de tests medido: SHA-256
`a8006d4c0ccb36a22711e61efdd971a517046743a5ac751e5238b0a9c252f549`.
Los pilotos anteriores y sus errores de preparación no se mezclan con esta tabla.

## Contadores exactos del kernel

Se usa `perf_event_open` sobre **sched:sched_wakeup**, con filtro del campo
**pid del TID despertado**, no common_pid del emisor. Solo contadores, sin muestras,
ring buffers, trazas generales ni escrituras en tracefs. No se instaló ninguna
herramienta ni se cambiaron sysctl/capacidades. La elevación local de Polkit dura
un batch acotado; el kernel cierra los FD al morir el helper.

La [API de contadores](https://man7.org/linux/man-pages/man2/perf_event_open.2.html)
y los [filtros de eventos del kernel](https://docs.kernel.org/trace/events.html)
son las referencias del método. Se exige enabled_ns == running_ns > 0, sin
escalado ni multiplexación, CPU online y TIDs estables, identidad/ascendencia y ELF
propios verificados. Si el formato presenta success, se filtra también success=1.

Antes de medir RAD, una calibración despierta diez veces un target seleccionado
y diez veces otro excluido desde un emisor externo. El ACK usa memoria compartida,
no un pipe de vuelta que pudiera producir falsos positivos con common_pid.
Se exige un total exacto de diez eventos seleccionados.

Cada ventana usa FD nuevos por **CPU emisora**, con cotas CLOCK_MONOTONIC de los
ioctl. No hay un corte simultáneo global: se informan conteos de esos intervalos
y una tasa **derivada**, suma de count/enabled_s por CPU. Las ventanas deben estar
íntegramente dentro de las fases marcadas por el fixture con el mismo reloj.

El perfil 0/1/4 usa plugins eco inertes y runtime/helper reales, sin servicios,
GUI o XL. El cliente de carga está en un hilo del fixture y su coste está incluido;
también el historial de preparación/asignaciones. Se cuentan los TIDs presentes
y verificados en ambos extremos, no hilos transitorios o hijos no seleccionados.
No mide salidas de C-states, causas, latencia wakeup→run ni wakeups de los oficiales.

### Resultado y límite actual

La calibración real preliminar `wk-cal1` contó **10/10** eventos del target, con
diez despertares negativos excluidos, 16 CPU cubiertas y enabled==running en todos
los contadores. Helper cerrado con exit 0, FD cerrados y procesos propios recogidos.
Conserva fuentes previas y hashes de herramientas, pero no copias previas de los
ELF de intérprete/pkexec: no se atribuye retrospectivamente esa copia al piloto.

El intento principal `wk-main1` conservó previamente también esas copias audit-only,
pero la autenticación local venció antes de que el helper confirmara ready.
Quedó **incompleto**, con cero ventanas realizadas, sin counters.json ni eventos
del helper. EOF/deadline y SIGTERM por pidfd al launcher aún del UID propio
terminaron la espera (returncode -15). **No significa cero wakeups de RAD**.
Ese intento fallido se conserva separado de la captura válida posterior.

### Captura principal completada

Con autorización del usuario para retomar este punto y autenticación local,
`wk-main2` completó la calibración **10/10** y las **seis ventanas** sobre base
`93bf217`, sin cambios de código ejecutable ni nuevas herramientas. Linux
7.2.8-9-omarchy, x86_64, 16 CPU emisoras. Una ventana nominal de 8 s por fase y N,
dentro de fases instrumentales de 16 s; **una repetición**, no un estudio estadístico.

| Plugins eco | Fase | Eventos contados | Tasa derivada /s | Runtime / plugins / helper |
|---:|---|---:|---:|---|
| 0 | Reposo | 0 | 0,00 | 0 / 0 / 0 |
| 0 | Carga | 128 052 | 16 006,13 | 128 052 / 0 / 0 |
| 1 | Reposo | 0 | 0,00 | 0 / 0 / 0 |
| 1 | Carga | 74 290 | 9 285,89 | 63 683 / 10 607 / 0 |
| 4 | Reposo | 0 | 0,00 | 0 / 0 / 0 |
| 4 | Carga | 47 964 | 5 995,27 | 41 112 / 6 852 / 0 |

La última columna reparte **eventos**, no tasas. Son sumas de contadores con sus
intervalos por CPU, no un corte global simultáneo de exactamente 8 s. Cobertura
verificada por ventana: 2/3/6 procesos y 8/9/12 TIDs para N=0/1/4. En todos los
FD, enabled_ns == running_ns > 0, sin escalado; todos los intervalos quedaron
dentro de sus marcadores de fase. La calibración volvió a excluir diez despertares
del target negativo y no usa pipe de ACK.

**Interpretación:** no se observaron despertares de los TIDs seleccionados en estas
ventanas de reposo. No implica cero despertares futuros del producto, del hardware
o de plugins oficiales. La carga es un bucle instrumental sostenido con cliente
embebido; sus tasas no representan uso normal. N=0 usa una acción nativa y N>0
acciones eco, con distinto trabajo por iteración y throughput: la caída de la
tasa al subir N **no demuestra una mejora de eficiencia**. No se adopta un
presupuesto de wakeups ni se propone optimizar a partir de esta única captura.

Evidencia: `target/performance/wk-main2/`, con metadata, calibration.json,
counters.json, helper-events.json, eventos del fixture y summary.json. Fuentes,
ELF del fixture/helper e intérpretes/pkexec audit-only copiados y cotejados **antes**
de ejecutar; hashes de fuentes/inputs comprobados después. Batch terminado con
exit 0, todos los FD cerrados y procesos propios recogidos. Esta captura completa
el conteo exacto **del perfil definido**, no las exclusiones de representatividad
ni el resto de la fase 4. No usó editor, XL, servicios personales ni OBS.

## Audio y multimedia de solo lectura

Plugins oficiales de producción, runtime/helper reales y perfiles nuevos privados.
Cada repetición separa cinco fases: instalado sin demanda, 1 visual, 32 visuales,
demanda retirada y runtime nuevo sin demanda. La preparación ocurre fuera del
runtime medido. Se muestrean RSS, PSS, privada, CPU, FD, hilos y procesos del árbol
propio, además de disponibilidad visual; no se ejecutan acciones del plugin.

- Audio: pactl real con allowlist exacta de list sinks, list sources, info y
  subscribe, socket Unix explícito y configuración privada autospawn=no. Nunca
  se lee una cookie personal; autenticación no disponible queda NO_VERIFICADO.
- Multimedia: proxy filtrado de métodos/señales según el
  [manual de xdg-dbus-proxy](https://man.archlinux.org/man/extra/xdg-dbus-proxy/xdg-dbus-proxy.1.en).
  El gate usa **una conexión persistente**: permite GetAll del nombre conocido
  y del owner único, pero exige AccessDenied para Play y Set de ambos. Esas llamadas
  de control se prueban **solo contra el reproductor sintético privado**, nunca
  contra el servicio personal. La no autoactivación la aporta NO_AUTO_START del
  Bus del producto, no se infiere exclusivamente del proxy.
- Namespace privado oculta home/tmp/proc personales. No se mide artwork de archivos
  personales. Un servicio ausente no se sustituye por uno falso para declarar PASS.
- Antes de conectar endpoints reales se verifica el gate completo con bus/player
  privados, allowlist y aislamiento. La prueba preliminar `svc-v1` fue aprobada;
  cada corrida real repite ese gate antes de conectar.
- Supervisores namespace/proxy se separan del coste del plugin. CPU/memoria de
  procesos pactl breves puede escapar al muestreo; cero CPU puede significar menos
  de un tick. La disponibilidad son observaciones discretas, no frescura continua.
  Cambios de contexto no se etiquetan como wakeups exactos.

### Resultados reales

Tres repeticiones por plugin, cinco fases de 5 s, intervalo nominal de muestreo
de 200 ms. **6/6 ciclos completos**, 150 agregados de recursos, fingerprints
coincidentes y cierre/reap de todos los procesos propios. No se inició ningún
reproductor personal: se usó una fuente ya disponible descubierta con ListNames
solo en memoria, sin guardar nombres de instancia.

Picos **muestreados máximos entre las tres repeticiones**, árbol del plugin y sus
descendientes, excluyendo runtime/helper y supervisores:

| Plugin | Visuales | RSS MiB | PSS MiB | Privada MiB | Procesos / hilos / FD pico |
|---|---:|---:|---:|---:|---|
| Audio | 1 | 8,105 | 2,194 | 2,047 | 2 / 4 / 14 |
| Audio | 32 | 8,148 | 2,237 | 2,090 | 2 / 4 / 14 |
| Multimedia | 1 | 8,102 | 2,397 | 2,309 | 1 / 8 / 11 |
| Multimedia | 32 | 8,168 | 2,464 | 2,375 | 1 / 8 / 13 |

Instalado sin demanda, retirada y runtime nuevo sin demanda: **cero procesos del
plugin observados** en todas las repeticiones. Con demanda, se obtuvieron
**144 observaciones válidas current de audio y 129 de multimedia**, cero stale o
unavailable en las muestras. No prueba frescura entre observaciones ni todas las
combinaciones de dispositivos/reproductores. La CPU de esos árboles fue 0,0 %
de un núcleo en las ventanas; no demuestra coste nulo y mantiene el límite por ticks
y de hijos breves. Los recursos del árbol completo y supervisores quedan en el
raw, no se suman silenciosamente a los del plugin.

Evidencia: `target/performance/svc-real1/` (metadata, fuentes/ELF previos, gate,
availability, resources y muestras/eventos sanitizados). No se adopta un presupuesto
nuevo para Audio/Multimedia ni se cambia el objetivo independiente de Métricas.
Los seis perfiles privados se enviaron a la papelera después de la auditoría y
agregación; las fuentes/ELF previos y la evidencia sanitizada se conservaron.

## Reproducción y frontera

Sin flags de ejecución, ambos controladores son dry-run y no inspeccionan ni
lanzan procesos, servicios o contadores:

```bash
python3 -I -S -B scripts/performance/wakeups.py target/performance/wk-new
python3 -I -S -B scripts/performance/services_real.py target/performance/svc-new
```

Tras compilar offline los binarios/helpers/ejemplos de ambos workspaces y con
autorización apropiada, el contador requiere --run-counters y servicios requiere
--run-services con sockets Unix propios explícitos y selector genérico permitido.
--validate-only ejecuta únicamente el gate sintético. No usar endpoints personales
por inferencia ni ejecutar builds u otros benchmarks durante una medición.

## Validación y cierre de esta entrega

Gate completo de AGENTS.md **11/11 PASS**, offline, release/locked/all-features,
en serie y sin editor/XL: fmt, builds de bins/ejemplos, tests y clippy
`-D warnings` de ambos workspaces y tres conformidades privadas. **1.189 + 270
tests aprobados**, cero fallos, 3 + 2 ignorados en las suites ordinarias. Además,
**62 controles críticos Python**, **2 tests del opt-in del seed** y **2 del gate
persistente MPRIS** aprobados. Logs `01-*` a `15-*` de
`target/performance/last-three-gate/`.

Se conservaron las regresiones críticas y se retiraron a la papelera el plan y
runner temporales; no se borró la evidencia. Después se repitieron ambas suites
(**1.189 + 270**, cero fallos, 3 + 2 ignorados), los **62 Python** y ambos pares de
tests de ejemplos: todo aprobado. Logs definitivos `16-*` a `20-*` y summary.json
en el mismo directorio. Esa revisión independiente aprobó la entrega parcial
`93bf217`, dejando entonces pendiente la captura principal del kernel. La
continuación `wk-main2` resuelve ese bloque sin modificar código ejecutable;
**no cierra la fase 4**, que conserva las fronteras y decisiones pendientes.

En la retoma se aprobaron los **20 controles del contador antes de ejecutar** y
los **62 controles Python** después, más diff check. El primer intento de estos
62 dentro del sandbox no pudo verificar el fixture de fuser por bloqueo de
sockets y falló la espera dry-idle por EOF prematuro; no se diagnosticó por separado
la causa de ese EOF. Se conservan ambos fallos, no como PASS. La repetición
autorizada fuera del sandbox pasó completa. Logs en
`target/performance/wk-retry-validation/`.
Solo cambian este informe y el roadmap; no se volvió a atribuir el gate Rust
anterior a una ejecución nueva. La auditoría independiente cotejó también las
seis ventanas, sus fuentes/ELF y límites, con el raw de `wk-main2`.

**OBS real permanece sin iniciar**, por pedido del usuario. La fase 4 sigue abierta:
quedan sus pruebas coordinadas y las exclusiones/decisiones de cierre indicadas en
el [roadmap](roadmap.md#fase-4--medición--en-curso). UX no se inicia automáticamente.
