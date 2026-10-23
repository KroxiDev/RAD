# Medición de RAD · Stream Deck XL físico

Prueba autorizada por el usuario tras cerrar RAD, condicionada a verificar entorno
exclusivo. UX permanece pospuesta. No se modifica configuración personal de RAD,
StreamController ni otros proyectos; todo el perfil y sus datos son temporales.

## Seguridad y alcance

Antes de abrir: un único XL compatible, identidad válida sin publicar serial,
driver `usbhid`, permisos de lectura/escritura, unidades RAD inactivas y sin
aplicaciones relevantes ni holders observados del hidraw/nodo USB concretos.
No se detienen servicios/apps ajenos por inferencia. `flock` solo protege frente a
cooperantes: el operador mantiene otros controladores cerrados y el observador
repite una verificación scoped de holders durante la prueba.

`physical.py` es **dry por defecto**, sin siquiera inspeccionar dispositivos o
servicios en ese modo. `--physical` requiere autorización y preflight. Si aparece
un escritor ajeno, cambia la topología o falla la observación, se termina únicamente
el proceso propio con pidfd, sin intentar restaurar el logo mediante nuevas
escrituras. Ese cierre se marca fallido/no restaurado, no como PASS.

El perfil usa un Server real, helper privado, datos/XDG nuevos y un único Deck
real envuelto en `DeckPort`. No hay GUI, plugins, lanzamiento de aplicaciones ni
lectores NVML/servicios personales. Brillo de prueba moderado; teclas oscuras,
navegación y Sleep/Wake, sin ráfagas de flashes ni fallos del driver inducidos.
El cierre normal devuelve el logo del dispositivo y el último brillo del perfil:
**no restaura el framebuffer ni el brillo que hubiera antes de la prueba**.

## Qué significa el tiempo medido

`Deck::Event::Report` no incluye timestamp del HID read. El punto inicial del
cronómetro es su extracción por el Server mediante el wrapper; el final es el
`Desired` esperado enviado al worker. Es **reporte consumido → proyección enviada**,
no contacto físico → acción, HID-read → acción, aceptación USB ni píxeles visibles.
`set_desired` solo publica/notifica; no tiene ACK del render o del hardware.

La instrumentación usa relay bloqueante de wake a un socket registrado en Server,
sin tick de sondeo de 10 ms en el actor. Ese relay y un cliente de control están
incluidos en recursos del fixture. El watcher externo de holders (1 Hz) añade
coste de observación fuera de los PIDs medidos. No confundir cambios de contexto
observados con despertares exactos.

El wrapper coteja estados esperados de Entrar/Volver, Sleep y Wake; descarta casos
ambiguos si se acumulan reportes antes de un paint. No asigna automáticamente un
paint a cada reporte ni infiere terminación de un Wait inmediato sin estado visible.
Las teclas presionadas/soltadas y contadores de la fase humana quedan separados de
las órdenes IPC automáticas.

## Protocolo

- Dry valida composición, estados y cierre mediante puerto falso, **sin hardware**.
- Físico: preparación/conexión, asentamiento, reposo conectado, navegación IPC
  espaciada y ventana humana acotada.
- Secuencia humana: pulsar la tecla superior izquierda doce veces (Entrar/Volver),
  aproximadamente una vez por segundo; luego la luna en tercera posición superior
  para dormir y la misma tecla para despertar, soltando después.
- Solo se acepta completo con modo correcto, cierre limpio, ≥12 navegaciones,
  Sleep y Wake observados y todas las teclas sueltas. Un timeout humano conserva
  los datos como parcial, sin certificar la parte no realizada.

## Resultados y evidencia

Corrida real **2026-10-23**, base `97e3859`, evidencia local ignorada en
`target/performance/p-xl-20261023T1256/`. No se mezclan sus cifras con los modos
dry de `physical-validation/` o `p-dry-*`.

### Exclusividad y cierre

- Preflight repetido inmediatamente antes de lanzar: un XL elegible, driver y
  permisos esperados, unidades RAD inactivas, sin procesos relevantes ni holders
  observados en sus dos nodos. No se detuvo ni modificó ninguna aplicación ajena.
- **108 comprobaciones** del guard, cero holders inesperados; una conexión y
  ninguna desconexión durante la prueba. Es observación discreta, no una garantía
  absoluta frente a un controlador no cooperante que apareciera entre checks.
- Modo `physical`, `synthetic:false`; **12 navegaciones humanas, un Sleep y un
  Wake**, todas las teclas sueltas al final, **cero casos ambiguos descartados**.
- `clean_shutdown:true`, `logo_restored:true`; postflight **cero holders** tanto
  del hidraw como del nodo USB, dispositivo liberado. No se volvió a abrir después.
- El usuario confirmó respuesta correcta y logo final visible. Es evidencia
  cualitativa, no medición óptica ni restauración del framebuffer anterior.

No se publican número de serie, puerto ni rutas locales del dispositivo. No hubo
lectura o modificación de configuración personal/StreamController. El único brillo
de salida confirmado es el **25 % configurado en el perfil de prueba**.

### Tiempos

Tiempos de software, **microsegundos**, desde los límites definidos arriba.
Nearest-rank; con n=12, p95/p99 coinciden aquí con el máximo. Los dos casos n=1
no describen distribuciones de latencia ni presupuestos del producto.

