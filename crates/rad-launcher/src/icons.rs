use gio::prelude::*;
use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Path};
pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains(['/', '\\'])
        && name != "."
        && name != ".."
        && !name.chars().any(char::is_control)
}
fn read(path: &Path) -> Option<rad_media::NormalizedImage> {
    if !matches!(
        path.extension()?.to_str()?.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg"
    ) {
        return None;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    rad_media::normalize(&bytes).ok()
}
pub(super) fn preview(id: &str, theme: &str) -> (rad_media::NormalizedImage, bool) {
    let image = gio_unix::DesktopAppInfo::new(id)
        .and_then(|a| a.icon())
        .and_then(|icon| {
            if let Some(file) = icon.downcast_ref::<gio::FileIcon>() {
                return file.file().path().and_then(|path| read(&path));
            }
            let themed = icon.downcast_ref::<gio::ThemedIcon>()?;
            let roots = roots();
            for name in themed.names() {
                if !valid_name(&name) {
                    continue;
                }
                let mut seen = std::collections::BTreeSet::new();
                if let Some(image) = lookup(&roots, theme, &name, &mut seen)
                    .or_else(|| lookup(&roots, "hicolor", &name, &mut seen))
                {
                    return Some(image);
                }
                for root in &roots {
                    for extension in ["png", "jpg", "jpeg"] {
                        if let Some(image) = read(&root.join(format!("{name}.{extension}"))) {
                            return Some(image);
                        }
                    }
                }
            }
            None
        });
    match image {
        Some(image) => (image, false),
        None => (
            rad_media::normalize(include_bytes!("../assets/generic.png"))
                .expect("embedded static PNG"),
            true,
        ),
    }
}

fn roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(home).join(".icons"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".local/share"))
        });
    if let Some(data) = data {
        roots.push(data.join("icons"));
    }
    let dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for data in std::env::split_paths(&dirs)
        .filter(|p| p.is_absolute())
        .take(16)
    {
        roots.push(data.join("icons"));
        roots.push(data.join("pixmaps"));
    }
    roots
}
fn safe_subdir(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && std::path::Path::new(name)
            .components()
            .all(|p| matches!(p, std::path::Component::Normal(_)))
}
fn lookup(
    roots: &[std::path::PathBuf],
    theme: &str,
    name: &str,
    seen: &mut std::collections::BTreeSet<String>,
) -> Option<rad_media::NormalizedImage> {
    if !valid_name(theme) || seen.len() >= 32 || !seen.insert(theme.into()) {
        return None;
    }
    let mut parents = Vec::new();
    let mut candidates = Vec::new();
    for (root_index, root) in roots.iter().enumerate() {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(root.join(theme).join("index.theme"))
            .ok();
        let Some(file) = file else { continue };
        if !file.metadata().ok()?.is_file() {
            continue;
        }
        let mut data = String::new();
        if file.take(65537).read_to_string(&mut data).is_err() || data.len() > 65536 {
            continue;
        }
        let key = glib::KeyFile::new();
        if key.load_from_data(&data, glib::KeyFileFlags::NONE).is_err() {
            continue;
        }
        if let Ok(inherits) = key.string("Icon Theme", "Inherits") {
            for parent in inherits
                .split(',')
                .map(str::trim)
                .filter(|p| valid_name(p))
                .take(32)
            {
                if !parents.iter().any(|p| p == parent) {
                    parents.push(parent.to_owned());
                }
            }
        }
        if let Ok(dirs) = key.string("Icon Theme", "Directories") {
            for (order, dir) in dirs
                .split(',')
                .map(str::trim)
                .filter(|p| safe_subdir(p))
                .take(256)
                .enumerate()
            {
                let size = i64::from(key.integer(dir, "Size").unwrap_or(256)).clamp(1, 4096);
                let scale = i64::from(key.integer(dir, "Scale").unwrap_or(1)).clamp(1, 16);
                let kind = key
                    .string(dir, "Type")
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "Threshold".into());
                let (min, max) = if kind == "Scalable" {
                    (
                        i64::from(key.integer(dir, "MinSize").unwrap_or(size as i32)),
                        i64::from(key.integer(dir, "MaxSize").unwrap_or(size as i32)),
                    )
                } else if kind == "Threshold" {
                    let threshold = i64::from(key.integer(dir, "Threshold").unwrap_or(2));
                    (size - threshold, size + threshold)
                } else {
                    (size, size)
                };
                let distance = if 256 < min * scale {
                    min * scale - 256
                } else if 256 > max * scale {
                    256 - max * scale
                } else {
                    0
                };
                for (extension_index, extension) in ["png", "jpg", "jpeg"].iter().enumerate() {
                    candidates.push((
                        distance,
                        root_index,
                        order,
                        extension_index,
                        root.join(theme)
                            .join(dir)
                            .join(format!("{name}.{extension}")),
                    ));
                }
            }
        }
    }
    candidates.sort_by_key(|a| (a.0, a.1, a.2, a.3));
    for (_, _, _, _, path) in candidates {
        if let Some(image) = read(&path) {
            return Some(image);
        }
    }
    for parent in parents {
        if let Some(image) = lookup(roots, &parent, name, seen) {
            return Some(image);
        }
    }
    None
}
#[derive(Default)]
pub(super) struct Cache {
    entries: std::collections::VecDeque<(String, String, rad_media::NormalizedImage, bool)>,
    bytes: usize,
}
impl Cache {
    pub fn get(&mut self, id: &str, theme: &str) -> (rad_media::NormalizedImage, bool) {
        if let Some(index) = self.entries.iter().position(|e| e.0 == id && e.1 == theme) {
            let entry = self.entries.remove(index).unwrap();
            let result = (entry.2.clone(), entry.3);
            self.entries.push_front(entry);
            return result;
        }
        let (image, generic) = preview(id, theme);
        self.insert(id.into(), theme.into(), image.clone(), generic);
        (image, generic)
    }
    fn insert(
        &mut self,
        id: String,
        theme: String,
        image: rad_media::NormalizedImage,
        generic: bool,
    ) {
        let cost = id.len() + theme.len() + image.png_bytes().len();
        while self.entries.len() >= 256 || self.bytes + cost > 8 * 1024 * 1024 {
            let Some((id, theme, image, _)) = self.entries.pop_back() else {
                return;
            };
            self.bytes -= id.len() + theme.len() + image.png_bytes().len();
        }
        self.bytes += cost;
        self.entries.push_front((id, theme, image, generic));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lru_retained_bytes_and_entry_count_are_bounded() {
        use image::ImageEncoder;
        let mut seed = 1u32;
        let pixels: Vec<u8> = (0..256 * 256 * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, 256, 256, image::ExtendedColorType::Rgba8)
            .unwrap();
        let image = rad_media::normalize(&png).unwrap();
        let mut cache = Cache::default();
        for i in 0..64 {
            cache.insert(
                format!("{i}.desktop"),
                "hicolor".into(),
                image.clone(),
                false,
            );
            assert!(cache.bytes <= 8 * 1024 * 1024);
        }
        assert!(cache.entries.len() < 64);
        assert!(!cache.entries.iter().any(|e| e.0 == "0.desktop"));
        let small = rad_media::normalize(include_bytes!("../assets/generic.png")).unwrap();
        let mut cache = Cache::default();
        for i in 0..257 {
            cache.insert(
                format!("{i}.desktop"),
                "hicolor".into(),
                small.clone(),
                false,
            );
        }
        assert_eq!(cache.entries.len(), 256);
        let _ = cache.get("1.desktop", "hicolor");
        cache.insert("new.desktop".into(), "hicolor".into(), small, false);
        assert!(cache.entries.iter().any(|e| e.0 == "1.desktop"));
        assert!(!cache.entries.iter().any(|e| e.0 == "2.desktop"));
    }
}
