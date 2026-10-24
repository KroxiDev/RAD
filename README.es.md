[English](README.md) · [Español](README.es.md)

# RAD · Rust Action Deck

Un editor nativo para Linux y un runtime local para tu Stream Deck.

![Editor de RAD con un deck demo](docs/images/editor-main.png)

## ¿Qué es RAD?

RAD convierte un Stream Deck XL en un lanzador de aplicaciones y una superficie
de control configurable. Organiza botones visualmente, agrúpalos en carpetas y
combina acciones. El runtime sigue funcionando al cerrar el editor.

## Funciones

- Distribución de 32 teclas, carpetas, edición por arrastre y deshacer/rehacer.
- Hasta 20 acciones por botón, ejecución secuencial o paralela y acciones nativas
  Esperar y Suspender Stream Deck.
- Apertura de aplicaciones desde el catálogo del escritorio.
- Texto propio e imágenes PNG/JPEG estáticas.
- Interfaz en inglés y español; ajustes de brillo, reposo y umbral de pulsación.
- Exportar/restaurar configuración. La exportación excluye credenciales y ejecutables de plugins.
- Automatización local por CLI con permisos heredados de consulta, modificación y ejecución.
- Plugins opcionales de audio, multimedia, OBS y métricas del sistema.
- Los plugins oficiales se cargan bajo demanda y duermen cuando nada los necesita.
  Los botones visibles, consultas y trabajos admitidos pueden mantenerlos activos,
  no solo las pulsaciones.

## Galería

| Inspector de tecla | Acción de plugin |
| --- | --- |
| ![Una tecla con varias acciones](docs/images/editor-key.png) | ![Formulario de una acción OBS](docs/images/editor-plugin-action.png) |

![Ajustes del editor](docs/images/editor-settings.png)

Las capturas usan una configuración demo.

## Requisitos

- Linux con Wayland, un driver compatible con OpenGL y un Rust reciente que soporte la edición 2024.
- Acceso de lectura/escritura al nodo `hidraw` del Stream Deck. Las distribuciones
  con el hwdb de controladores audiovisuales de systemd y sus reglas udev pueden
  conceder acceso a la sesión activa mediante `uaccess`; comprueba los permisos
  de dispositivos de tu distribución.
- Audio: `pactl`. Multimedia: un reproductor MPRIS en el D-Bus de sesión.
- Métricas GPU/VRAM: driver NVIDIA con NVML. CPU/RAM no requieren NVIDIA.
- OBS: OBS Studio local con su servidor WebSocket habilitado.
- Tests: `bwrap`, `pactl`, `dbus-daemon` y soporte de namespaces Linux.

## Compilar y ejecutar

```bash
cargo build --release
target/release/rad-editor
```

La primera vez, abre `rad-editor` desde la carpeta `target/release` de RAD: añade
RAD al menú de aplicaciones y arranca el runtime; desde entonces, abre RAD desde el
menú. Mantén `rad-runtime`, `rad-tray` y `rad-launcher-helper` junto al editor. El
runtime arranca sin el deck y lo toma cuando se activa «Usar el Stream Deck con RAD»
en los ajustes del editor.

Nunca uses dos controladores del deck a la vez. RAD no lee ni modifica datos de
otras aplicaciones. Cerrar el editor deja el runtime activo; «Salir de RAD» en el
ícono de la bandeja cierra el editor y detiene el runtime.

Los datos viven en `$XDG_DATA_HOME/rad` o `~/.local/share/rad`; los sockets locales,
bajo `$XDG_RUNTIME_DIR/rad`.

El documento actual del deck es v4; el Store SQLite y el contenedor de copias
siguen en v2. Los datos anteriores migran hacia adelante conservando el original;
un Store v1 conserva un `.v1-backup`.
Las copias son portables entre instalaciones; las preferencias del editor quedan
en `$XDG_CONFIG_HOME/rad/editor.json` (o `~/.config/rad/editor.json`). Consulta los
[formatos de datos](docs/formats.md) para conocer la compatibilidad y los límites.

