# Desktop assets

`rad.png` is the only artwork source for RAD's window, application menu and tray.
It is the author's provisional GitHub K; replace this file with the definitive PNG
and rebuild, without editing Rust code or the desktop entry.

The stable application ID is `io.github.KroxiDev.RAD`. Install the icon under that
name in the desktop icon theme; the matching `.desktop` template is provided here.
Automatic menu installation remains roadmap E2; this template does not install itself.

Build `rad-editor`, `rad-runtime` and `rad-tray` together for tray support. The editor
installs both user units only when their contents change; a partial build without
`rad-tray` still runs the authority. No separate tray autostart is needed (ADR 0016).
