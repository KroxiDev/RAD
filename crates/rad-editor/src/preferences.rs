//! Editor-owned preferences, never part of the deck document or backups.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Es,
}
impl Language {
    pub fn from_locale(locale: &str) -> Self {
        if locale
            .split(['_', '-', '.', '@'])
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case("es"))
        {
            Self::Es
        } else {
            Self::En
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Preferences {
    pub language: Option<Language>,
}
impl Preferences {
    pub fn language(self, locale: &str) -> Language {
        self.language
            .unwrap_or_else(|| Language::from_locale(locale))
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Ok(m) if m.is_file() && m.len() <= 4096 => {}
            Ok(_) => return Err(invalid()),
            Err(e) => return Err(e),
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(invalid());
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let object = value.as_object().ok_or_else(invalid)?;
        if value["version"] != 1 || object.keys().any(|k| k != "version" && k != "language") {
            return Err(invalid());
        }
        let language = match value.get("language") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) if v == "en" => Some(Language::En),
            Some(v) if v == "es" => Some(Language::Es),
            _ => return Err(invalid()),
        };
        Ok(Self { language })
    }
    pub fn save(self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let parent = path
            .parent()
            .filter(|p| p.is_absolute())
            .ok_or_else(invalid)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        static NEXT_TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (mut file, mut temporary) = loop {
            let nonce = NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
            {
                Ok(file) => break (file, PendingFile(Some(temporary))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let language = self
            .language
            .map(|l| if l == Language::Es { "es" } else { "en" });
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"language":language}))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary.0.as_ref().expect("unpublished temporary"), path)?;
        temporary.0 = None;
        fs::File::open(parent)?.sync_all()
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid editor preferences")
}
pub fn system_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default()
}
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    base.is_absolute().then(|| base.join("rad/editor.json"))
}

// Own only the file created by this attempt, never an existing collision or the published path.
struct PendingFile(Option<PathBuf>);
impl Drop for PendingFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