| Operación y origen | n | p50 µs | Máximo µs |
|---|---:|---:|---:|
| Navegación física: reporte consumido → Desired esperado | 12 | 101,832 | 118,102 |
| Sleep físico: reporte consumido → Desired asleep | 1 | 87,104 | 87,104 |
| Wake físico: reporte consumido → Desired awake | 1 | 35,276 | 35,276 |
| Navegación IPC: respuesta de ejecución observada por cliente | 12 | 300,235 | 347,344 |

La fila IPC no es una pulsación física ni tiene el mismo punto de partida que las
otras filas. Los ≈0,102 ms de la primera **no** son contacto → acción ni retardo de
pantalla. La cola del worker y el relay antes del dequeue no están incluidos.

### Recursos conectado

Fixture runtime + helper, sin editor ni plugins; incluye cliente/relay instrumental
y worker real del deck. No equivale exactamente al binario de producto sin esa
instrumentación. MiB = 1.048.576 B; medianas y máximo muestreado de PSS:

| Fase | Duración observada s / muestras | RSS MiB | PSS MiB | PSS pico MiB | CPU % de un núcleo |
|---|---:|---:|---:|---:|---:|
| Reposo conectado | 19,556 / 73 | 20,750 | 12,895 | 12,895 | 0 observado |
| Navegación IPC | 4,128 / 17 | 21,023 | 13,168 | 13,215 | 0,242 |
| Ventana humana, incluida espera | 92,472 / 336 | 21,070 | 13,214 | 13,249 | 0,011 |

Los tres grupos mantuvieron **dos procesos y diez hilos**. FD observados: 38 en
reposo/IPC, 36 en la fase humana. Memoria privada
mediana: 12,805 / 13,078 / 13,125 MiB, respectivamente. RSS sumada duplica páginas
compartidas; los picos son solo máximos muestreados y las lecturas no son atómicas.

En reposo no hubo incrementos observados de ticks ni cambios de contexto; hubo un
turno de frontera de 55,214 µs en una ventana de 20 s. No demuestra CPU o wakeups
exactamente cero. En IPC se observaron un tick y 146 cambios de contexto; en la
ventana humana, un tick y 166. El máximo turno humano fue **155,614 µs** (29 turnos
instrumentados, incluidos reportes de soltar), no un límite global del runtime.

El intervalo solicitado al observador es 200 ms; la verificación scoped de holders
puede retrasarlo. Se conservan timestamps reales: no inferir 5 Hz exactos ni picos
inferiores al intervalo. No se corrieron builds ni otros perfiles a la vez.

### Procedencia

`metadata.json` registra hashes de las tres fuentes (`physical_profile.rs`,
`physical.py`, `profile.py`) y de los dos ELF realmente usados. Se verificaron sin
cambios al terminar; las copias `measured-executable` y `measured-launcher-helper`
preservan esos ELF aunque un gate posterior reconstruya los alias de Cargo.
`observer/events.jsonl`, `samples.jsonl`, `guards.json`, `timings.json` y
`user-observation.json` conservan las evidencias y sus límites.

La primera invocación física se detuvo **en preflight, sin crear raíz ni abrir el
deck**: el `fuser` instalado no acepta el separador `--`. Se corrigió sin relajar
la verificación y se vio rojo→verde con un archivo propio abierto/cerrado (no con
hardware). Los modos dry y SIGTERM se comprobaron antes de la apertura real.

## Reproducción y pruebas críticas

```bash
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
cargo build --offline --locked --release -p rad-runtime -p rad-launcher --all-features --bins --examples
python3 -I -S -B scripts/performance/test_physical.py
id="$(date -u +%m%d%H%M%S)-$$"
python3 -I -S -B scripts/performance/physical.py "target/performance/p-dry-$id"
# Solo con autorización y entorno exclusivo verificado; cambia el panel del XL:
python3 -I -S -B scripts/performance/physical.py "target/performance/p-xl-$id" --physical
```

`test_physical.py` conserva los controles **críticos de seguridad del arnés**:
dry sin inspección, preflight antes de lanzar, fuser fail-closed sobre mocks y archivo
propio, completion/partial estricto, reap tras fallo del callback y SIGTERM en dry
sin esperar al humano. No abre ni inspecciona el deck o servicios reales. Requiere
los helpers previamente compilados; fuser y sockets privados pueden necesitar
ejecución autorizada fuera del sandbox. No se instalan herramientas por inferencia.

## Validación y pendientes

Gate completo de AGENTS.md **11/11 PASS** después de cerrar/liberar el dispositivo:
fmt, build release offline/locked, tests y clippy `-D warnings` de ambos workspaces
y las tres conformidades privadas. Logs: `target/performance/physical-gate/`.
No se volvió a operar hardware durante ese gate.

Se conservaron las siete pruebas críticas del arnés y pasaron. Tras mover los
temporales exploratorios a la papelera, se repitieron en serie ambas suites y las
siete pruebas: **1184 + 269 tests Rust aprobados**, cero fallos (3 + 2 ignorados),
y **7/7 controles críticos aprobados**, sin hardware. La revisión independiente
verifica el alcance, la procedencia, los límites de las cifras y la seguridad.

Esta prueba no cierra GUI/compositor, lectores/servicios reales de plugins (incluido
NVML y el objetivo con GPU), trazado exacto de wakeups ni latencia contacto/píxeles.
Tampoco prueba otros tamaños de configuración, cables, unidades o distribuciones.
UX permanece pospuesta; no se adoptaron optimizaciones ni presupuestos nuevos.

La [continuación posterior](performance-desktop.md) ya mide el editor real con XL,
cargas adicionales y métricas con procfs/NVML reales (objetivo de 20 MiB incumplido
en el perfil observado). No cambia los límites ni las cifras de este primer ensayo.
