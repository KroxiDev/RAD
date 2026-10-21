# Desktop assets

`rad.png` is the only artwork source for RAD's window, application menu and tray.
It is the author's provisional GitHub K; replace this file with the definitive PNG
and rebuild, without editing Rust code or the desktop entry.

The stable application ID is `io.github.KroxiDev.RAD`. The editor installs the
`.desktop` template and a 256 px icon under that name in `$XDG_DATA_HOME` (or
`~/.local/share`), rewriting them only when they change and pointing `Exec=` at the
editor in use.

Build `rad-editor`, `rad-runtime` and `rad-tray` together for tray support. The editor
installs both user units only when their contents change; a partial build without
`rad-tray` still runs the authority. No separate tray autostart is needed (ADR 0016).
