# Medición de RAD · continuación autónoma

Continúa la [primera línea base](performance.md), base `806f26a`. UX sigue pospuesta.
Esta entrega mide; no adopta optimizaciones, modifica límites ni autoriza hardware.
El [cierre posterior de fase 4](roadmap.md#fase-4-medicion) fue aprobado por el
usuario con presupuestos y exclusiones explícitos; los alcances y pendientes de
esta entrega siguen históricos. UX permanece en otra fase, sin iniciar.
Antes de operar el Stream Deck físico se avisa al usuario y se requiere control
exclusivo. No se ha usado hardware, ventanas, servicios ni sensores personales.
El [ensayo físico posterior](performance-physical.md) tiene autorización separada;
no se mezclan sus resultados con esta entrega sintética.
La [continuación de escritorio y métricas reales](performance-desktop.md) añade
escenarios posteriores, incluido el incumplimiento del objetivo original de
20 MiB con GPU y la aprobación posterior del usuario de 30 MiB RSS, que el
escenario real sí cumple. Las cifras y criterios de esta entrega siguen históricos.

## Bloques y método

### M1 y M2 · dominio y medios

`domain_deep_profile` usa APIs públicas y configuraciones sintéticas escaladas.
El wrapper `Authority::installed_types([])` permite observar la reconstrucción de
proyecciones con el pequeño overhead de asignar una lista vacía. Se distinguen
clonación, validación, edición, consultas y navegación, admisión, persistencia y
publicación. Comparar tiempos no es atribuir automáticamente un resto a una
función privada. La ruta de publicación puede reconstruir proyecciones más de una
vez; los resultados deben leerse con el camino medido, no como un perfil de CPU.

Los catálogos incluyen medios compartidos y únicos, cantidad y dimensiones
variables, con RGBA sintético de alta entropía, no iconos típicos. Normalización,
digest/validación/lectura y commit con medios ya presentes
o resuministrados se presentan separados. La creación inicial del Store es setup
fuera del timer: no se ofrece una cifra de primera inserción. Se conservan checksums, durabilidad y
reapertura, sin vaciar cachés ni eludir los límites del documento.

### Memoria y oficiales

Preparación de paquetes/configuración fuera del PID del runtime que se mide.
Runtime de producto fresco con máquina explícitamente deshabilitada y XDG nuevo;
se compara instalado sin demanda, un visual, muchos visuales, retirada y reinicio.
Los procesos auxiliares falsos se identifican aparte del plugin y del runtime.

Se usan las implementaciones oficiales con adaptadores de prueba: pactl falso
con aislamiento fail-closed, bus multimedia privado y reproductor falso, OBS en
loopback privado y lector de métricas determinista. Ninguna medida sintética
certifica un driver NVIDIA real ni transforma el objetivo de 20 MiB en cumplido
para todas las máquinas. No se compara como mejora contra un caso que sí use GPU.

### Fallos controlados

Los seams privados se observan mediante instrumentación pasiva exclusiva de
`cfg(test)` en pruebas críticas existentes; el flag propio no activa nada en el
producto. Otros componentes usan ejemplos del paquete dueño. Se declaran los
relojes ficticios del dominio y el tiempo real del observador por separado.
La respuesta de un componente sano no se presentará como Server integrado cuando
el experimento solo componga componentes aislados.

### M4 · variantes de build

`scripts/performance/builds.py` separa preparación de variantes y medición. Compara
release actual, una copia con `strip --strip-all` y thin-LTO, sin editar perfiles en
Cargo.toml ni reemplazar binarios instalados del usuario. Thin-LTO se compila en un
target separado en disco, offline/locked/jobs2; el resto reutiliza la caché.
El tiempo de build incluye estados de caché distintos y **no** es un benchmark del
compilador. Los ELF copiados se verifican por SHA-256 antes de medir.

Arranque observado = spawn del runtime → primera respuesta `status` con deck
deshabilitado, incluyendo un sondeo corto del socket y el cliente tipado de prueba.
No es una instrumentación interna del arranque ni implica caché de disco fría.
Se separa documento nuevo de reabrir el mismo documento; el orden de las variantes
rota entre repeticiones. El cliente se retira antes de la ventana de memoria.
Las lecturas de recursos se limitan al árbol propio con SID/starttime y pidfd para
cleanup. Se conserva el criterio de PSS/RSS, picos muestreados y CPU de la línea base.

## Reproducción y evidencia

Desde la raíz, con la caché ya disponible, sin builds/cargas paralelas:

```bash
export CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
cargo build --offline --locked --release --workspace --all-features --bins --examples
(
  export CARGO_TARGET_DIR="$PWD/target/plugins"
  cargo build --manifest-path plugins/Cargo.toml --offline --locked --release --workspace --all-features --bins --examples
)
# Cada salida debe ser nueva; mantener corto el nombre por los sockets Unix.
id="$(date -u +%m%d%H%M%S)-$$"
bash scripts/performance/faults.sh --seconds 3 --repetitions 3
# Fijar también la selección de paquetes del cliente/seed medidos.
cargo build --offline --locked --release -p rad-runtime -p rad-launcher --all-features --bins --examples
python3 -I -S -B scripts/performance/builds.py "target/performance/m4-$id" --stage build
target/release/examples/domain_deep_profile "$PWD/target/performance/d-$id" > "target/performance/d-$id.jsonl"
python3 -I -S -B scripts/performance/officials.py "target/performance/o-$id" --seconds 5 --repetitions 3
python3 -I -S -B scripts/performance/builds.py "target/performance/m4-$id" --stage measure --repetitions 12
```

Se requieren las mismas herramientas ya usadas en conformidad (`bwrap`, D-Bus privado, Python estándar,
Cargo) y, para M4, `strip`. No instalar nada por inferencia. Las variantes M4 se
preparan antes de sus muestras; su metadata también verifica el cliente tipado.
Las restricciones de sockets/namespaces pueden requerir ejecución autorizada
fuera del sandbox. Un fallo o una identidad no observable no se convierte en PASS.

Corrida final 2026-10-23, base `806f26a`, misma plataforma de la primera línea base:

| Bloque | Evidencia local ignorada |
|---|---|
| Dominio y manifiesto de 14 fuentes | `target/performance/e-20261023T1150/` |
| Oficiales y demanda | `target/performance/o-20261023T1150/` |
| Fallos | `target/performance/faults-20261023T115032-153201/` |
| Variantes de build | `target/performance/m4-20261023T1150/` |

El manifiesto `source-sha256.txt`, verificado al terminar todas las muestras, tiene
SHA-256 `1f9acad69307ab1a64246ca5dda93ccd4a43ebc74883f4cf1f15b82103c3a856`.
Cada controlador conserva metadata/hashes de fuentes y ELF; M4 incluye el cliente
tipado. Los directorios de validación y `m4-20261023T1135` son preparación, no datos
de estas tablas. No hubo builds ni otras cargas de prueba simultáneas a las muestras.

El gate posterior recompiló los alias `target/release/examples/conformance_client`
y `official_profile_seed` con otra selección de paquetes/features. No confundirlos
con los ELF medidos: sus copias exactas conservadas desde la caché están en
`e-20261023T1150/measured-clients/`. Cliente: SHA-256
`857727491f23bcb99f695c0d4accc619da8b6681df72b4b32825de9019e31615`, 3.518.984 B;
seed: `c67a17411790a2cec81ad587a0e7296e4f1c8b3de7dc7d4b6e13f20b6120fed6`,
1.467.720 B. Metadata y preflight identifican los artefactos efectivamente usados,
no prometen que un alias reconstruido después tenga el mismo hash. Para repetir,
preparar una salida nueva y registrar sus propios hashes.

## Resultados

Los datos de validación de desarrollo no se mezclan con los resultados siguientes.
Los percentiles son nearest-rank, sin interpolación; no sumar percentiles de
operaciones distintas para fabricar una latencia extremo a extremo.

### M1 · coste público por tamaño

Corrida final `target/performance/e-20261023T1150/domain.jsonl`: **770 muestras en
77 grupos**, diez repeticiones por grupo, reapertura durable confirmada. Tabla de
p50 en ms, sin medios:

| Operación | 32 identidades | 1.000 | 7.000 |
|---|---:|---:|---:|
| Clonar candidato | 0,002 | 0,035 | 0,301 |
| Validar configuración ya poseída | 0,006 | 0,310 | 10,494 |
| Preparar edición | 0,015 | 0,781 | 22,309 |
| Refresh vía wrapper público | 0,012 | 0,649 | 23,188 |
| Navegación Authority | 0,011 | 0,638 | 22,916 |
| Admitir mutación | 0,049 | 1,393 | 26,277 |
| Publicar tras commit | 0,028 | 1,465 | 47,097 |

En 7.000, el máximo del refresh fue 25,921 ms y el de publicación, 49,756 ms.
El coste de clonación no explica por sí solo las decenas de ms. El refresh aislado
por la API pública es una señal fuerte para M1; la relación publicación ≈ dos
refreshes es coherente con el camino del código, no una atribución exacta obtenida
restando medianas. Antes de introducir índices o cambiar contratos, investigar
ese camino y conservar coherencia de vistas y sincronización de entrada.

### M2 · catálogos y persistencia

Todas las filas siguientes mantienen 7.000 identidades. PNG normalizados
deterministas de distintos colores; tamaños de medios, no tamaño del documento.
Diez muestras por operación; **p50 / máximo**, ms.

| Catálogo | Bytes de medios | Commit existente | Commit resuministrado |
|---|---:|---:|---:|
| 1 compartido, 256×256 | 262.488 | 87,647 / 92,448 | 88,838 / 102,128 |
| 16 compartidos, 256×256 | 4.199.808 | 117,876 / 121,598 | 143,586 / 156,585 |
| 16 únicos, 256×256 | 4.199.808 | 117,054 / 122,422 | 146,613 / 150,202 |
| 128 únicos, 64×64 | 2.114.048 | 108,238 / 112,794 | 120,791 / 127,437 |
| 128 únicos, 256×256 | 33.598.464 | 321,676 / 338,222 | 549,021 / 563,477 |

En el catálogo mayor, digest p50 **17,068 ms**, validación **64,724 ms**,
normalización **64,039 ms** y carga Store **102,781 ms**. No son partes aditivas
independientes de cada commit: son operaciones públicas cronometradas aparte.
Compartidos significa 362 botones con imagen; únicos, un botón por imagen.
El documento mayor del perfil sigue bajo 1 MiB; no se ampliaron límites.

Sin medios, commit p50 fue **25,976 / 31,360 / 88,020 ms** para 32/1.000/7.000
identidades. La diferencia con los ≈5 ms de la primera línea base pequeña no
demuestra una regresión: no cambió el producto y son corridas con condiciones de
almacenamiento/caché distintas. Tampoco el máximo 142,436 ms observado en 1.000
identidades constituye por sí solo un límite estadístico del producto.
Store trabaja en un worker en el producto: su latencia durable no equivale a ese
tiempo bloqueando el bucle de autoridad. No quitar sync/checksums para reducirla.

### Memoria, demanda y oficiales

Tres repeticiones por plugin; cinco ventanas de 5 s (instalado, un visual, 32
visuales, retirado, reinicio fresco). **60 fases, 330 grupos**. Los doce ciclos
terminaron limpios; cero descendientes omitidos por SID. `product_tree` incluye
runtime, helper y plugin cuando existe; excluye backends falsos/supervisor, que
constan por separado. Rangos de medianas por repetición, **PSS MiB**:

| Plugin / backend falso | Instalado | 1 visual | 32 visuales | Retirado | Fresco |
|---|---:|---:|---:|---:|---:|
| Audio / pactl aislado | 9,808–10,010 | 11,491–11,983 | 11,886–12,319 | 10,601–10,791 | 9,825–10,112 |
| Multimedia / MPRIS privado | 9,808–9,985 | 12,469–12,870 | 12,824–13,226 | 10,601–10,778 | 9,785–10,075 |
| OBS / loopback | 9,914–10,093 | 12,073–12,157 | 12,468–12,548 | 10,762–10,878 | 9,830–10,001 |
| Métricas / lector determinista | 9,738–9,960 | 11,781–12,004 | 11,957–12,180 | 10,602–10,761 | 9,743–9,935 |

En todos los casos hay dos procesos de producto sin demanda y tres al activarse:
un solo proceso de plugin también con 32 visuales. Al retirar la última demanda
el plugin desaparece. El residual de memoria del runtime tras ediciones no prueba
una fuga; el reinicio vuelve cerca del instalado. Tampoco se atribuye ese residual
automáticamente al allocator sin una medición adicional de asignaciones.

Mayor **RSS muestreada solo del plugin** con 32 visuales: audio **3,383 MiB**,
multimedia **8,395 MiB**, OBS **3,680 MiB**, métricas **3,438 MiB**. Los procesos
falsos y bibliotecas compartidas hacen que RSS sumada y PSS tengan usos distintos;
no comparar estos números como si incluyeran exactamente las mismas páginas.

Con 32 visuales, CPU observada del árbol de producto fue 0,400 % de un núcleo para
OBS, y sin incrementos de ticks en los otros tres. Ventanas cortas y resolución
del contador impiden afirmar CPU cero. El backend OBS falso también trabaja:
el total observado de su escenario fue 1,0–1,2 %, **no imputable todo al plugin**.
Los datos brutos separan backend, bus privado, reproductor, pactl y supervisor.

`metrics-target.json` registra seis picos activos de RSS del fixture entre
**3,168–3,438 MiB**, por debajo de 20 MiB **solo en ese perfil sintético**. Conserva
`product_target_certified:false`: sin lector proc/NVML real no se certifica el
objetivo del producto con GPU ni se anuncia una mejora de memoria real.

### C4 · fallos controlados

Corrida final `target/performance/faults-20261023T115032-153201`: tres repeticiones,
18 grupos de recursos estables, 27 snapshots declarados de proceso propio (no
árbol) y nueve tests exactos verdes. Host/worker: seis cierres completos limpios.
Las transiciones tienen n=3: p50/máximo no caracterizan una cola de producción.

| Caso / medida | p50 ms | Máximo ms |
|---|---:|---:|
| Host sano antes del peer sin lector | 0,037 | 0,060 |
| Host sano durante saturación del peer sin lector | 0,044 | 0,140 |
| Host sano después | 0,037 | 0,058 |
| Retirar peer sin lector y reap | 0,239 | 0,250 |
| Worker sano mientras otro murió | 0,015 | 0,029 |
| Detectar worker muerto | 0,047 | 0,055 |
| Reemplazar worker | 0,062 | 0,062 |
| Arranque frío de otro plugin con downloader retenido | 4,330 | 4,335 |
| Status durante downloader retenido | 0,023 | 0,042 |
| Asentarse al liberar downloader | 55,220 | 62,392 |
| Respuesta de texto con conversor retenido | 0,079 | 0,082 |
| Commit con retorno incierto inyectado | 26,455 | 28,899 |
| Rechazo mientras Store está incierto | 0,000260 | 0,000280 |
| Reconciliación durable | 24,354 | 42,590 |

Las sondas sanas de host/worker suman 180 muestras por fase y se ejecutan cada
50 ms: su trabajo está incluido, no es reposo sin sondeo. Los deltas de ticks
observados fueron cero a esa granularidad, no CPU garantizada cero.

Límites: Host comparte owner real con un peer sano, no Server integrado; Worker
usa el módulo real sin App ni GUI y no repite el trabajo viejo; downloader usa
reloj de dominio fijo y poll del fixture de 1 ms. Conversor usa un rendezvous
retenido, **sin decode aceptado**: no se colgó un codec ni un driver real. Store usa
SQLite/VFS real con fallo de retorno inyectado y reconciliación. Los tres tests
cortos solo aportan snapshots before/during/after de sí mismos, no picos de árbol
ni CPU en steady state. Se conservaron sus aserciones de regresión originales.

### M4 · tamaño, arranque y memoria

Doce repeticiones por variante y tipo de documento, **72 arranques privados**,
todos con runtime+helper estables y cierre limpio. ELF en disco, bytes:

| Variante | Runtime | Helper |
|---|---:|---:|
| Referencia release | 20.679.640 | 1.584.592 |
| `strip --strip-all` sobre copias | 16.155.392 | 1.197.728 |
| Thin-LTO | 20.355.256 | 1.370.832 |

Arranque observado **p50 / máximo ms** y mediana de las medianas de PSS del árbol
en las ventanas posteriores, MiB (cliente tipado ya retirado):

| Variante | Documento nuevo | Documento existente | PSS nuevo / existente |
|---|---:|---:|---:|
| Referencia | 53,456 / 73,181 | 28,693 / 31,241 | 9,812 / 9,565 |
| Strip | 53,271 / 68,794 | 28,820 / 31,910 | 9,514 / 9,438 |
| Thin-LTO | 53,074 / 67,566 | 30,233 / 38,977 | 9,639 / 9,822 |

Las ventanas PSS se solapan entre variantes (aproximadamente 9,2–10,0 MiB).
Strip reduce disco de forma clara, **no demuestra una reducción proporcional de
memoria residente ni una mejora de arranque**. Las diferencias de arranque entre
variantes son pequeñas frente a máximos y condiciones de I/O; no se declara ganador
de rendimiento ni se adopta un perfil de compilación. Helper y runtime tienen sus
SHA separados en metadata, además del cliente tipado común de 3.518.984 bytes.

## Validación y lectura de la entrega

Los seis bloques autónomos tienen medición dentro de los alcances anteriores,
no certificación universal. La señal más clara para profundizar es M1: refresh y
publicación con muchas identidades; M2 crece con cantidad/bytes de medios. Los
oficiales recuperan su estado sin demanda; no se reproduce el gran sesgo de seed
del primer arnés de ecos. Los fallos contenidos no impiden el trabajo sano de los
componentes medidos, sin extrapolarlo a toda la GUI o a un driver bloqueado.

No se cambió comportamiento de producto, contratos, dependencias, límites ni UX;
la instrumentación de los tres tests existentes solo se activa por flag propio
bajo `cfg(test)`. No se añadieron pruebas permanentes no críticas. Presupuestos y
optimizaciones quedaron pendientes de decisión al publicar esta entrega: estos
datos son su evidencia, no una autorización implícita para eliminar garantías o
introducir índices. El cierre posterior aprueba criterios de referencia, no
optimizaciones nuevas.

Gate completo de AGENTS.md **11/11 PASS**, ejecutado en serie después de medir:
fmt, build release offline/locked y clippy `-D warnings` de ambos workspaces;
**1.184 + 269 tests pasados**, 3 + 2 ignorados en las invocaciones ordinarias;
conformidades audio/media, OBS y métricas verdes con fixtures privados. Logs en
`target/performance/extended-gate/`. No se operó hardware ni ventanas.

Revisión independiente de estándares y C1–C7 **sin bloqueantes**, conforme dentro
de los alcances medidos. Los ocho tests temporales y el plan se enviaron a la
papelera, conservando todos los logs/resultados. Ambas suites definitivas se
repitieron y pasaron después de esa retirada. Los tests críticos existentes y sus
aserciones se conservaron. La aceptación de esta entrega no cierra los escenarios
reales aún no verificados de la fase 4.

## Límites y frontera de interacción

- Stream Deck físico: no se operó en esta entrega sintética. El ensayo posterior
  documenta su autorización y control exclusivo; no autoriza otras pruebas o
  dispositivos por inferencia.
- GUI real con foco/sin foco: fuera de esta entrega sin ventanas; medida después
  con XL. No equivale a medir el coste del propio compositor o memoria GPU.
- Despertares exactos: no sustituir por cambios de contexto; no instalar trazadores
  por inferencia.
- Presupuestos y optimizaciones: se pueden proponer con evidencia, no se adoptan
  silenciosamente. UX permanece pospuesta.
- Solo escenarios, tamaños y fallos efectivamente medidos permiten un veredicto;
  no extrapolar fixtures a todas las distribuciones, dispositivos o integraciones.
