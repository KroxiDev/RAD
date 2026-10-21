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
- Hasta 20 acciones por botón, ejecución secuencial o paralela y acción Esperar integrada.
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
target/release/rad-runtime  # terminal 1: sin hardware por defecto
target/release/rad-editor   # terminal 2
```

Mantén `rad-launcher-helper` junto a `rad-runtime`. Para tomar el control del XL,
primero cierra StreamController y evita que se reinicie automáticamente; luego:

```bash
target/release/rad-runtime --xl-exclusive
```

Nunca uses dos controladores del deck a la vez. RAD no importa ni modifica datos
de StreamController. Cerrar el editor deja el runtime activo; Ctrl+C en la
terminal del runtime solicita su cierre.

Los datos viven en `$XDG_DATA_HOME/rad` o `~/.local/share/rad`; los sockets locales,
bajo `$XDG_RUNTIME_DIR/rad`.

Los datos v1 se migran a v2 conservando el original con sufijo `.v1-backup`.
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

En **Ajustes → Plugins**, pega una URL pública de release de GitHub, confirma que
confías en su código nativo y pulsa **Instalar**. Usa los enlaces etiquetados de
arriba para elegir un plugin concreto. Actualizar, reinstalar, revertir y
desinstalar son operaciones manuales. Las releases 0.1.x son anteriores al
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
`rad-launcher-helper` juntos en esa carpeta. El servicio arranca sin hardware;
añade `--xl-exclusive` solo después de asegurar el control exclusivo, con un
*drop-in* (`systemctl --user edit rad-runtime.service`). Habilitar el inicio con
el equipo vendrá como un ajuste del editor.

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

La aplicación y los plugins son workspaces Cargo separados:

```bash
cargo test --release --workspace --all-features
cargo test --manifest-path plugins/Cargo.toml --release --workspace --all-features
```

Compila primero los binarios/ejemplos de fixtures. La [puerta completa de AGENTS.md](AGENTS.md#validación-completa)
incluye ejecución offline, variables de helpers, formato, clippy y scripts de
conformidad. Los tests usan servicios privados y lectores sintéticos, no el deck físico.

La arquitectura objetivo, sus decisiones y el plan por fases están en
[docs/architecture.md](docs/architecture.md).

## Estado

Probado con un Stream Deck XL en Omarchy/Hyprland. Otros modelos de dispositivo y
distribuciones Linux no están validados.

## Licencia

[MIT](LICENSE), copyright KroxiDev. Las dependencias conservan sus licencias;
consulta los [avisos de terceros](plugins/notices/README.md) de los plugins.
