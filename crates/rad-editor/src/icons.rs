//! Finds the PNG or JPEG icon an installed application ships, so a key can wear it.
//! Imported images are PNG/JPEG only: an application with just an SVG icon has none here.
use crate::session::Inspection;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];
/// Side, in pixels, of the largest image a key stores: nearer icons lose the least.
const TARGET: i64 = 256;

/// Desktop entry whose icon the key should take: its top action opens an application and
/// the key has no image of its own.
pub fn wanted(inspection: &Inspection) -> Option<&str> {
    if !inspection.appearance["image"].is_null() {
        return None;
    }
    let top = &inspection.actions.first()?["fields"];
    if top["type"] != "launcher" {
        return None;
    }
    top["parameters"]["desktop_entry"].as_str()
}

/// Where applications and their icons are installed.
pub struct Icons {
    data_dirs: Vec<PathBuf>,
}
impl Icons {
    /// `data_dirs` in order of precedence, as in `XDG_DATA_HOME` then `XDG_DATA_DIRS`.
    pub fn new(data_dirs: Vec<PathBuf>) -> Self {
        Self { data_dirs }
    }
    pub fn from_environment() -> Self {
        let own = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")));
        let shared = std::env::var_os("XDG_DATA_DIRS")
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let shared = std::env::split_paths(&shared)
            .filter(|path| path.is_absolute())
            .take(16);
        Self::new(own.into_iter().chain(shared).collect())
    }
    /// The best PNG or JPEG icon of `desktop_entry`, if one is installed.
    pub fn find(&self, desktop_entry: &str) -> Option<PathBuf> {
        if !simple_name(desktop_entry) || !desktop_entry.ends_with(".desktop") {
            return None;
        }
        let file = self
            .data_dirs
            .iter()
            .map(|dir| dir.join("applications").join(desktop_entry))
            .find(|path| path.is_file())?;
        let icon = icon_key(&read_text(&file)?)?;
        if icon.starts_with('/') {
            let path = PathBuf::from(icon);
            let raster = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            return (raster && path.is_file()).then_some(path);
        }
        if !simple_name(&icon) {
            return None;
        }
        self.themed(&icon).or_else(|| self.pixmap(&icon))
    }
    /// Icon from the `hicolor` theme every application may install into, at the size
    /// nearest to what a key keeps.
    fn themed(&self, icon: &str) -> Option<PathBuf> {
        let mut best = None;
        for (precedence, dir) in self.data_dirs.iter().enumerate() {
            let theme = dir.join("icons/hicolor");
            for size_dir in std::fs::read_dir(&theme).into_iter().flatten().flatten() {
                let Some((pixels, scale)) = size_dir.file_name().to_str().and_then(size) else {
                    continue;
                };
                for (preference, extension) in EXTENSIONS.iter().enumerate() {
                    let path = size_dir.path().join(format!("apps/{icon}.{extension}"));
                    let rank = ((pixels - TARGET).abs(), precedence, scale, preference);
                    if best.as_ref().is_none_or(|(least, _)| rank < *least) && path.is_file() {
                        best = Some((rank, path));
                    }
                }
            }
        }
        best.map(|(_, path)| path)
    }
    fn pixmap(&self, icon: &str) -> Option<PathBuf> {
        self.data_dirs
            .iter()
            .flat_map(|dir| {
                EXTENSIONS
                    .iter()
                    .map(move |extension| dir.join(format!("pixmaps/{icon}.{extension}")))
            })
            .find(|path| path.is_file())
    }
}

/// A file name, never a path that could leave the directory it is looked up in.
fn simple_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}
/// Pixels and scale of a theme directory named like `256x256` or `128x128@2`.
fn size(directory: &str) -> Option<(i64, i64)> {
    let (size, scale) = directory.split_once('@').unwrap_or((directory, "1"));
    let (width, height) = size.split_once('x')?;
    let (width, height, scale): (i64, i64, i64) = (
        width.parse().ok()?,
        height.parse().ok()?,
        scale.parse().ok()?,
    );
    (width == height && (1..=4096).contains(&width) && (1..=16).contains(&scale))
        .then_some((width * scale, scale))
}
fn read_text(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}
/// Value of `Icon` in the `[Desktop Entry]` group; other groups describe extra actions.
fn icon_key(desktop_file: &str) -> Option<String> {
    let mut in_entry = false;
    for line in desktop_file.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "Icon"
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}
