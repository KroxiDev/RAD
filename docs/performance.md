# Rendimiento y medición

Referencia vigente de presupuestos, evidencia y reproducción para desarrolladores
y agentes. Los criterios se aprobaron el **2026-10-23** sobre los perfiles medidos
ese día: no son límites duros del producto ni garantías para cualquier equipo o
configuración. Medir no autoriza optimizaciones ni cambios de contratos; las reglas
de trabajo están en [AGENTS.md](../AGENTS.md) y la validación funcional en
[Pruebas y validación](testing.md).

## Presupuestos

| Escenario de referencia | Criterio aprobado | Evidencia observada |
|---|---|---|
| Runtime conectado + helper, sin editor ni plugins demandados | PSS ≤20 MiB | Pico muestreado 13,782 MiB, incluida la fase tras cerrar el editor |
| Runtime + helper + editor, sin plugins demandados | PSS ≤150 MiB | Pico muestreado 109,738 MiB |
| Navegación IPC caliente del perfil pequeño, con editor enfocado | p95 ≤5 ms por repetición | Mayor p95 observado: 0,867 ms |
| Plugin Métricas y sus descendientes, con GPU y 32 visuales | RSS ≤30 MiB (31.457.280 B) | Pico muestreado 28,855 MiB |

**Perfil de escritorio:** dos carpetas, 32 posiciones visibles, brillo 25 %,
reposo automático desactivado y ningún plugin demandado. Tres ciclos de cinco
ventanas de 8 s: runtime solo, editor enfocado, desenfocado, navegación IPC y editor
cerrado. Runtime/editor de producción, XL real, Wayland/Hyprland y eframe/Glow;
datos/XDG y bus privados, backend de servicio aislado. El árbol medido excluye
observador, cliente IPC, bus privado y ventana auxiliar de desfoco. No es una
medición de systemd o bandeja reales. Navegación a aproximadamente 4 Hz, 33 muestras
por repetición; se comprueba la carpeta esperada en cada transición.

**Perfil de Métricas:** plugin de producción con procfs/NVML reales, sin GUI ni XL;
tres ciclos de cinco ventanas de 5 s: instalado sin demanda, un visual CPU, 32
visuales CPU/RAM/GPU/VRAM, retirada y runtime nuevo sin demanda. El presupuesto es
solo del plugin y descendientes, no del runtime/helper ni de RAD completo. Para
evaluarlo se exige disponibilidad GPU/VRAM válida y fresca, árbol completo y
estable y pico RSS muestreado. Un consumo bajo con evidencia ausente sigue siendo
`not_verified`. El evaluador usa `METRICS_RSS_TARGET_BYTES` en
[profile.py](../scripts/performance/profile.py).

Los cuatro criterios se satisfacen **en esos escenarios observados**. RSS y PSS no
son intercambiables; no sumar estos objetivos para inventar un presupuesto global
con todos los plugins. CPU y wakeups son líneas base **sin umbrales numéricos
aprobados**. No hay un presupuesto extrapolable a 7.000 entidades, otros medios,
drivers, dispositivos o máquinas.

## Evidencia y límites

