//! What this computer remembers about images made with the icon library: the icon,
//! colors and size behind each one, keyed by the image's digest, so reopening the library
//! starts from them. It is a convenience, never configuration: a missing, unreadable or
//! invalid memory just means choosing again, and a failed write never affects a save.
use crate::icon_library::{Choice, ENTRIES, SCALE_STEP, SCALES};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

/// Images remembered; the least recently saved are forgotten first.
const LIMIT: usize = 512;
/// A larger file is ignored rather than read.
const MAX_BYTES: u64 = 128 * 1024;

/// `$XDG_STATE_HOME/rad/editor-icons.json`, or `$HOME/.local/state/rad/editor-icons.json`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    base.is_absolute()
        .then(|| base.join("rad/editor-icons.json"))
}

/// The choice that produced `image`, a media digest, if this computer remembers it.
pub fn recall(path: &Path, image: &str) -> Option<Choice> {
    if !is_digest(image) {
        return None;
    }
    read(path)
        .into_iter()
        .find_map(|(digest, choice)| (digest == image).then_some(choice))
}

/// Remembers that `choice` produced `image`. An unreadable memory is replaced.
pub fn remember(path: &Path, image: &str, choice: Choice) -> io::Result<()> {
    if !is_digest(image) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an image digest",
        ));
    }
    let mut entries = read(path);
    entries.retain(|(digest, _)| digest != image);
    entries.push((image.to_owned(), choice));
    let first = entries.len().saturating_sub(LIMIT);
    let icons: Vec<_> = entries[first..]
        .iter()
        .map(|(digest, choice)| {
            json!({
                "image": digest,
                "icon": ENTRIES[choice.icon].id,
                "color": hex(choice.foreground),
                "background": hex(choice.background),
                "size": choice.scale,
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&json!({"version": 1, "icons": icons}))?;
    crate::preferences::write_private(path, &bytes)
}

/// Every valid entry, oldest first; anything unreadable counts as an empty memory.
fn read(path: &Path) -> Vec<(String, Choice)> {
    let mut bytes = Vec::new();
    let readable = std::fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .is_ok_and(|length| length as u64 <= MAX_BYTES);
    let Some(document) = readable
        .then(|| serde_json::from_slice::<Value>(&bytes).ok())
        .flatten()
        .filter(|document| document["version"] == 1)
    else {
        return Vec::new();
    };
    document["icons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(entry)
        .collect()
}

fn entry(value: &Value) -> Option<(String, Choice)> {
    let image = value["image"].as_str().filter(|d| is_digest(d))?;
    let id = value["icon"].as_str()?;
    let icon = ENTRIES.iter().position(|entry| entry.id == id)?;
    let scale = value["size"]
        .as_u64()
        .and_then(|size| u8::try_from(size).ok())
        .filter(|size| SCALES.contains(size) && size % SCALE_STEP == 0)?;
    Some((
        image.to_owned(),
        Choice {
            icon,
            foreground: color(&value["color"])?,
            background: color(&value["background"])?,
            scale,
        },
    ))
}

fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn color(value: &Value) -> Option<[u8; 3]> {
    let digits = value.as_str()?.strip_prefix('#').filter(|d| d.len() == 6)?;
    let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}
