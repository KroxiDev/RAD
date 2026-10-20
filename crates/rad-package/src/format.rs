use crate::PackageError as Error;
use rad_core::configuration::Configuration;
use rad_store::{MediaEntry, MediaSource};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write};
pub(crate) const PACKAGE: u64 = 256 * 1024 * 1024;
pub(crate) const DOCUMENT: usize = 1024 * 1024;
pub(crate) const MEDIA: u64 = 128 * 1024 * 1024;
pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub(crate) fn key(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn references(c: &Configuration) -> BTreeSet<String> {
    c.data()
        .buttons
        .iter()
        .filter_map(|b| b.appearance.image.clone())
        .collect()
}
pub(crate) fn catalog(
    c: &Configuration,
    source: &dyn MediaSource,
) -> Result<Vec<MediaEntry>, Error> {
    let entries = source.catalog();
    if entries.len() > 512 {
        return Err(Error::LimitExceeded);
    }
    let mut entries = entries.to_vec();
    entries.sort_by_key(|e| e.digest);
    let mut sum = 0u64;
    let mut keys = BTreeSet::new();
    for e in &entries {
        if e.length == 0 || e.length > 512 * 1024 {
            return Err(Error::LimitExceeded);
        }
        sum = sum
            .checked_add(e.length as u64)
            .ok_or(Error::LimitExceeded)?;
        if !keys.insert(key(&e.digest)) {
            return Err(Error::InvalidPackage);
        }
    }
    if sum > MEDIA {
        return Err(Error::LimitExceeded);
    }
    if keys != references(c) {
        return Err(Error::InvalidPackage);
    }
    Ok(entries)
}
pub(crate) fn emit(out: &mut impl Write, hash: &mut Sha256, bytes: &[u8]) -> Result<(), Error> {
    for part in bytes.chunks(65536) {
        out.write_all(part).map_err(|_| Error::Io)?;
        hash.update(part);
    }
    Ok(())
}
