#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub accent: [u8; 3],
}
impl Default for Palette {
    fn default() -> Self {
        Self {
            background: [21, 25, 34],
            foreground: [230, 234, 243],
            accent: [88, 180, 220],
        }
    }
}
impl Palette {
    pub fn apply(&mut self, text: &str) -> bool {
        if text.len() > 4096 {
            return false;
        }
        let Ok(table) = text.parse::<toml::Table>() else {
            return false;
        };
        let color = |name: &str| -> Option<[u8; 3]> {
            let hex = table.get(name)?.as_str()?.strip_prefix('#')?;
            if hex.len() != 6 || !hex.is_ascii() {
                return None;
            }
            Some([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ])
        };
        let (Some(background), Some(foreground), Some(accent)) =
            (color("background"), color("foreground"), color("accent"))
        else {
            return false;
        };
        *self = Self {
            background,
            foreground,
            accent,
        };
        true
    }
    pub fn read(&mut self, directory: &std::path::Path) {
        use std::io::Read;
        if let Ok(file) = std::fs::File::open(directory.join("colors.toml")) {
            let mut text = String::new();
            if file.take(4097).read_to_string(&mut text).is_ok() {
                self.apply(&text);
            }
        }
    }
}
