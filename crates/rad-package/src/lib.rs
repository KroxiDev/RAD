//! Bounded package streams only; caller owns Origin, confirmation, leases and publication.
//!
//! Blocking IO belongs on the transfer/Store workers. The adapter supplies deadline-aware
//! Read/Write, the global export+restore admission limit and the per-connection quota.
//! A successful export receipt is not permission to publish a partial destination file.
//! No package payload is interpreted as a path, command or installed plugin.
mod restore;
mod staging;
pub use restore::{PreparedMedia, PreparedRestore, RestoreSummary, prepare};
pub use staging::StagingArea;
mod format;
use rad_core::configuration::Configuration;
use rad_store::MediaSource;
use sha2::{Digest, Sha256};
use std::io::Write;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageError {
    InvalidPackage,
    UnsupportedVersion,
    LimitExceeded,
    InvalidConfiguration,
    IdentityCollision,
    Io,
    Capacity,
    NotAllowed,
    ConfirmationRequired,
    UnsafeStaging,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportReceipt {
    pub total_len: u64,
    pub digest: [u8; 32],
}
/// Trusted in-process snapshot, not a client DTO. Caller retains the matching revision
/// lease until export returns, preventing GC while this exact media catalogue is read.
/// No SQLite transaction may be held while Write waits on a client.
pub struct ExportSnapshot<'a> {
    pub configuration: &'a Configuration,
    pub media: &'a mut dyn MediaSource,
}
pub fn export(
    snapshot: ExportSnapshot<'_>,
    out: &mut impl Write,
) -> Result<ExportReceipt, PackageError> {
    let document = rad_codec::encode(snapshot.configuration)
        .map_err(|_| PackageError::InvalidConfiguration)?;
    let catalog = format::catalog(snapshot.configuration, snapshot.media)?;
    let original = snapshot.media.catalog().to_vec();
    let total_len =
        92 + document.len() as u64 + catalog.iter().map(|e| 36 + e.length as u64).sum::<u64>();
    if total_len > format::PACKAGE {
        return Err(PackageError::LimitExceeded);
    }
    let mut header = Vec::with_capacity(60);
    header.extend_from_slice(b"RADPKG\r\n");
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&0u16.to_be_bytes());
    header.extend_from_slice(&total_len.to_be_bytes());
    header.extend_from_slice(&(document.len() as u32).to_be_bytes());
    header.extend_from_slice(&(catalog.len() as u16).to_be_bytes());
    header.extend_from_slice(&0u16.to_be_bytes());
    header.extend_from_slice(&format::digest(&document));
    let mut hash = Sha256::new();
    format::emit(out, &mut hash, &header)?;
    format::emit(out, &mut hash, &document)?;
    for entry in catalog {
        if snapshot.media.catalog() != original {
            return Err(PackageError::InvalidPackage);
        }
        let image = snapshot.media.read(&entry.digest).map_err(|e| match e {
            rad_store::MediaReadError::Io => PackageError::Io,
            _ => PackageError::InvalidPackage,
        })?;
        if snapshot.media.catalog() != original
            || image.png_bytes().len() != entry.length as usize
            || format::digest(image.png_bytes()) != entry.digest
        {
            return Err(PackageError::InvalidPackage);
        }
        rad_media::NormalizedImage::validate(image.png_bytes())
            .map_err(|_| PackageError::InvalidPackage)?;
        format::emit(out, &mut hash, &entry.digest)?;
        format::emit(out, &mut hash, &entry.length.to_be_bytes())?;
        format::emit(out, &mut hash, image.png_bytes())?;
    }
    let digest: [u8; 32] = hash.finalize().into();
    out.write_all(&digest).map_err(|_| PackageError::Io)?;
    Ok(ExportReceipt { total_len, digest })
}
