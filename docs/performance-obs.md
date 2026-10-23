# Medición de RAD · OBS real en solo lectura

Continuación de [fallos, servicios y kernel](performance-services.md), sobre base
`f9e14e6`. El usuario abrió OBS y confirmó WebSocket en puerto 4455 con
autenticación activa. Alcance: recursos, disponibilidad y ciclo de demanda del
plugin oficial, **sin ejecutar acciones** de OBS. UX sigue pospuesta y no se
adoptan presupuestos nuevos; Métricas conserva su objetivo independiente de
**≤30 MiB RSS**.

## Seguridad y alcance del enlace

- Runtime/helper y plugin **de producción**, con perfil nuevo privado, sin editor,
  XL, autostart ni datos personales de RAD/StreamController. El seed solo selecciona
  el binario oficial con `obs --real-obs`; sus defaults sintéticos se conservan.
- El plugin usa un puerto efímero del guard, comprobado mediante API antes de la
  primera demanda y en cada fase. Solo se permiten visuales `record` y `stream`;
  no acciones, opciones, escenas, fuentes ni credenciales personales del producto.
- El relay conecta exclusivamente a **127.0.0.1:4455**, sin redirects ni fallback.
  Reconstruye solo `GetVersion`, `GetRecordStatus` y `GetStreamStatus`, sin argumentos
  adicionales. Rechaza escrituras, batches, operaciones desconocidas y argumentos
  no vacíos **antes del upstream**, también en una sesión ya autenticada.
- Autenticación conforme al [protocolo oficial de obs-websocket 5.x](https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md).
  La contraseña real entra mediante Zenity directamente en el relay Rust, nunca
  en chat, Python, argv, entorno, Vault del runtime o archivos de resultados.
  Permanece en RAM con Zeroizing, CORE=0 y DUMPABLE=0. Eso no certifica el borrado
  físico de todas las copias internas de las bibliotecas o del escritorio.
- El producto se autentica contra un **token efímero aleatorio** del relay,
  transmitido únicamente por pipes y guardado en el Vault del perfil de prueba,
  inútil al cerrar el relay. **No mide autenticación directa producto→OBS**: la
  autenticación real pertenece al relay y su coste se separa del plugin.
- El relay suscribe solo Outputs (64), no la máscara 239 del producto. Solo
  entrega eventos de grabación/emisión con datos vacíos y respuestas de estado
  con booleans; descarta outputPath, nombres y diagnósticos. No se guardan frames
  crudos, títulos, colecciones, rutas de grabaciones, claves de emisión ni secretos.
  GetVersion conserva capacidades/versiones públicas reales, sin inventarlas.
- Aislamiento de archivos/PID con bwrap, **no namespace de red**: la garantía está
  acotada al código y ELF oficiales congelados y al puerto comprobado. No es un
  sandbox frente a plugins hostiles. No se muestrean procesos personales de OBS.
- EOF, identidad del padre/PDEATH, tiempos máximos y cierre explícito limitan la
  vida del relay y de su diálogo. Cancelación o error de autenticación abortan;
  no se reintenta automáticamente una contraseña ni se repite el diálogo.

## Método de medición

Tres perfiles y runtimes nuevos, con relay reutilizado entre ellos, de cinco
fases de 5 s e intervalo nominal de
200 ms: instalado sin demanda, 1 visual de grabación, 32 visuales alternando
grabación/emisión, retirada y runtime nuevo sin demanda. Preparación fuera del
runtime medido. Todas las visuales se comprueban al inicio; después se observan
hasta dos representantes por segundo. No es una prueba de frescura continua.

Se exige demanda/socket coherentes en **ambos extremos** de la ventana, misma
cuenta de conexiones/autenticaciones sin reconexión intermedia, avance de lecturas
de la fuente activa y ausencia de lecturas sin demanda. Un estado current cacheado
por sí solo no basta para aprobar una ventana sin progreso.

RSS, PSS, privada, CPU, hilos, FD y procesos del plugin y árbol completo se
atribuyen solo a PID/starttime/SID propios y ELF verificados. Supervisor y relay
quedan separados. Por DUMPABLE=0, el relay solo expone RSS de stat, CPU e hilos:
**PSS, privada, FD y cambios de contexto quedan NO_VERIFICADOS/null, nunca cero**.
Su identidad usa PID/starttime/SID y copia/hash previos del ejecutable confiable;
no se elude la protección para cotejar `/proc/PID/exe` o leer smaps/memoria.

Los picos son muestreados, las lecturas de recursos no son atómicas y cero CPU
puede estar bajo la resolución por ticks. Cambios de contexto no son wakeups.
Los resultados corresponden al enlace **filtrado mediante relay**, no al tráfico
original completo ni a todas las versiones/configuraciones de OBS.

## Gates previos y procedencia

