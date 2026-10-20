[English](README.md) · [Español](README.es.md)

# RAD · Rust Action Deck

A native Linux editor and local runtime for your Stream Deck.

![RAD editor with a demo deck](docs/images/editor-main.png)

## What is RAD?

RAD turns a Stream Deck XL into a configurable application launcher and control
surface. Arrange buttons visually, organize them into folders, and combine actions.
The runtime keeps working when the editor is closed.

## Features

- A 32-key layout, folders, drag-and-drop editing and undo/redo.
- Up to 20 actions per button, sequential or parallel execution, and a built-in Wait action.
- Application launching from your desktop's application catalog.
- Custom labels and static PNG/JPEG images.
- English and Spanish UI; brightness, idle and press-threshold settings.
- Configuration export/restore. Exports exclude credentials and plugin executables.
- Local CLI automation with inherited query, modify and execute permissions.
- Optional audio, media, OBS and system-metrics plugins.
- Official plugins load on demand and sleep when nothing needs them. Visible
  buttons, queries and accepted work can keep them active—not just key presses.

## Gallery

| Key inspector | Plugin action |
| --- | --- |
| ![A key with multiple actions](docs/images/editor-key.png) | ![An OBS action form](docs/images/editor-plugin-action.png) |

![Editor settings](docs/images/editor-settings.png)

Screenshots use a demo configuration.

## Requirements

- Linux with Wayland, an OpenGL-capable driver and a recent Rust toolchain supporting edition 2024.
- Read/write access to the Stream Deck's `hidraw` node. Distributions with the
  systemd audiovisual-controller hwdb and udev rules can grant active-session
  access through `uaccess`; check your distribution's device permissions.
- Audio: `pactl`. Media: an MPRIS player on the session D-Bus.
- GPU/VRAM metrics: the NVIDIA driver with NVML. CPU/RAM do not require NVIDIA.
- OBS: local OBS Studio with its WebSocket server enabled.
- Tests: `bwrap`, `pactl`, `dbus-daemon` and Linux namespace support.

## Build and run

```bash
cargo build --release
target/release/rad-runtime  # terminal 1: no hardware by default
target/release/rad-editor   # terminal 2
```

Keep `rad-launcher-helper` beside `rad-runtime`. To take control of the XL,
first close StreamController and prevent its automatic restart, then start:

```bash
target/release/rad-runtime --xl-exclusive
```

Never run two deck controllers together. RAD does not import or modify
StreamController data. Closing the editor leaves the runtime running; Ctrl+C
in the runtime's terminal requests shutdown.

Data lives in `$XDG_DATA_HOME/rad` or `~/.local/share/rad`; local sockets live
under `$XDG_RUNTIME_DIR/rad`.

## Plugins

Official plugins and releases: [KroxiDev/rad-plugins](https://github.com/KroxiDev/rad-plugins).

| Plugin | Release | Controls |
| --- | --- | --- |
| Audio | [0.1.0](https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.1.0) | Output and microphone mute; status indicators |
| Media | [0.1.1](https://github.com/KroxiDev/rad-plugins/releases/tag/media-v0.1.1) | Playback controls and now playing |
| OBS | [0.1.1](https://github.com/KroxiDev/rad-plugins/releases/tag/obs-v0.1.1) | Program scene, filters, scene items, recording and streaming |
| Metrics | [0.1.1](https://github.com/KroxiDev/rad-plugins/releases/tag/metrics-v0.1.1) | CPU, RAM, NVIDIA GPU and VRAM visuals |

In **Settings → Plugins**, paste a public GitHub release URL, confirm that you
trust its native code, and choose **Install**. Use the tagged release links above
to select a specific plugin. Updates, reinstalls, rollback and removal are manual.
Plugins run as native processes, **not in a security sandbox**; install only code
you trust. See the [plugin author guide](plugins/README.md) for the SDK and protocol.

## Autostart

[packaging/rad-runtime.service](packaging/rad-runtime.service) is an example
systemd **user** unit. Adapt its executable path and install the runtime/helper
together before enabling it with your user service manager. The example starts
without hardware; add `--xl-exclusive` only after arranging exclusive ownership.
Nothing is installed or enabled automatically.

## How it works

```text
Editor / rad-cli ↔ local Unix socket ↔ Runtime ↔ Stream Deck (hidraw)
                                         ↕
                                  Plugin processes
```

The runtime owns configuration, execution, navigation and the device. The native
eframe/egui editor uses Glow; editor and CLI share the same domain validation.
Each active plugin has its own process, isolated from the runtime's address space.

## Development

The application and plugins are separate Cargo workspaces:

```bash
cargo test --release --workspace --all-features
cargo test --manifest-path plugins/Cargo.toml --release --workspace --all-features
```

Build the fixture binaries/examples first. See [AGENTS.md](AGENTS.md#validación-completa)
for the complete offline gate, helper variables, formatting, clippy and conformance
scripts. Tests use private services and synthetic readers, not the physical deck.

## Status

Tested with one Stream Deck XL on Omarchy/Hyprland. Other device models and Linux
distributions are not validated. The repository remains private.

## License

[MIT](LICENSE), copyright KroxiDev. Dependencies retain their own licenses;
see the plugins' [third-party notices](plugins/notices/README.md).
