use crate::{MediaEntry, MediaReadError, MediaSource, StoreError, digest};
use rad_core::configuration::Configuration;
use rad_media::NormalizedImage;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};
const COUNT: usize = 512;
const BYTES: usize = 128 * 1024 * 1024;
fn references(config: &Configuration) -> BTreeSet<String> {
    config
        .data()
        .buttons
        .iter()
        .filter_map(|b| b.appearance.image.clone())
        .collect()
}
fn parse_key(key: &str) -> Result<Vec<u8>, StoreError> {
    if key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(StoreError::Invalid);
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&key[i..i + 2], 16).map_err(|_| StoreError::Invalid))
        .collect()
}
pub(crate) fn read(connection: &Connection, key: &str) -> Result<NormalizedImage, StoreError> {
    let hash = parse_key(key)?;
    let (bytes, width, height): (Vec<u8>, u32, u32) = connection
        .query_row(
            "SELECT bytes,width,height FROM media WHERE digest=?1",
            [&hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(StoreError::Invalid)?;
    if digest(&bytes) != hash {
        return Err(StoreError::Corrupt);
    }
    let image = NormalizedImage::validate(&bytes).map_err(|_| StoreError::Corrupt)?;
    if image.dimensions() != (width, height) {
        return Err(StoreError::Corrupt);
    }
    Ok(image)
}
fn key(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
fn supplied(
    source: &mut dyn MediaSource,
    entry: &MediaEntry,
    catalog: &[MediaEntry],
) -> Result<NormalizedImage, StoreError> {
    if source.catalog() != catalog {
        return Err(StoreError::Invalid);
    }
    let image = source.read(&entry.digest).map_err(|e| match e {
        MediaReadError::Invalid => StoreError::Invalid,
        MediaReadError::Io => StoreError::Io,
    })?;
    if source.catalog() != catalog {
        return Err(StoreError::Invalid);
    }
    if image.png_bytes().len() != entry.length as usize || digest(image.png_bytes()) != entry.digest
    {
        return Err(StoreError::Corrupt);
    }
    NormalizedImage::validate(image.png_bytes()).map_err(|_| StoreError::Corrupt)?;
    Ok(image)
}
// Compare against SQLite in bounded chunks, never materialize a second source image.
fn compare(
    connection: &Connection,
    hash: &[u8; 32],
    image: &NormalizedImage,
) -> Result<(), StoreError> {
    let length: i64 = connection.query_row(
        "SELECT length(bytes) FROM media WHERE digest=?1",
        [hash.as_slice()],
        |r| r.get(0),
    )?;
    if length != image.png_bytes().len() as i64 {
        return Err(StoreError::Corrupt);
    }
    for (i, expected) in image.png_bytes().chunks(65536).enumerate() {
        let bytes: Vec<u8> = connection.query_row(
            "SELECT substr(bytes,?2,?3) FROM media WHERE digest=?1",
            params![
                hash.as_slice(),
                (i * 65536 + 1) as i64,
                expected.len() as i64
            ],
            |r| r.get(0),
        )?;
        if bytes != expected {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(())
}
pub(crate) fn prepare(
    connection: &Connection,
    config: &Configuration,
    source: &mut dyn MediaSource,
) -> Result<BTreeSet<String>, StoreError> {
    let refs = references(config);
    let catalog = source.catalog();
    if refs.len() > COUNT || catalog.len() > COUNT {
        return Err(StoreError::Capacity);
    }
    let catalog = catalog.to_vec();
    let mut unique = BTreeMap::new();
    let mut supplied_bytes = 0usize;
    for entry in &catalog {
        if entry.length == 0 || entry.length > 512 * 1024 {
            return Err(StoreError::Capacity);
        }
        supplied_bytes = supplied_bytes
            .checked_add(entry.length as usize)
            .ok_or(StoreError::Capacity)?;
        let key = key(&entry.digest);
        if !refs.contains(&key) || unique.insert(key, *entry).is_some() {
            return Err(StoreError::Invalid);
        }
    }
    if supplied_bytes > BYTES {
        return Err(StoreError::Capacity);
    }
    // Entire first pass is read-only. Existing and supplied images are never retained together.
    let mut total = 0usize;
    for key in &refs {
        let hash = parse_key(key)?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM media WHERE digest=?1)",
            [&hash],
            |r| r.get(0),
        )?;
        let length = if exists {
            let length = read(connection, key)?.png_bytes().len();
            if let Some(entry) = unique.get(key) {
                let image = supplied(source, entry, &catalog)?;
                compare(connection, &entry.digest, &image)?;
            }
            length
        } else {
            let entry = unique.get(key).ok_or(StoreError::Invalid)?;
            supplied(source, entry, &catalog)?.png_bytes().len()
        };
        total = total.checked_add(length).ok_or(StoreError::Capacity)?;
        if total > BYTES {
            return Err(StoreError::Capacity);
        }
    }
    // Second pass stays inside caller's transaction; a late failure rolls everything back.
    for entry in unique.values() {
        let image = supplied(source, entry, &catalog)?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM media WHERE digest=?1)",
            [entry.digest.as_slice()],
            |r| r.get(0),
        )?;
        if exists {
            compare(connection, &entry.digest, &image)?;
        } else {
            let (w, h) = image.dimensions();
            connection.execute(
                "INSERT INTO media VALUES(?1,?2,?3,?4)",
                params![entry.digest.as_slice(), w, h, image.png_bytes()],
            )?;
        }
    }
    Ok(refs)
}
pub(crate) fn link(
    connection: &Connection,
    revision: i64,
    refs: &BTreeSet<String>,
) -> Result<(), StoreError> {
    for key in refs {
        connection.execute(
            "INSERT INTO revision_media VALUES(?1,?2)",
            params![revision, parse_key(key)?],
        )?;
    }
    Ok(())
}
pub(crate) fn validate(
    connection: &Connection,
    revision: i64,
    config: &Configuration,
) -> Result<(), StoreError> {
    let expected = references(config);
    if expected.len() > COUNT {
        return Err(StoreError::Capacity);
    }
    let mut query =
        connection.prepare("SELECT lower(hex(media)) FROM revision_media WHERE revision=?1")?;
    let actual = query
        .query_map([revision], |r| r.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != expected {
        return Err(StoreError::Corrupt);
    }
    let mut total = 0usize;
    for key in expected {
        total = total
            .checked_add(read(connection, &key)?.png_bytes().len())
            .ok_or(StoreError::Capacity)?;
        if total > BYTES {
            return Err(StoreError::Capacity);
        }
    }
    Ok(())
}