Antes de cualquier diálogo o conexión real, `--validate-only` compone un peer
privado y exige los 13 booleans del gate: lecturas permitidas, denegaciones
post-auth, cero efectos upstream, autenticación errónea sin upstream, Outputs
mínimos, filtrado de eventos/respuestas, versiones reales, snapshots y cierre
por finish/EOF. Después `--synthetic-only` prueba el ciclo completo del **plugin
de producción** contra el servidor privado, sin GUI ni puerto 4455.

ROOT ejecutó `ov1` y `os1`: ambos completos, hashes previos/posteriores coincidentes;
el ciclo sintético tuvo 130 muestras, 25 agregados y 48 observaciones current.
**No son resultados del OBS personal.** Se conservan aparte de la captura real.

### Intentos incompletos y corrección del arnés

El primer intento real, `or1`, terminó dos ciclos y falló preparando el tercero,
sin muestras de ese tercer ciclo. Su metadata global es **incompleta**, no 3/3.
Solo se conservó el tipo ProcessLookupError, sin ubicación: no se atribuye su
causa de forma única a la reproducción posterior ni se mezclan sus muestras
silenciosamente con las de una corrida final.

`os2`, exclusivamente sintético, reprodujo ProcessLookupError **errno=3 (ESRCH)**
con frames sanitizados: stop_runtime, después del reap, llamaba a
profile.same_owned_process para leer stat de un proceso terminado. Un FD procfs
abierto antes de terminar el proceso puede devolver ESRCH al leerlo, no ENOENT.
El test nativo abre ese FD de un sleep propio, termina/recoge el hijo y obtiene
el mismo error del kernel; otro test recorre stop_runtime con ese FD post-reap.
Ambos se observaron rojos y luego verdes.

La corrección está solo en el **sampler del arnés**, no en RAD/OBS: ese ESRCH
específico significa proceso ya no vivo. No se captura todo ProcessLookupError;
errno=None, permisos y las guardas de SID/starttime conservan su comportamiento.
Las regresiones comprueban que una identidad distinta no recibe señales.

Desde este ajuste se conservan por separado error primario y error de cleanup, el ACK del
relay y su reap, incluso en fallo. Los diagnósticos contienen únicamente tipo,
errno y archivo relativo/función/línea: nunca mensajes, líneas de código, cadenas
de excepciones o variables locales. Una captura fallida nunca se vuelve PASS.

Con la corrección, `os3` completó **tres ciclos privados**, 390 muestras, 75
agregados y 144 observaciones current, sin lecturas/conexiones sin demanda, sin
denegaciones y con cierre completo. Otra vez: son pruebas del arnés contra un
backend **falso**, no mediciones del OBS personal.

Fuentes, ELF y herramientas se copian y cotejan antes de ejecutar; el alias del
relay puede cambiar por un build con distinta unificación de features. Los gates
ROOT usan la copia SHA-256
`e44ea6d00b5721c42430b6efc567222c264eb8c03ca8d069ea292f6f0c36dcc4`,
no el ELF anterior del piloto del implementador. Los artefactos permanecen
ignorados en `target/performance/`, sin datos privados en el repositorio público.

## Resultado real válido

`or2`, independiente del intento incompleto anterior, completó **3/3 ciclos**,
390 muestras y 75 agregados. El servidor anunció **OBS 32.2.2 / obs-websocket
5.7.4**, RPC 1. **144 observaciones current válidas**, cero stale/unavailable en
las muestras; conexiones y autenticaciones estables dentro de las 15 ventanas.

Máximos de los **picos muestreados** entre las tres repeticiones, solo el árbol
del plugin oficial, sin runtime/helper, relay, supervisores o proceso personal OBS:

| Visuales | RSS MiB | PSS MiB | Privada MiB | Procesos / hilos / FD pico |
|---:|---:|---:|---:|---|
| 1 | 3,434 | 1,701 | 1,691 | 1 / 2 / 5 |
| 32 | 3,480 | 1,748 | 1,738 | 1 / 2 / 5 |

Sin demanda —instalado, retirado y runtime nuevo— se observaron **cero procesos
del plugin** en todas las repeticiones. La CPU del plugin fue 0,0 % de un núcleo
en estas ventanas, no coste nulo: conserva el límite por ticks. El árbol de
producto con 32 visuales (runtime/helper/plugin, sin supervisor/relay) alcanzó
12,703 MiB PSS muestreados y una CPU media por ventana de hasta 0,802 % de un
núcleo según diferencias de ticks. El relay observó hasta 2,863 MiB RSS de stat
y 0,201 % de CPU media por ventana; sus campos protegidos
permanecen null. No se suman a los del plugin ni se reinterpretan como ahorro.

