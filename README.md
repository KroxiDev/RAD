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

Keep `rad-launcher-helper` beside `rad-runtime`. The runtime starts without the
deck and takes it when "Use the Stream Deck with RAD" is turned on in the editor's
settings.

Never run two deck controllers together. RAD does not read or modify data of other
applications. Closing the editor leaves the runtime running; Ctrl+C
in the runtime's terminal requests shutdown.

Data lives in `$XDG_DATA_HOME/rad` or `~/.local/share/rad`; local sockets live
under `$XDG_RUNTIME_DIR/rad`.

Existing v1 data migrates to v2 while keeping the original as `.v1-backup`.
Backups are portable between installations; editor preferences stay local in
`$XDG_CONFIG_HOME/rad/editor.json` (or `~/.config/rad/editor.json`). See the
[data formats](docs/formats.md) reference for compatibility and limits.

## Plugins

Official plugins and releases: [KroxiDev/rad-plugins](https://github.com/KroxiDev/rad-plugins).

| Plugin | Release | Controls |
| --- | --- | --- |
| Audio | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.2.0) | Output and microphone mute; status indicators |
| Media | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/media-v0.2.0) | Playback controls and now playing |
| OBS | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/obs-v0.2.0) | Program scene, filters, scene items, recording and streaming |
| Metrics | [0.2.0](https://github.com/KroxiDev/rad-plugins/releases/tag/metrics-v0.2.0) | CPU, RAM, NVIDIA GPU and VRAM visuals |

In **Settings → Plugins**, paste a public GitHub release URL, confirm that you
trust its native code, and choose **Install**. Use the tagged release links above
to select a specific plugin. Updates, reinstalls, rollback and removal are manual.
Releases 0.1.x predate plugin contract v1: RAD keeps them installed but does not
run them until you update to 0.2.0.
Plugins run as native processes, **not in a security sandbox**; install only code
you trust. See the [plugin author guide](plugins/README.md) for the SDK and protocol.

## Autostart

The editor installs and starts the runtime as a systemd **user** service: when the
runtime does not answer, it writes the unit
([packaging/rad-runtime.service](packaging/rad-runtime.service)) to
`$XDG_CONFIG_HOME/systemd/user/` (or `~/.config/systemd/user/`) pointing at the
`rad-runtime` beside the editor, rewrites it when a new version changes it, and
starts it, with or without autostart. Keep `rad-runtime` and `rad-launcher-helper`
together in that folder. The service starts without the deck; using it is a
machine setting changed from the editor. Starting with your computer will
arrive as an editor setting.

The unit restarts the runtime after unexpected failures (exit code 1 or a crash) with a
growing delay, and stops after 5 starts in 5 minutes. It does not restart after a clean
stop (0) or when a restart cannot help (75: another instance is running, or the runtime
directory or environment is unusable). The exit classes are tabulated in
[`crates/rad-runtime/src/exit.rs`](crates/rad-runtime/src/exit.rs).

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

The target architecture, its decisions and the phased plan are documented in
[docs/architecture.md](docs/architecture.md) (in Spanish).

## Status

Tested with one Stream Deck XL on Omarchy/Hyprland. Other device models and Linux
distributions are not validated.

## License

[MIT](LICENSE), copyright KroxiDev. Dependencies retain their own licenses;
see the plugins' [third-party notices](plugins/notices/README.md).