Las capturas de referencia son del **2026-10-23**, en Linux 7.2.8-9-omarchy,
x86_64 y 16 CPU lógicas. Cada perfil conserva método y procedencia propios en las
[fuentes inmutables](#fuentes-históricas); los resultados no se mezclan entre
corridas válidas, pilotos, intentos incompletos y fixtures sintéticos.

| Perfil | Qué demuestra la evidencia | Frontera de interpretación |
|---|---|---|
| Runtime sintético con 0/1/4 plugins eco | Tres repeticiones por cantidad; reposo/carga y admisión/terminación observadas por IPC | Ecos inertes, no oficiales. El cliente embebido y la preparación de paquetes afectan al PID instrumental; la diferencia de RAM no es coste incremental de plugins ni prueba de fuga. |
| Dominio, Store y medios | Configuraciones sintéticas de 32 a 7.000 entidades; perfiles adicionales de 256/1.000; catálogos hasta 128 imágenes y ≈32 MiB, con checksums, durabilidad y reapertura | Operaciones públicas separadas, no tiempos aditivos. El perfil concreto de 10.000 supera 1 MiB y no tiene cifra de guardado: no se ampliaron límites ni se invalidan por ello todos los documentos históricos de ese tamaño. |
| Oficiales con backends falsos | Instalado, 1/32 visuales, retirada y reinicio; un proceso de plugin por demanda, retirada observada | Audio/pactl, MPRIS, OBS y métricas deterministas no certifican los servicios o sensores reales. Supervisores y backends falsos se contabilizan aparte. |
| Variantes release, strip y thin-LTO | 72 arranques privados, tamaño ELF, arranque observado y PSS | Reducir disco no demuestra reducir proporcionalmente memoria o arranque; no se adoptó otro perfil de compilación. Las condiciones de caché impiden comparar tiempos como benchmark del compilador. |
| XL físico | 12 navegaciones humanas, Sleep/Wake, cierre y liberación; confirmación humana del logo en ese ensayo | El tiempo es **reporte consumido por Server → Desired enviado al worker**, no contacto, lectura HID, ACK USB ni píxeles. El cierre devuelve logo/brillo del perfil, no el framebuffer anterior. |
| Escritorio y Métricas reales | Los perfiles de los presupuestos anteriores; editor desaparece al cerrar y Métricas se retira sin demanda | El backend de servicio es aislado: no acredita el ciclo real de systemd/bandeja ni sus cgroups. Sin CPU del compositor ni memoria GPU. |
| Fallos controlados | 3×5 casos compuestos con Server/Transport/cliente: downloader retenido, Host sin lectura, conversor oficial retenido, Store incierto y worker Store perdido; trabajo sano y no replay | No son fallos físicos de disco/driver ni toda la GUI. El reader de métricas retenido usa backend sintético; los experimentos de componentes anteriores no se reclasifican como Server integrado. |
| Wakeups del kernel | Calibración 10/10 y seis ventanas 0/1/4 ecos, una repetición; cero eventos seleccionados en reposo | `sched_wakeup` del TID despertado, sin multiplexación, con intervalos por CPU; no un corte global simultáneo. No cubre hilos transitorios/no seleccionados, C-states, causas, wakeup→run ni oficiales. |
| Audio y multimedia reales, solo lectura | Tres ciclos por plugin, 1/32 visuales y retirada; picos de árbol a 32 visuales: audio 8,148 MiB RSS / 2,237 MiB PSS; multimedia 8,168 / 2,464 MiB | Sin acciones ni artwork personal. Audio usa allowlist; MPRIS, proxy filtrado y bus aislado. No hay presupuesto nuevo; hijos breves y cambios de vigencia entre muestras pueden escapar a la observación. |
| OBS real mediante relay, solo lectura | Tres perfiles/runtimes nuevos, relay reutilizado; 1/32 visuales de grabación/emisión, 144 observaciones current válidas y cierre sin conexiones residuales. Pico del plugin a 32 visuales: 3,480 MiB RSS / 1,748 MiB PSS | OBS 32.2.2 / obs-websocket 5.7.4. Solo GetVersion/GetRecordStatus/GetStreamStatus; no acciones, transiciones reales observadas ni autenticación directa producto→OBS. Relay/supervisor y proceso personal OBS quedan fuera del coste del plugin. |

### Cómo leer las cifras

- MiB = 1.048.576 B. Los picos son **máximos muestreados**, no máximos absolutos;
  las lecturas no son atómicas. RSS sumada cuenta páginas compartidas varias veces.
  Los perfiles base, XL y servicios solicitan intervalos de 200 ms; escritorio,
  250 ms. Están sujetos al coste del observador; mandan sus timestamps reales.
- CPU usa un núcleo = 100 % y ticks con resolución `1/CLK_TCK`. Cero observado
  no significa coste nulo. Los cambios de contexto son otra medida, no wakeups
  exactos; estos últimos solo los aporta el perfil de contadores del kernel.
- Los percentiles son nearest-rank, por repetición, sin sumar percentiles de
  operaciones distintas. En grupos de 10 o 12 muestras, p95 coincide con el máximo:
  no caracteriza una cola de producción. «Frío» significa primera demanda con
  proceso ausente, no caché de disco fría.
- La terminación IPC observada incluye transporte y consulta del resultado. Los
  tiempos de Store medidos directamente no equivalen a bloquear el event loop:
  el producto realiza la persistencia en su worker. Los costes de proyección y
  medios justifican investigar, no quitar sync/checksums ni atribuir causas por
  resta de medianas.
- La disponibilidad visual se observa de forma discreta. En OBS se exige además
  progreso de lecturas, conexiones/autenticaciones estables y demanda coherente
  en ambos extremos de cada ventana: un `current` cacheado no basta.

### Exclusiones y procedencia

Quedan fuera la latencia completa contacto→acción/pantalla/píxeles, CPU del
compositor, memoria GPU, representatividad en otras máquinas/drivers/cargas,
acciones y autenticación directa de OBS, y fallos físicos de disco o drivers.
Tampoco se certifica toda la composición Server/GUI. **Una exclusión aceptada no es
una verificación aprobada**, ni la suite funcional sustituye una medición real.

Los ELF medidos y las fuentes se identifican por las huellas de cada captura;
recompilar un alias de Cargo con otro grafo de features puede cambiarlo sin
cambiar fuentes. El commit que conserva un informe no identifica por sí solo los
binarios que midió. Los informes señalan límites de procedencia cuando faltan
huellas pre-exec; recuperar después una copia compatible no los subsana
retroactivamente. Las rutas de resultados bajo `target/performance/` son ignoradas:
**Git conserva los informes, no garantiza la disponibilidad local de sus raw o
copias de ELF**.

La evaluación vigente de Métricas usa 30 MiB RSS; los artefactos originales que
registran 20 MiB permanecen sin reescribir. La reevaluación conserva los mismos
datos, no una optimización ni otra captura. Los campos protegidos del relay OBS
(PSS, privada, FD y cambios de contexto) permanecen no verificados/null, nunca cero;
la protección de credenciales no se elude para medirlos.

## Reproducción

Medir de nuevo requiere un encargo. Los controladores están en
[`scripts/performance/`](../scripts/performance/); compilar antes según
[Pruebas y validación](testing.md#validación-completa) y no ejecutar builds ni
benchmarks concurrentes. El arnés sintético base solo crea procesos y datos
propios, sin ventanas, hardware ni servicios personales:

```bash
bash scripts/performance/run.sh --seconds 15 --repetitions 3
```

Los perfiles con acceso real (`physical.py`, `desktop.py`, `metrics_real.py`,
`wakeups.py`, `services_real.py` y `obs_real.py`) necesitan además autorización
específica y su flag de ejecución real; sin él no tocan el entorno personal.
Las recetas, los gates de aislamiento y los criterios para aceptar una captura
están en las [fuentes históricas](#fuentes-históricas): antes de repetir una,
cotejarla con el controlador actual. Una captura fallida o incompleta queda como
no verificada, nunca como cero ni PASS.

## Fuentes históricas

El detalle se consulta en Git, sin mantener una crónica paralela en el árbol.
La revisión **`30eb0e831c5873df944a6cf1ec7ea37a5fd39191` conserva los documentos**;
no es la versión común de todos los binarios medidos. Cada informe identifica su
base, fuentes, ELF, recetas, resultados y limitaciones:

- [Línea sintética](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance.md): runtime/eco, dominio, recursos y método base.
- [Perfil extendido](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance-extended.md): medios, oficiales falsos, fallos de componentes y variantes de build.
- [XL físico](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance-physical.md): protocolo humano, frontera temporal y exclusividad.
- [Escritorio y Métricas](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance-desktop.md): perfiles de los cuatro presupuestos, cargas y fallos integrados.
- [Fallos, kernel y servicios](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance-services.md): fallos compuestos, wakeups y audio/multimedia reales.
- [OBS de solo lectura](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/performance-obs.md): relay, gates, captura válida y límites de procedencia.

También pueden leerse sin acceso remoto, por ejemplo:

```bash
git show 30eb0e831c5873df944a6cf1ec7ea37a5fd39191:docs/performance-obs.md
```