El guard registró tres conexiones upstream autenticadas, **cero activas al
terminar**, cero denegaciones y cero errores de autenticación frontend. Pasaron
201 GetRecordStatus, 192 GetStreamStatus y 3 GetVersion. Son solicitudes de lectura reenviadas,
no acciones ni cambios de OBS. No se observaron eventos reales, por lo que no
se certifican transiciones de estado en este ensayo. Cierre/FD propios, diálogo
recogido, reap y hashes posteriores comprobados. El arnés no cambió escenas,
fuentes o configuración ni inició/detuvo grabación o emisión.

Evidencia válida: `target/performance/or2/` (metadata, copias previas, gate,
availability, resources, summary, muestras y eventos sanitizados). No se agregan
las muestras de `or1` ni las sintéticas a esta tabla. Esto completa el bloque
**OBS real de solo lectura en el perfil descrito**, no la autenticación directa
producto→OBS, las acciones mutantes, el OBS sin relay ni todas las distribuciones.

## Reproducción

Con caché/dependencias del proyecto disponibles y ambos workspaces compilados
offline en serie, sin otra medición concurrente:

```bash
# Dry: no inspección, proceso, diálogo ni red.
python3 -I -S -B scripts/performance/obs_real.py target/performance/obs-new
# Solo fixtures propios; nunca el OBS personal.
python3 -I -S -B scripts/performance/obs_real.py target/performance/obs-gate --validate-only
python3 -I -S -B scripts/performance/obs_real.py target/performance/obs-pilot --synthetic-only --repetitions 1
# Solo con autorización y entrada local de la contraseña:
python3 -I -S -B scripts/performance/obs_real.py target/performance/obs-real --run-obs --port 4455
```

Cada salida debe ser nueva y privada bajo target/performance; comprobar exit code,
metadata y disponibilidad. Una captura fallida no se convierte en PASS ni se
mezcla con corridas válidas. No cambiar OBS o quitar autenticación para hacerla pasar.

## Validación y cierre de esta entrega

Tras corregir el sampler y pasar `os3`, el gate completo de AGENTS.md se ejecutó
**antes de `or2`**, en serie, offline/release/locked/all-features: **11/11 PASS**,
fmt/build bins y ejemplos/tests/clippy `-D warnings` de ambos workspaces y las tres
conformidades privadas. **1.189 + 276 Rust** aprobados, cero fallos, 3 + 2 ignorados;
**83 controles Python** y ejemplos invocados explícitamente: **4 seed, 2 proxy
MPRIS y 6 relay OBS**, aprobados. Las cifras son por invocación, no se suman como
tests únicos. Logs `01-*` a `16-*` y summary-precapture.json en
`target/performance/obs-final-gate/`.

No hubo cambios ejecutables entre ese gate y la captura. `or2` volvió a retener
las fuentes y ELF actuales antes de ejecutar y verificó hashes después: el gate
previo no se presenta retrospectivamente como una ejecución posterior a medir.
Tras los reruns, cuatro aliases generados del workspace raíz tenían otro SHA,
sin cambios de fuentes. No sustituyen los ELF medidos: las nueve copias de `or2`
siguen coincidiendo exactamente con el inventario al terminar el gate previo y
las 22 fuentes coinciden entre gate, captura y estado final. summary-final.json
registra esas diferencias de aliases, no una falsa igualdad de todos los builds.
Una comprobación inicial demasiado estricta de los aliases actuales falló en esa
fase posterior; no fue un fallo de tests. El resumen separa fuente coincidente,
copia medida coincidente y alias actual no coincidente, con el momento de observación.

Tras auditar los resultados, los ocho directorios privados de perfiles/relay de
`or1` y `or2`, el plan y runner temporales se enviaron a la papelera; se conservan
raw sanitizado, copias de fuentes/ELF y gates. Luego se repitieron ambas suites
Rust (**1.189 + 276**, cero fallos, 3 + 2 ignorados), **83 Python** y los ejemplos
**4/2/6**, todos aprobados (`17-*` a `23-*`). Un listado auxiliar de tests en /tmp
requirió traslado con hash verificado al disco del repo antes de enviarlo a la
papelera; se retiró mientras corrían esas suites y no participa en fuentes,
build ni datos de prueba. Después de la limpieza total se repitieron **83 Python
y diff check**, también aprobados (`24-*`, `25-*`). cleanup.json y
summary-final.json conservan el orden y la incidencia parcial de Gio sin ocultarla.

Esta entrega no cambió contratos, dependencias ni comportamiento del producto,
no inició UX y no cerró automáticamente fase 4. Posteriormente el usuario aprobó
los presupuestos de referencia, aceptó las exclusiones y ordenó
[marcar completada la fase 4](roadmap.md#fase-4-medicion). Se mantiene el alcance
read-only mediante relay, sin certificar lo excluido. **UX sigue en una fase
posterior y no se inicia con este cierre.**