## Plugins

Plugins oficiales y releases: [KroxiDev/rad-plugins](https://github.com/KroxiDev/rad-plugins).

| Plugin | Release | Controles |
| --- | --- | --- |
| Audio | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.2.0) | Silenciar salida y micrófono; indicadores de estado |
| Multimedia | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/media-v0.2.0) | Controles de reproducción y reproducción actual |
| OBS | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/obs-v0.2.0) | Escena Program, filtros, elementos, grabación y transmisión |
| Métricas | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/metrics-v0.2.0) | Visuales de CPU, RAM, GPU NVIDIA y VRAM |

En **Ajustes → Plugins → + Añadir plugin**, instala un plugin oficial con su botón,
o pega la URL pública de release de GitHub de otro, confirma que confías en su
código nativo y pulsa **Instalar**; una barra muestra la descarga. Un plugin oficial
ofrece **Actualizar** cuando sale una release más nueva; los demás se actualizan
desde el origen con que se instalaron. Reinstalar y revertir están en **Ver
detalles**. Las releases 0.1.x son anteriores al
contrato de plugins v1: RAD las conserva instaladas, pero no las ejecuta hasta
que actualices a 0.2.0. Los plugins son procesos nativos,
**no un sandbox de seguridad**: instala solo código en el que confíes.
La [guía para autores](plugins/README.md) explica el SDK y el protocolo.

## Arranque automático

El editor instala y arranca el runtime como servicio systemd **de usuario**: si el
runtime no responde, escribe la unidad
([packaging/rad-runtime.service](packaging/rad-runtime.service)) en
`$XDG_CONFIG_HOME/systemd/user/` (o `~/.config/systemd/user/`) apuntando al
`rad-runtime` que está junto al editor, la reescribe si una versión nueva la cambia
y la arranca, con o sin inicio automático. Mantén `rad-runtime` y
`rad-launcher-helper` juntos en esa carpeta. El servicio arranca sin el deck. Usar
el deck e iniciar RAD con el equipo son ajustes de la máquina que se cambian desde el
editor; activar el primero activa también el segundo.

La unidad reinicia el runtime tras un fallo inesperado (código de salida 1 o una
caída) con una espera creciente, y se detiene tras 5 arranques en 5 minutos. No lo
reinicia tras una parada limpia (0) ni cuando reiniciar no sirve (75: hay otra
instancia en marcha, o el directorio de runtime o el entorno no se pueden usar). Las
clases de salida están en
[`crates/rad-runtime/src/exit.rs`](crates/rad-runtime/src/exit.rs).

## Cómo funciona

```text
Editor / rad-cli ↔ socket Unix local ↔ Runtime ↔ Stream Deck (hidraw)
                                         ↕
                                  Procesos de plugins
```

El runtime posee la configuración, ejecución, navegación y dispositivo. El editor
nativo eframe/egui usa Glow; editor y CLI comparten la validación de dominio.
Cada plugin activo tiene su propio proceso, aislado del espacio de memoria del runtime.

## Desarrollo

Empieza por [CONTRIBUTING.md](CONTRIBUTING.md) y el
[índice de documentación técnica](docs/README.md), cuyo idioma canónico es el
español. Conectan glosario, arquitectura, contratos y recorrido de contribución.

La aplicación y los plugins son workspaces Cargo separados. La
[guía de validación](docs/testing.md) es la fuente única del gate completo offline,
builds de helpers, aislamiento de tests, formato, clippy y conformidades.
Los agentes también deben seguir [AGENTS.md](AGENTS.md). Compilar no requiere
abrir el editor ni tomar el dispositivo.

## Estado

Probado con un Stream Deck XL en Omarchy/Hyprland. Otros modelos de dispositivo y
distribuciones Linux no están validados.

## Licencia

[MIT](LICENSE), copyright KroxiDev. Las dependencias conservan sus licencias;
consulta las [licencias de terceros](plugins/notices/README.md) de los plugins.
