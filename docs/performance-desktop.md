# Medición de RAD · escritorio real y continuación de fase 4

Continúa el [ensayo físico](performance-physical.md), sobre base `079f793`.
El usuario autorizó completar lo autónomo con el XL disponible y después autorizó
expresamente consultas temporales de CPU/RAM/GPU mediante procfs/NVML, solo lectura.
UX permanece pospuesta. Esta entrega mide: no cambia contratos, límites,
dependencias, comportamiento de producto ni diseño de interfaz.

La [continuación posterior](performance-services.md) registra los fallos compuestos
restantes, contadores del kernel y lecturas de audio/multimedia reales. El estado
de cierre al final de este informe es histórico de esta entrega, no de la posterior.

## Aislamiento y método

- ROOT es el único operador del XL y escritorio. Preflight inmediato por apertura,
  vigilancia scoped de sus dos nodos (espera de 1 s tras cada check, no 1 Hz exacto),
  solo el PID propio del runtime como
  escritor admitido y postflight sin titulares. No se detienen aplicaciones ajenas,
  reinician USB ni inducen fallos del driver. La exclusividad es observada de forma
  discreta, no una garantía frente a escritores no cooperantes entre checks.
- Runtime y editor **reales**, eframe/Glow y Wayland del escritorio actual. HOME,
  datos, configuración, caché, estado y socket RAD privados. Bus D-Bus privado.
  El editor instala sus archivos de menú/unidades solo en estos directorios.
- El editor siempre intenta iniciar su servicio: un `systemctl` privado permite
  únicamente daemon-reload/start/show del runtime sin ejecutar el systemctl real.
  Rechaza cualquier otra operación. El runtime lo inicia y supervisa el arnés.
  Esto mide la aplicación con backend de servicio aislado, **no systemd/bandeja real**.
- El foco se dirige únicamente a una dirección de ventana verificada por
  PID/starttime propio; el desfoco usa `foot --hold /usr/bin/true`, con el hijo ya
  terminado. Cada muestra verifica el foco; si cambia, invalida el caso en lugar
  de recuperarlo del usuario. Se guardan solo rol/PID propios y timestamp de esa
  corroboración, nunca títulos ni listas del escritorio ajenas.
