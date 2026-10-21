//! RAD in the application menu (E2, ADR 0016): the editor installs its desktop entry and
//! icon under the user's data directory, like the units of ADR 0012. Each file is written
//! only when missing or different, and the entry runs the editor in use.
use crate::{app_icon, service::write_atomic};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const TEMPLATE: &str = include_str!("../../../packaging/io.github.KroxiDev.RAD.desktop");
/// Size of the installed icon, a standard `hicolor` directory.
const ICON_SIZE: u32 = 256;

/// `$XDG_DATA_HOME`, or `$HOME/.local/share`. Like the XDG rules, a relative or empty
/// `XDG_DATA_HOME` is ignored.
pub fn data_directory(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg_data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local/share"))
        })
}

/// [`install`] for the current user and the running editor.
pub fn install_for_user() -> Result<bool, String> {
    let data = data_directory(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
        .ok_or("no usable HOME or XDG_DATA_HOME")?;
    let editor =
        std::env::current_exe().map_err(|e| format!("the editor's path is unknown: {e}"))?;
    install(&data, &editor)
}

/// Installs the entry and the icon if missing or changed; says whether anything was written.
pub fn install(data: &Path, editor: &Path) -> Result<bool, String> {
    let entry = render_entry(editor)?;
    let mut icon = Vec::new();
    app_icon::png_rgba(ICON_SIZE)
        .write_to(
            &mut std::io::Cursor::new(&mut icon),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("the icon could not be encoded: {e}"))?;
    let id = app_icon::APP_ID;
    let files = [
        (
            data.join(format!("applications/{id}.desktop")),
            entry.into_bytes(),
        ),
        (
            data.join(format!(
                "icons/hicolor/{ICON_SIZE}x{ICON_SIZE}/apps/{id}.png"
            )),
            icon,
        ),
    ];
    let mut changed = false;
    for (path, wanted) in files {
        if std::fs::read(&path).is_ok_and(|current| current == wanted) {
            continue;
        }
        write_atomic(&path, &wanted)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        changed = true;
    }
    Ok(changed)
}

/// The template with its single `Exec=` line running `editor`, quoted and escaped the way
/// the Desktop Entry Specification reads it.
fn render_entry(editor: &Path) -> Result<String, String> {
    let path = editor
        .to_str()
        .filter(|p| editor.is_absolute() && !p.chars().any(char::is_control))
        .ok_or_else(|| format!("{} cannot be written in a desktop entry", editor.display()))?;
    let reserved = |c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c);
    let exec = if path.contains(reserved) {
        // Quoting escapes `"`, `` ` ``, `$` and `\`; then the string escape doubles `\`.
        let quoted: String = path
            .chars()
            .flat_map(|c| match c {
                '"' | '`' | '$' => vec!['\\', '\\', c],
                '\\' => vec!['\\'; 4],
                _ => vec![c],
            })
            .collect();
        format!("\"{quoted}\"")
    } else {
        path.to_owned()
    }
    .replace('%', "%%");
    if TEMPLATE.lines().filter(|l| l.starts_with("Exec=")).count() != 1 {
        return Err("the desktop entry template needs exactly one Exec= line".into());
    }
    Ok(TEMPLATE
        .split_inclusive('\n')
        .map(|line| match line.strip_prefix("Exec=") {
            Some(old) => {
                let end = &old[old.trim_end_matches(['\n', '\r']).len()..];
                format!("Exec={exec}{end}")
            }
            None => line.to_owned(),
        })
        .collect())
}
