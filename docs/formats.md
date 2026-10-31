# Formatos de datos

Contratos de persistencia de RAD. Los límites se aplican antes de publicar una
configuración; ningún lector interpreta nombres como rutas, órdenes o binarios.
Las decisiones están en [ADR 0002](adr/0002-identidades-sobre-lo-vigente.md),
[0003](adr/0003-copias-portables.md), [0004](adr/0004-evolucion-de-formatos.md),
[0005](adr/0005-tres-tipos-de-ajustes.md) y
[0006](adr/0006-dispositivo-por-modelo.md) y
[0017](adr/0017-accion-nativa-sleep-y-documento-v4.md) y
[0019](adr/0019-brightness-persistente-y-documento-v5.md).

## Versiones y compatibilidad

| Formato | Escritor actual | Lectores |
|---|---|---|
| Documento JSON del deck | 5 | 1, 2, 3 y 4 mediante importadores, 5 |
| Store SQLite | 2 | Migración única de 1, 2 |
| Copia RADPKG | 2 | 1, 2 |
| Preferencias del editor | 1 | 1 |
| Ajustes de máquina | 1 | 1 |

No hay downgrade ni tolerancia silenciosa a versiones futuras. Hay que actualizar
RAD para abrirlas. Un documento desconocido no se reemplaza por valores de fábrica.
El lector heredado vive únicamente en `rad-codec`; Store y restauración lo
reutilizan. El codec trabaja con bytes acotados, sin acceder al sistema de archivos.

## Documento del deck v5

JSON UTF-8 de hasta **1 MiB**; el escritor canónico es `rad_codec::encode` y el
lector es `rad_codec::decode_data`. La reconstrucción del dominio valida el árbol,
las referencias, las identidades y los valores. Los campos desconocidos se rechazan.

- `version`: `5`.
- `grid`: `[8,4]` (columnas, filas). Solo se admite el XL; guardar una rejilla no
  implementa otros modelos. `Grid::XL` reúne sus constantes en el dominio.
- `settings`: brillo, reposo y umbral de pulsación. No contiene idioma ni opciones
  propias de la máquina.