- La sintaxis Lua de foco se contrastó con el manual/stub instalados y los
  [dispatchers oficiales de Hyprland](https://wiki.hypr.land/configuring/core/dispatchers/).
  No se cambia la configuración del compositor.
- Cancelación SIGINT/SIGTERM acotada: el registro Popen→identidad/rol difiere la
  interrupción sin heredar SIG_IGN al hijo. El reap termina antes de entregar una
  primera cancelación recibida durante cleanup. Un aborto mata únicamente el
  escritor propio sin nuevas escrituras de logo; no se declara completo.

### Ventanas GUI/XL

Tres repeticiones independientes de cinco fases de 8 s: runtime solo, editor
con foco, editor sin foco, navegación IPC con editor enfocado y editor cerrado.
Perfil temporal con dos carpetas y 32 slots visibles, brillo 25 %, sin suspensión
por inactividad ni plugins. La preparación ocurre en otro proceso antes de cada
runtime medido. Los cierres son por acción `quit` privada y SIGTERM normal del
runtime; guard y postflight se mantienen durante su liberación.

Muestras solicitadas cada 250 ms, sujetas al coste de observación; los timestamps
reales mandan. RSS/PSS/privada y CPU/hilos/FD de procesos propios se separan por
runtime+helper y editor. Bus privado, cliente IPC, ventana de desfoco y observador
están fuera de `product_tree`. El driver gráfico puede añadir memoria compartida;
esto **no mide memoria GPU ni CPU del compositor**, y RSS sumada duplica páginas.
CPU cero observado no prueba ausencia de actividad; cambios de contexto no son
wakeups exactos. La navegación mide ejecución IPC observada, no contacto/USB/píxeles.

## Resultados GUI/XL

Corrida válida: `target/performance/ds-xl2/`, **tres ciclos completos**, 15 fases,
495 muestras de recursos y 396 corroboraciones sanitizadas de foco. Los tres
editores cerraron por su acción privada; runtime/helper terminaron limpios,
sin supervivientes propios ni titulares del XL al acabar. No se infiere de eso
logo/píxeles restaurados: estos ciclos no tuvieron confirmación visual humana.

Rangos de las medianas de cada repetición; `product_tree` = runtime + helper +
editor cuando existe. MiB = 1.048.576 B, CPU 100 % = un núcleo:

| Fase | RSS MiB | PSS MiB | CPU % | Procesos / hilos / FD |
|---|---:|---:|---:|---:|
| Runtime conectado, sin editor | 21,418–21,738 | 13,458–13,646 | 0–0,250 | 2 / 8 / 31 |
| Editor enfocado | 178,922–179,270 | 107,288–107,591 | 0–0,125 | 3 / 20 / 81 |
| Editor desenfocado | 180,922–181,270 | 109,285–109,590 | 0–0,250 | 3 / 20 / 81 |
| Navegación IPC, editor enfocado | 179,945–180,312 | 108,309–108,633 | 2,875–3,000 | 3 / 20 / 80 |
| Editor cerrado, runtime continúa | 18,250–21,875 | 10,290–13,782 | 0 observado | 2 / 8 / 31 |

El editor solo: PSS mediana 93,761–93,888 MiB enfocado y pico muestreado máximo
95,961 MiB durante navegación. Su proceso desaparece al cerrar; no queda GUI
residente. La mayor PSS muestreada del árbol fue 109,738 MiB. El orden fijo de
fases, asignaciones previas y páginas compartidas impiden atribuir las diferencias
de memoria exclusivamente al foco o al allocator; no demuestran una fuga.

Navegación IPC a aproximadamente 4 Hz, 33 muestras por repetición: p50
**0,660–0,715 ms**, p95 **0,782–0,867 ms**, máximo observado **0,941 ms**.
Son percentiles por repetición, no percentiles de percentiles ni un presupuesto.
Se verificó carpeta esperada en cada transición. No incluyen el contacto físico,
upload/píxeles ni una confirmación de frame presentado por el compositor.

`ds-xl1` se conserva como **parcial/inválida**: el foco cambió, se abortó sin
recuperarlo del usuario, se terminó el escritor propio y el postabort confirmó
cero titulares, sin afirmar restauración del logo. La corrida válida empezó
después de que el usuario confirmara una ventana sin teclado/ratón. No se mezclan
datos del intento parcial ni de los pilotos sin hardware.

## Cargas adicionales de dominio y medios

`domain_deep_profile --representative` añade ocho configuraciones sintéticas:
32/256/1.000 identidades; sin medios y catálogos planos/bandas de 1/4/16 imágenes,
64–128 px, compartidas o únicas. Diez muestras por operación, 770 en total;
preparación fuera del timer, checksums/durabilidad y reapertura real conservados.
No pretende ser un censo de configuraciones personales ni suma percentiles de
operaciones independientes. No aumenta límites para fabricar guardados de 10.000.

Corrida retenida: `target/performance/phase4-finish/representative-freeze.jsonl`.
p50 nearest-rank en ms (n=10 por operación), no promedio de los dos valores centrales:

| Identidades sin medios | Preparar edición | Navegar Authority | Commit durable |
|---|---:|---:|---:|
| 32 | 0,015 | 0,010 | 26,161 |
| 256 | 0,139 | 0,191 | 28,338 |
| 1.000 | 0,791 | 0,661 | 30,826 |

| Catálogo adicional | Bytes canónicos | Commit existente p50 / máx. ms | Resuministrado p50 / máx. ms |
|---|---:|---:|---:|
| 32 identidades, 1 plano compartido, 64 px | 4.741 | 24,114 / 25,266 | 24,355 / 28,830 |
| 256, 4 bandas compartidas, 64 px | 61.201 | 26,555 / 29,787 | 27,255 / 30,538 |
| 256, 4 bandas únicas, 64 px | 61.201 | 27,476 / 39,462 | 27,856 / 36,776 |
| 1.000, 16 bandas compartidas, 128 px | 1.032.284 | 39,388 / 55,731 | 46,986 / 58,789 |
| 1.000, 16 bandas únicas, 128 px | 1.032.284 | 41,389 / 67,300 | 47,715 / 49,793 |

En el producto la durabilidad ocurre en un worker; este ejemplo mide Store
directamente. Sus tiempos no equivalen a un bloqueo de la interfaz. No vaciar
cachés ni controlar toda la carga de almacenamiento impide
atribuir diferencias con otras corridas a regresiones del producto.

Los píxeles de baja entropía **no garantizan un PNG canónico pequeño**: el encoder
real usa Fast/NoFilter. El plano de 64 px quedó en 4.741 B (28,9 % de RGBA); las
bandas en 93,3–98,4 %. Las aserciones de ratio ensayadas durante preparación eran
supuestos instrumentales y se retiraron; no eran bugs del producto ni un presupuesto
aprobado. No se cambió el encoder para mejorar los números.

## Fallos integrados controlados

Cinco tests críticos existentes de `server_options` exponen instrumentación opt-in
`RAD_PROFILE_INTEGRATED=1`: plugin lento, caída, respuesta inválida, cliente que se
va y saturación del crédito de consultas. Camino real: cliente tipado → socket →
Transport → Server → Authority/Host → proceso fixture. Conservan aserciones de
contención, no repetición, recuperación del crédito, operación sana y cierre/reap.

Los tiempos usan Instant de pared monotónico; incluyen scheduling y esperas/sondeos
explícitos de los tests. Agregados de turnos separan servicio y shutdown; servicio
incluye preparación y tráfico sano. No son percentiles de latencia de producción.

Una ejecución medida por caso (N=1), `phase4-finish/integrated-green.log`:

| Caso | Intervalo observado ms | Status sano ms |
|---|---:|---:|
| Plugin lento | Timeout 2.002,171 | 0,278 |
| Plugin cae | Solicitud → unavailable 1,490 | 0,140 |
| Respuesta inválida | Solicitud → unavailable 1,500 | 0,178 |
| Cliente se va | Drop → crédito de transporte liberado 10,086 | 0,246 |
| Capacidad ocupada | Tercera consulta → rechazo 0,144 | 0,204 |

En el caso lento, Status + Run + Query sanos tardaron conjuntamente 1,144 ms.
El slot tras respuesta tardía se observó reutilizable a 3.301,719 ms desde la
solicitud; saturación recuperó su slot a 3.007,913 ms desde las dos consultas.
Son límites/esperas del fixture y sondeo (10 ms para detectar crédito), no trabajo
continuo del runtime. Los cinco cierres dieron `shutdown_complete` y reap propio,
en 10,577–13,900 ms observados. La suite crítica completa pasó 17/17.

### Reader de métricas retenido dentro del runtime real

La conformidad `target/conformance/metrics-20261023T140811-432683/` añade una ruta
auténticamente integrada, con **backend sintético**, sin sensores: binario runtime,
cliente tipado, Host, plugin oficial/SDK y un único reader retenido. Instrumentación
pasiva opt-in `RAD_PROFILE_INTEGRATED_METRICS=1`, sin cambiar RPC ni aserciones.
Raw original `conformance.log`; extracto exacto en
`phase4-finish/integrated-metrics-real.jsonl`. Los archivos `.mock` preparatorios
son dobles de test, no mediciones ni datos de estas tablas.

Una transición por caso, escaneo de 32 visuales y sondeo de 25 ms incluidos:
inicial current 1,043 s; missing→unavailable 1,018 s; recuperación→current 0,992 s;
block→stale 1,398 s. Con los 32 visuales stale se comprobaron seis operaciones
sanas: Status p50/máx. 0,223/0,306 ms; Wait0 con resultado observado
0,378/0,476 ms. El contador del reader se mantuvo **5→5**, sin nuevas lecturas.
Runtime y cliente terminaron con código 0 y wait comprobado (1,987 ms observado);
esta instrumentación **no enumera independientemente los descendientes del plugin**
ni mide turnos del Server. No convertirla en evidencia de esos extremos.

No equivalen a un fallo de driver ni a toda la GUI. Downloader/conversor retenidos
y escritura incierta mantienen su alcance previamente declarado, sin reclasificar
una composición de componentes como Server integrado.

## Métricas con lectores reales

Corrida autorizada `target/performance/m-real1/`: plugin de producción con lectores
procfs y NVML reales, **sin acciones**, GUI, XL ni servicios personales. Tres
repeticiones de instalado sin demanda → un visual CPU → 32 visuales CPU/RAM/GPU/VRAM
→ retiro → reinicio fresco. Cinco segundos por fase, 390 muestras de recursos.
En cada ventana de 32 visuales hubo cinco observaciones de las cuatro fuentes
current/válidas, con datos actualizados; sin huecos de evidencia de GPU/VRAM.

Rangos entre repeticiones, MiB; `runtime_tree` incluye runtime + helper + plugin
cuando existe, no el controlador:

| Fase | RSS árbol mediana | PSS árbol mediana | RSS plugin pico muestreado | PSS plugin pico muestreado |
|---|---:|---:|---:|---:|
| Instalado, sin demanda | 17,742–17,941 | 9,993–10,070 | Sin proceso | Sin proceso |
| Un visual CPU | 21,438–21,980 | 11,816–12,113 | 2,977–3,398 | 1,059–1,480 |
| 32 visuales, cuatro fuentes | 47,094–47,590 | 34,739–35,021 | **28,496–28,855** | **23,877–24,236** |
| Última demanda retirada | 18,613–18,750 | 10,801–10,926 | Sin proceso | Sin proceso |
| Reinicio fresco sin demanda | 17,758–17,914 | 9,963–10,021 | Sin proceso | Sin proceso |

**Resultado histórico frente al objetivo anterior de 20 MiB RSS: NO CUMPLIDO.**
`metrics-target.json` registra `exceeds_observed_target`, sin afirmar certificación
universal ni máximo absoluto. Los tres picos RSS fueron 30.257.152 / 29.880.320 /
29.880.320 B. No es simplemente RSS sumada de muchos procesos: se observó un solo
proceso de plugin, cuatro hilos y 15 FD en la fase de cuatro fuentes; se informa
PSS aparte. No atribuir el aumento exclusivamente a NVML: también cambian cantidad
de visuales y fuentes. Hace falta un experimento de atribución antes de optimizar.

CPU observada del plugin quedó bajo resolución; el árbol durante los 32 visuales
dio 0–0,2 % de un núcleo. Eso no prueba CPU cero. El plugin desapareció al retirar
la demanda, no hubo descendientes no observados y los tres ciclos cerraron limpios.
Durante la captura no se cambió el objetivo ni el código del plugin para obtener
un PASS. La decisión posterior del usuario se registra aparte, a continuación.

### Objetivo aprobado y proceso de evaluación (2026-10-23)

El usuario fija **≤30 MiB RSS (31.457.280 B)** para el proceso del plugin de
Métricas y sus descendientes. No es un presupuesto del runtime, del editor ni
de RAD completo. Con este criterio, los **28,496–28,855 MiB ya observados quedan
dentro del presupuesto** en las tres repeticiones; no hubo optimización ni nueva
lectura de sensores. No es certificación universal ni máximo absoluto.

El proceso usa `METRICS_RSS_TARGET_BYTES` en `scripts/performance/profile.py` como
criterio único de los controladores reales y sintéticos. Para aprobar un escenario
real siguen siendo obligatorias las comprobaciones de GPU/VRAM válidas y frescas,
árbol completo y estable, y comparación del pico RSS muestreado; ausencia de
evidencia sigue siendo `not_verified`, aunque el consumo sea menor que 30 MiB.
Las pruebas críticas cubren el límite inclusive y el rechazo un byte por encima.

Las capturas y `m-real1/metrics-target.json` originales conservan el criterio de
20 MiB usado entonces. Una reevaluación de los mismos datos, sin reescribirlos,
queda en `target/performance/budget30-validation/reevaluation.json`, con hashes de
entradas y del código evaluador y resultado `within_observed_target` a 30 MiB.

Validación del cambio de criterio: límite de 30 MiB y un byte por encima
comprobados rojo→verde; gate completo 11/11, 1.184 + 269 tests Rust, los 28 controles
Python y los 2 tests de opt-in aprobados (3 + 2 ignorados ordinarios).
Logs en `target/performance/budget30-validation/`. Sin nuevas lecturas de sensores,
ventanas ni operaciones del XL.

## Procedencia, validación y pendientes

Los controladores conservan hashes de fuentes y copias exactas de los ELF medidos;
los alias de Cargo posteriores no sustituyen esos artefactos. Preparaciones y
pilotos abortados están separados de las corridas finales. El primer piloto GUI
sin XL abortó por el dispatcher legacy de foco, se corrigió a Lua sin tocar
configuración y pasó después; no fue un fallo del producto ni una corrida física.

Para dominio y `server_options`, las copias de `phase4-finish/elf/` coinciden con
los hashes registrados al medir; el ejecutable de dominio se recuperó del artefacto
Cargo con hash original, no de un alias reconstruido con otro grafo de features.
Para la conformidad del reader retenido, `c5-a-artifacts.json` conserva test/fixtures,
paquete y otros ELF coherentes con los artefactos Cargo preexistentes. **Límite:**
esa corrida no registró hashes pre-exec de todos los alias y Scratch ya eliminó el
paquete instalado del caso fault. Las copias recuperadas aportan consistencia,
no una atestación criptográfica retroactiva del paquete eliminado. No mezclar
su runtime con el ELF de otro grafo de features usado por métricas reales.

## Reproducción

Requiere caché y herramientas ya presentes; no instala nada. Compilar antes de
medir, nunca ejecutar builds u otros benchmarks al mismo tiempo:

```bash
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
cargo build --offline --locked --release --workspace --all-features --bins --examples
(
  export CARGO_TARGET_DIR="$PWD/target/plugins"
  cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples
)
export RAD_TEST_LAUNCHER_HELPER="$CARGO_TARGET_DIR/release/rad-launcher-helper"
export RAD_TEST_LAUNCHER_FIXTURE="$CARGO_TARGET_DIR/release/rad-launcher-fixture"
python3 -I -S -B scripts/performance/test_desktop.py
python3 -I -S -B scripts/performance/test_metrics_real.py
id="$(date -u +%m%d%H%M%S)-$$"
# Los dos controladores sin flags de ejecución no inspeccionan ni lanzan nada:
python3 -I -S -B scripts/performance/desktop.py "target/performance/d-$id"
python3 -I -S -B scripts/performance/metrics_real.py "target/performance/m-$id"
# SOLO tras autorización, control exclusivo y ventana de foco coordinada:
python3 -I -S -B scripts/performance/desktop.py "target/performance/d-$id" --run-desktop --physical
# SOLO con autorización de los lectores reales; no abre XL:
python3 -I -S -B scripts/performance/metrics_real.py "target/performance/m-$id" --run-real-metrics
# Sin hardware/sensores, datos nuevos; mantener offline/cache del ejemplo:
target/release/examples/domain_deep_profile "$PWD/target/performance/r-$id" --representative
RAD_PROFILE_INTEGRATED=1 cargo test --offline --locked --release -p rad-runtime \
  --all-features --test server_options -- --nocapture --test-threads=1
RAD_PROFILE_INTEGRATED_METRICS=1 bash scripts/conformance/metrics/run.sh
```

GUI necesita Wayland/Hyprland reales; sockets y namespaces de fixtures pueden
requerir ejecución autorizada fuera del sandbox. No cambiar servicios/configuración
personal para hacer pasar una prueba.

## Otros presupuestos propuestos, no adoptados

Como punto de discusión **solo para esta máquina y el perfil pequeño actual**:
runtime conectado sin editor PSS ≤20 MiB; árbol con editor PSS ≤150 MiB;
navegación IPC caliente p95 ≤5 ms. Son propuestas con margen sobre esta línea base,
no aceptación aprobada ni garantías para 7.000 identidades, otros drivers o
latencia contacto/píxeles. No se fija aún presupuesto de CPU/wakeups con ventanas
tan cortas. Estas propuestas siguen sin aprobarse; son independientes del
objetivo **30 MiB RSS de Métricas**, aprobado explícitamente por el usuario.

## Estado de cierre

Gate completo de AGENTS.md **11/11 PASS** después de medir, en serie y sin GUI/XL:
fmt/build release offline/locked/tests/clippy `-D warnings` de ambos workspaces y
las tres conformidades privadas. **1.184 + 269 tests aprobados**, cero fallos,
3 + 2 ignorados en las invocaciones ordinarias. Además, **28 controles críticos
Python** (7 físicos + 11 escritorio + 10 métricas) y los **2 tests críticos de
opt-in del seed** aprobados. Logs: `target/performance/finish-gate/`.

La revisión independiente verifica aislamiento, cifras, procedencia y límites;
separa método correcto del **objetivo anterior de 20 MiB incumplido**; la posterior
aceptación de 30 MiB está documentada sin alterar esa evidencia. Retirada de
exploratorias a la papelera completada, conservando evidencia y tests críticos.
Después se repitieron en serie ambas suites (**1.184 + 269**, cero fallos,
3 + 2 ignorados), los **28 controles Python** y los **2 tests de opt-in**: todos
aprobados. Logs definitivos `16-*` a `21-*` en el mismo directorio del gate.

Esta entrega completa los bloques autónomos descritos, no todas las verificaciones
originales de fase 4. El nuevo objetivo de Métricas ya queda aprobado y cumplido
en el escenario observado. Continúan pendientes: aprobar los demás presupuestos;
latencia contacto→pantalla con
instrumentación externa; trazado exacto de wakeups (sin trazador disponible ni IDs
de tracepoints legibles en la comprobación actual, sin instalar o cambiar kernel);
audio/multimedia/OBS con servicios reales autorizados; los fallos originales
todavía no compuestos con Server/GUI y representatividad fuera de estos perfiles.
No se inducen fallos reales del driver. La fase no se declara cerrada ni se
empieza UX por inferencia: el usuario decide el cierre con estas exclusiones o
encarga lo pendiente.