- `plugin_settings`: mapa por ID de plugin con valores no secretos opacos,
  codificados en hexadecimal, como los parámetros de acciones. El contrato de
  plugins define su esquema `settings`; la [API de ajustes](client-api.md#ajustes-y-contrato-publicado-de-plugins)
  valida los cambios explícitos contra ese esquema. El documento conserva los
  bytes guardados aunque el plugin falte o su esquema sea incompatible.
- `plugin_dependencies`: mapa por ID con `version` (`{"major":1,"minor":2,"patch":3}`
  o `null`) y `origin` (cadena o `null`). Conserva la información conocida para
  ofrecer la instalación después de restaurar, incluso si el plugin falta
  localmente. No es el registro de plugins instalados ni una autorización para
  descargar. Los mapas de plugins rechazan claves duplicadas.
- `principal`: identidad de la carpeta raíz; `global_permissions`: objeto de permisos.
- `folders`: array de carpetas; `buttons`: array de botones. No son mapas por ID.

### Campos y valores

Todos los campos anteriores y los de las tablas siguientes son obligatorios salvo
las excepciones explícitas. No se rellenan ajustes ausentes con defaults. Los
objetos rechazan campos desconocidos y campos repetidos. El escritor emite JSON
compacto; el lector no exige el orden de sus propiedades ni ausencia de espacios.

| Objeto | Campos y representación |
|---|---|
| `settings` | `brightness_percent`: número entero 10–100; `sleep_ms` y `press_threshold_ms`: cadenas decimales `u64` (0–18446744073709551615). Defaults al crear: `100`, `"300000"`, `"0"`. `long_press_threshold_ms`: cadena decimal `u64`, independiente del umbral de pulsar; omitido equivale a `"500"` y el escritor omite ese valor. Reposo cero desactiva solo el reposo automático. |
| Identidad | `{"installation":"77","serial":"1"}`: instalación `u128` (1–340282366920938463463374607431768211455), serial `u64` no nulo. |
| Binding | `{"kind":"folder","number":"1","identity":{...}}`: `kind` es `folder`, `button` o `action`; `number` es `u64` no nulo. El ShortId es la pareja clase/número (`f1`, `b1`, `a1`); la identidad es instalación/serial. |
| Permisos | `{"consult":true,"modify":true,"execute":false}`: tres booleanos; `modify` o `execute` requieren `consult`. Este ejemplo es el default global al crear. |
| Permisos locales | `"inherit"` o `{"own":{...}}` con objeto completo de permisos. Un botón hereda del global y una acción del botón; `own` reemplaza el conjunto, no lo combina. |
| Carpeta | `binding` de clase folder, `parent` identidad o `null`, `slots` array de exactamente 32 identidades de botón o `null`, en orden de posiciones 0–31. `parent` omitido se lee como `null`; el escritor siempre lo emite. |
| Botón | `binding` de clase button, `name` cadena, `appearance` objeto, `visual` variante, `permissions` locales, `mode` (`"sequential"` o `"parallel"`), `gesture` (`"press"`, `"hold"` o `"long_press"`; omitido equivale a `"press"`, que el escritor omite), `actions` array ordenado de 0–20 acciones. |
| Apariencia | `text` cadena; `image` digest SHA-256 de 64 dígitos hexadecimales minúsculos o `null` (omitido también se lee como `null`); `text_position` opcional: `"top"`, `"middle"`, `"bottom"`, `"above_image"`, `"below_image"` o `null`. Ausente/null sigue la colocación automática según la imagen; el escritor lo omite en ese caso. |
| Acción | `binding` de clase action, `kind` variante de la tabla siguiente, `permissions` locales. |
| Tipo externo | `{"plugin":"id","name":"tipo"}`: ambas cadenas no vacías y sin caracteres de control. |
| Dependencia de plugin | `version`: `null` o `{"major":1,"minor":2,"patch":3}`, cada componente número entero `u32` (0–4294967295); `origin`: cadena no vacía sin controles o `null`. Campos omitidos se leen como `null`; el escritor emite ambos. |

| Variante | JSON |
|---|---|
| Visual manual | `"manual"` |
| Visual externo | `{"external":{"type_key":{"plugin":"id","name":"tipo"},"configuration":"00ff"}}` |
| Lanzador | `{"launcher":"aplicacion.desktop"}`; ID terminado en `.desktop`, con prefijo no vacío, sin `/`, `\` ni controles; nunca una orden de shell. |
| Acción externa | `{"external":{"type_key":{"plugin":"id","name":"tipo"},"parameters":"00ff"}}` |
| Entrar en carpeta | `{"enter_folder":{"installation":"77","serial":"2"}}` |
| Volver | `"return"` |
| Suspender | `"sleep"`; activa el reposo existente, sin parámetros ni ajustes nuevos. |
| Brillo | `{"brightness":"increase"}` o `{"brightness":"decrease"}`; cambia el brillo vigente en diez puntos porcentuales, con saturación en 10–100. |
| Esperar | `{"wait":"1000"}`; milisegundos de 0 a 600000. |

Los decimales en cadenas son ASCII, sin signo, espacios ni ceros iniciales salvo
`"0"`. Los bytes opacos usan pares hexadecimales **minúsculos**, sin prefijo;
la cadena vacía representa cero bytes. Nombres y textos pueden estar vacíos.
El límite de escalar de 8.192 bytes serializados también limita parámetros y
configuración externos a 4.095 bytes decodificados. La referencia de imagen se
valida contra los medios al persistir/restaurar, no solamente al decodificar JSON.

La variante `"sleep"` solo existe desde v4. Los importadores cerrados v1, v2 y
v3 rechazan esa variante bajo una etiqueta anterior. La variante `brightness` solo
existe desde v5; los importadores v1–v4 la rechazan bajo una etiqueta anterior.
Al importar documentos v1–v4, los porcentajes válidos de brillo 0–9 pasan a 10;
valores mayores de 100 siguen siendo inválidos. Se conservan gesto, umbral,
identidades y acciones anteriores. El [golden v4](fixtures/v4/README.md) se generó
con el escritor anterior, no con el escritor v5.

### Relaciones del árbol

Hay exactamente una raíz `principal`, sin padre. Todas las demás carpetas llegan
a ella por `parent`, sin ciclos ni padres ausentes. Cada botón aparece exactamente
una vez entre todos los slots, y cada slot ocupado referencia un botón existente.
Cada carpeta no raíz tiene exactamente un botón de entrada en su padre y un botón
de retorno entre sus slots; la raíz no tiene entrada ni retorno. Una acción de
entrada apunta a una hija de la carpeta que contiene su botón. Entrada y retorno
son la única acción de su botón. Las identidades y los ShortId son únicos entre
todas las carpetas, botones y acciones; coinciden exactamente con las entradas
vigentes del registro. Las carpetas no tienen nombre ni permisos propios en este
formato: la presentación y permisos de entrada pertenecen a su botón.

Fuentes normativas de conversión y validación:
[`document.rs`](../crates/rad-codec/src/document.rs),
[`rad-codec`](../crates/rad-codec/src/lib.rs) y
[`validation.rs`](../crates/rad-core/src/configuration/validation.rs).

El documento no contiene contadores de asignación ni el token propio de la
instalación: son metadata monotónica y no pueden retroceder con el historial.
Las identidades de cada entidad sí forman parte del documento.

Una identidad se representa como `{"installation":"77","serial":"1"}` y un
binding añade `kind` (`folder`, `button` o `action`) y `number`. Los enteros grandes
son cadenas decimales sin signo; las duraciones `sleep_ms`, `press_threshold_ms` y
las acciones `wait` también se representan en milisegundos como cadenas. Los
parámetros de acciones y visuales externos son cadenas hexadecimales. Una imagen
se referencia por su digest SHA-256 hexadecimal, no por una ruta del equipo.

Los ajustes opacos admiten hasta 4.095 bytes por plugin (8.190 caracteres HEX).
Los IDs y orígenes no pueden estar vacíos ni contener controles; cada escalar JSON,
incluidas comillas y escapes, se limita a 8.192 bytes. También se aplica el límite
global del documento, no una reserva de ese tamaño para cada entrada.
Los mapas de plugins vacíos se omiten y se reconstruyen como mapas vacíos. La
rejilla compacta y la eliminación del idioma evitan que la conversión de un
documento v1 canónico válido exceda 1 MiB por el mero overhead del formato nuevo.

El límite para aumentar el contenido mediante ediciones es **7.000 entidades
vigentes**, entre carpetas, botones y acciones; no cuenta retiradas. Se eligió tras
medir un perfil sintético compacto:
5.000 entidades ocupaban 720.378 bytes en v1 y 7.000, 1.006.733 bytes (1.006.726 en
v2). No es una promesa de
que cualquier contenido quepa: textos, parámetros y demás datos siguen sujetos al
tope de 1 MiB. El límite se valida al preparar la edición, no después de guardarla.
Una configuración antigua o una copia válida que ya supere ese límite no se pierde:
se admite su reconstrucción hasta el techo anterior de 10.000 y se permiten cambios
que no aumenten su cantidad de entidades. Se pueden borrar elementos y deshacer;
no hace falta alterar el original antes de abrirlo o restaurarlo.

### Importación del documento v2

El importador cerrado de v2 conserva el contenido y asigna gesto `press` a cada
botón y umbral largo 500 ms. No acepta campos de v3 con una etiqueta v2. Los
[golden v2](fixtures/v2/README.md) provienen del escritor original; el
[golden v3](fixtures/v3/README.md) incluye selección y umbral no predeterminados.
La omisión canónica de Press y 500 ms evita aumentar documentos históricos en el
límite de 1 MiB. Semántica de planes y reservas de fin:
[ADR 0015](adr/0015-gesto-del-plan-y-documento-v3.md).

### Importación del documento v1

El importador conserva el contenido del deck, descarta `gui_language` y asigna la
rejilla XL y los espacios nuevos vacíos. Las pausas prerelease `pauses_ms` solo se
aceptan si todas valen cero; no se silencian pausas con efecto. El escritor actual nunca
vuelve a emitir ese campo. El editor usa el idioma del sistema hasta que se elige
uno explícitamente.

Los [golden originales v1](fixtures/v1/README.md) se generaron desde un perfil
sintético con el escritor de origen, antes de modificarlo; contienen un documento,
una base real con revisiones y una identidad retirada, y una copia RADPKG. No se
regeneran con el escritor nuevo.
El perfil grande, generado también con las fuentes originales, tiene 7.200
entidades y ocupa exactamente 1.048.576 bytes en v1; convertido a v2 ocupa
1.048.569. Se prueban su apertura, migración y conservación del archivo anterior.
Los [golden v2](fixtures/v2/README.md) conservan documento, Store y RADPKG con
ajustes y referencias de plugins no vacíos.

## Identidades y retención

Cada instalación conserva su token no nulo y cuatro máximos persistentes `u64`:
carpetas, botones, acciones y serial. Asignar incrementa el máximo correspondiente y
el serial local; el desbordamiento se rechaza, nunca se envuelve. Los máximos no
disminuyen con deshacer, recuperación, restauración ni recolección.

El registro conserva lo vigente y las identidades necesarias para revisiones,
historial y exportaciones retenidas. Al liberarse esas raíces se pueden podar las
retiradas, sin permitir que un ShortId vuelva a asignarse. La comparación de
registros ordenados es por mezcla y el guardado inserta solo las asignaciones nuevas.
Un agotamiento de recursos conserva su causa hasta la respuesta al cliente.

## Store SQLite v2

`application_id = 1380009025`, `user_version = 2`. Esquema completo de creación
([schema.sql](../crates/rad-store/src/schema.sql),
[recovery.sql](../crates/rad-store/src/recovery.sql),
[counters.sql](../crates/rad-store/src/counters.sql)):

```sql
CREATE TABLE identities (
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 2),
    number BLOB NOT NULL CHECK(length(number)=8 AND number != zeroblob(8)),
    installation BLOB NOT NULL CHECK(length(installation)=16 AND installation != zeroblob(16)),
    serial BLOB NOT NULL CHECK(length(serial)=8 AND serial != zeroblob(8)),
    PRIMARY KEY(kind,number), UNIQUE(installation,serial)
) STRICT;
CREATE TABLE revisions (
    id INTEGER PRIMARY KEY CHECK(id > 0),
    document BLOB NOT NULL CHECK(length(document) BETWEEN 1 AND 1048576),
    digest BLOB NOT NULL CHECK(length(digest)=32)
) STRICT;
CREATE TABLE media (
    digest BLOB PRIMARY KEY CHECK(length(digest)=32),
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 256),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 256),
    bytes BLOB NOT NULL CHECK(length(bytes) BETWEEN 1 AND 524288)
) STRICT;
CREATE TABLE revision_media (
    revision INTEGER NOT NULL REFERENCES revisions(id),
    media BLOB NOT NULL REFERENCES media(digest),
    PRIMARY KEY(revision,media)
) STRICT;
CREATE TABLE heads (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    current INTEGER NOT NULL REFERENCES revisions(id),
    previous INTEGER REFERENCES revisions(id)
) STRICT;
CREATE TABLE meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    installation BLOB NOT NULL CHECK(length(installation)=16),
    ledger_count INTEGER NOT NULL CHECK(ledger_count >= 1),
    ledger_digest BLOB NOT NULL CHECK(length(ledger_digest)=32),
    barrier INTEGER NOT NULL DEFAULT 0
) STRICT;
PRAGMA application_id=1380009025;
PRAGMA user_version=2;
CREATE TABLE recovery (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    hold INTEGER NOT NULL CHECK(hold >= 0)
) STRICT;
INSERT INTO recovery VALUES(1,0);
CREATE TABLE counters (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    folders BLOB NOT NULL CHECK(length(folders)=8),
    buttons BLOB NOT NULL CHECK(length(buttons)=8),
    actions BLOB NOT NULL CHECK(length(actions)=8),
    serial BLOB NOT NULL CHECK(length(serial)=8),
    digest BLOB NOT NULL CHECK(length(digest)=32)
) STRICT;
```

Las claves y seriales BLOB son enteros sin signo big-endian: 8 bytes para `u64`,
16 para instalación `u128`. `identities.kind` codifica folder=0, button=1, action=2.
No hay columna retired: la vigencia se reconstruye desde el documento y el resto
son bindings retenidos. `meta.installation` es el token local no nulo;
`ledger_count` es el número exacto de filas de `identities`, sin techo histórico.
`ledger_digest` es SHA-256 de `installation:u128 BE`, seguido de cada binding
ordenado por `(kind,number)`: `kind:u8 || number:u64 BE || installation:u128 BE ||
serial:u64 BE`. No se añade contador ni separador a esa concatenación.

`revisions.document` contiene los bytes JSON y `digest` su SHA-256 de 32 bytes
crudos. `media.bytes` contiene PNG normalizado, `digest` es SHA-256 de esos bytes y
`width`/`height` deben coincidir con el PNG validado; cada dimensión es 1–256.
`revision_media` relaciona una revisión con sus imágenes. Las referencias JSON a
imágenes usan la versión hexadecimal minúscula del digest. Cada configuración
admite hasta 512 medios y 128 MiB en conjunto.

`heads` tiene una fila: `current` es la revisión publicada y `previous` el respaldo
anterior o NULL. La apertura valida checksum, documento, registro y medios; si el
head actual está corrupto puede recuperar el anterior válido, sin borrar la
evidencia. `recovery.hold=0` permite la recolección normal; un valor positivo
conserva la evidencia hasta descarte explícito. `meta.barrier`, inicialmente cero,
se incrementa con una escritura real para reconciliar durabilidad incierta.

Una base que no se puede abrir nunca se borra: al reemplazarla queda como
`state.sqlite3.damaged-<ms>`, con sus `-journal`, `-wal` y `-shm` renombrados a
`state.sqlite3.damaged-<ms>-journal` (y análogos) para que un journal caliente no se
aplique a la base nueva. El reemplazo es un intercambio atómico y se niega si ya existe
una evidencia con ese sello. Para no reutilizar IDs, los contadores de la base ilegible
se leen en solo lectura (`immutable=1`: sin locks, sin tocar sidecars ni escribir) y sin
exigir el `digest`: se toma el máximo entre `counters`, `identities` y, para `serial`,
solo las filas de la `meta.installation` legible.

El runtime configura páginas de 4.096 bytes, máximo 262.144 páginas, límite SQLite
de valores de 2 MiB, `journal_mode=DELETE`, `synchronous=EXTRA`,
`foreign_keys=ON`, `trusted_schema=OFF` y espera de busy cero.

El Store es local, tiene un único worker propietario y publica documento, registro,
medios y heads en una transacción. Checksums, estado antes/después, reconciliación
de resultados inciertos y retención de evidencia corrupta siguen siendo parte del
contrato. Leer después de un fallo no equivale por sí solo a confirmar durabilidad.

Los contadores viven fuera de `revisions`; recuperar una revisión anterior no los
restaura hacia atrás. La recolección considera las raíces proporcionadas por
Authority, las revisiones retenidas y la evidencia de recuperación.

La tabla STRICT `counters` tiene una sola fila (`singleton = 1`): `folders`,
`buttons`, `actions` y `serial` son BLOB de ocho bytes, `u64` big-endian. Su `digest`
es SHA-256 de la concatenación `installation:u128 BE || folders:u64 BE ||
buttons:u64 BE || actions:u64 BE || serial:u64 BE`. Tanto ese checksum como el del
registro forman parte de la comparación antes/después de una escritura incierta.
Un candidato con contadores menores que los persistidos se rechaza.

### Apertura de un Store v2 con documento antiguo

Antes de convertir documentos v4 se conserva el archivo completo como
`.document-v4-backup`; con documentos v3 se conserva `.document-v3-backup`;
si solo hay documentos v1/v2, se conserva `.document-v2-backup`. Ninguno sobrescribe un respaldo distinto. Las revisiones
válidas se convierten atómicamente, incluidos sus digests, sin cambiar IDs,
heads, contadores, medios ni el esquema SQLite v2. Evidencia corrupta permanece
intacta y retenida; una versión documental futura impide migrar. Reabrir una
base ya migrada no vuelve a convertir ni sobrescribe el respaldo.

### Apertura de un Store v1

Se conserva una copia completa del archivo anterior con sufijo `.v1-backup` antes
de modificarlo. Nunca se sobrescribe un respaldo existente. La migración convierte
las revisiones mediante el importador del codec y deriva los máximos del registro
**completo**, incluidas las identidades retiradas, no solo del documento vigente.
La transformación y el cambio de versión son una sola transacción; un fallo no
publica una combinación parcial de esquemas.
La publicación del respaldo usa staging exclusivo, sync y hard link sin
sobrescritura; un staging abandonado no impide reintentar. Si una revisión corrupta
tiene un head anterior recuperable, se conserva la evidencia sin cambiar sus bytes
y se activa la retención, en vez de perder el camino de recuperación durante la
migración.

## Copias RADPKG

Una copia lleva el documento, los medios que referencia, los máximos de asignación
y las referencias de plugins usados (ID, versión y origen cuando se conocen).
No lleva contraseñas, binarios, preferencias del editor ni ajustes de la máquina.

### Envelope v2

Todos los enteros del envelope son big-endian. El encabezado mantiene 60 bytes:

| Offset | Bytes | Contenido |
|---|---:|---|
| 0 | 8 | Magic `RADPKG\r\n` |
| 8 | 2 | Versión `2` |
| 10 | 2 | Flags reservados, cero |
| 12 | 8 | Longitud total de la copia |
| 20 | 4 | Longitud del documento |
| 24 | 2 | Cantidad de medios |
| 26 | 2 | Longitud de metadata: `48` |
| 28 | 32 | SHA-256 del documento |

Sigue el documento, luego 48 bytes de metadata: token de instalación `u128` y
cuatro contadores `u64`, en orden carpetas, botones, acciones, serial. A continuación,
por medio: digest de 32 bytes, longitud `u32` y PNG canónico. Cierra un SHA-256 de
todo lo anterior, incluido el encabezado y la metadata. No se permiten bytes después
del digest final, entradas duplicadas ni referencias a medios ausentes.

Límites del paquete: **256 MiB** en total, **512** medios, **128 MiB** de medios
en conjunto y **512 KiB** por imagen normalizada. El documento conserva su límite
de 1 MiB. El staging y el transporte también tienen cuotas y plazos propios; el
éxito de una escritura parcial nunca autoriza publicar un archivo incompleto.

El origen de medios es reiterable y debe permanecer estable durante la operación;
el lector revalida los datos preparados antes de publicar. La retención de la revisión
evita que la recolección retire sus medios durante una exportación.

### Lectura de v1

El encabezado v1 tiene versión `1` y longitud de metadata cero. Su documento se
importa con el mismo lector v1 del codec. Al no contener máximos, se deducen de las
identidades presentes. Una copia v1 no puede recordar el rango de IDs borrados por
encima del máximo presente: es una limitación declarada de ese formato, no una
garantía de no reutilización que se pueda reconstruir.

### Política de restauración

La confirmación humana autoriza reemplazar el deck completo, no ejecutar sus
acciones ni instalar plugins. Es una sola operación del historial y se puede deshacer.
RAD elige los IDs sin preguntar:

| Destino | Resultado |
|---|---|
| Misma instalación | Conserva los IDs |
| Otra instalación con contadores iniciales y raíz vacía | Adopta los IDs de la copia |
| Otra instalación con asignaciones previas | Remapea por encima de ambos rangos |

Un destino que creó y deshizo contenido no es nuevo: sus máximos no retroceden.
Lo creado después usa el token propio del destino y números por encima de ambos
rangos. Para permitir deshacer la adopción inicial sin colisionar con `f1`, se
conserva el vacío anterior con una raíz equivalente remapeada por encima de ambos
rangos; esa excepción se aplica únicamente al destino inicial probado.

**Excepción estrecha para la copia inicial local (precisión aceptada 2026-10-20):**
si se exportó el vacío inicial antes de adoptar una copia extranjera, se puede
restaurar después en esa misma instalación. La copia debe tener contadores
`(folders,buttons,actions,serial)=(1,0,0,1)`, una única carpeta `f1` con identidad
`(instalación de origen,1)`, sin padre, 32 slots vacíos y ningún botón ni carpeta
hija. Se conservan sus ajustes y dependencias no secretos. Si el binding inicial
exacto sigue retenido en el registro (vigente o retirado), no se remapea; si ya
no está, por colisión, desplazamiento o poda, **solo esa raíz vacía**
recibe una asignación local normal por encima de los rangos reservados. No es una
adopción de destino nuevo: el `f1` extranjero queda retirado y retenido para poder
deshacer. No cambia ninguna entidad de botón ni se extiende la excepción a copias
con contenido o historia (contadores no iniciales).

Los ajustes no secretos de plugins se restauran y se deshacen como el resto del
deck. El editor ofrece instalar los ausentes desde un origen conocido, con confianza
explícita; no inventa enlaces para referencias antiguas. Las necesidades de contraseña
se consultan al plugin instalado, no se confían a metadata importada.

## Archivos locales que no viajan

### Preferencias del editor

`$XDG_CONFIG_HOME/rad/editor.json`, o `$HOME/.config/rad/editor.json` si no se
define XDG. JSON v1 acotado a 4 KiB:

```json
{"version":1,"language":null}
```

`language` admite `null`, `"en"` y `"es"`. Sin elección se usa la primera variable
no vacía entre `LC_ALL`, `LC_MESSAGES` y `LANG`, con respaldo en inglés. El cambio
de idioma no necesita conexión con el runtime. La escritura usa archivo temporal,
sync, reemplazo atómico y sync del directorio.

### Memoria de iconos del editor

`$XDG_STATE_HOME/rad/editor-icons.json`, o `$HOME/.local/state/rad/editor-icons.json`
si no se define XDG. JSON v1 de hasta 128 KiB con las 512 imágenes guardadas más
recientes desde la biblioteca de iconos, de la más antigua a la más reciente:

```json
{"version":1,"icons":[{"image":"<digest SHA-256>","icon":"speaker-high","color":"#EBEDF0","background":"#181A1F","size":100}]}
```

`image` es la referencia de la imagen en el deck; `icon`, el identificador estable
del icono incluido; `size`, un porcentaje del 50 al 100 múltiplo de 5. Es una ayuda
local, no configuración: no viaja en las copias, un archivo ausente, mayor del
límite, de otra versión o ilegible se trata como vacío, y una entrada inválida se
ignora. La escribe el worker del editor tras subir la imagen, con la misma
escritura atómica privada que las preferencias; un fallo no altera el guardado.

### Ajustes de máquina

`machine.json` en el directorio local privado del runtime, separado del Store. JSON
v1 de hasta 4 KiB:

```json
{"version":1,"use_stream_deck":false,"start_with_session":false}
```

El runtime lo carga al arrancar. Si no existe, usa los valores por defecto (todo
`false`); si es ilegible, usa los mismos valores, registra el evento
`machine_settings_unreadable` y no reescribe el archivo. Solo `set_machine_settings`
lo escribe, antes de aplicar nada: si el guardado falla, el cambio no se aplica.
`use_stream_deck` toma o suelta el deck en caliente, sin reiniciar el runtime;
`start_with_session` solo se guarda y lo aplica el editor. No se leen secretos ni
configuración de otras aplicaciones.
