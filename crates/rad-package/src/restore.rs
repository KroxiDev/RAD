use crate::{IdentityMetadata, PackageError as Error, StagingArea, format, staging::StageFile};
use rad_core::{
    configuration::{Candidate, Configuration, ConfigurationError, Origin},
    identity::IdentityRegistry,
};
use rad_media::{MediaEntry, MediaReadError, MediaSource};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSummary {
    pub plugins: Vec<PluginReference>,
    pub entities: usize,
    pub media_count: usize,
    pub media_bytes: u64,
    pub document_bytes: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginReference {
    pub id: String,
    pub version: Option<rad_core::configuration::PluginVersion>,
    pub origin: Option<String>,
}
#[derive(Debug)]
pub struct VerifiedRestore {
    pub data: rad_core::configuration::ConfigurationData,
    pub identity: IdentityMetadata,
}
/// Single-use preparation; dropping it cancels and attempts accounted cleanup.
/// It reserves no identities and cannot publish a configuration.
pub struct PreparedRestore {
    media: PreparedMedia,
    document_len: usize,
    version: u16,
    summary: RestoreSummary,
}
/// Owns staging after confirmation. Keep it alive while Store is pending/Uncertain;
/// only release after the outcome is resolved. Store revalidates every media read.
pub struct PreparedMedia {
    stage: StageFile,
    entries: Vec<MediaEntry>,
    offsets: Vec<u64>,
    total: u64,
    digest: [u8; 32],
}
fn exact(input: &mut impl Read, bytes: &mut [u8]) -> Result<(), Error> {
    for chunk in bytes.chunks_mut(65536) {
        input.read_exact(chunk).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::InvalidPackage
            } else {
                Error::Io
            }
        })?;
    }
    Ok(())
}
fn eof(input: &mut impl Read) -> Result<(), Error> {
    let mut b = [0; 1];
    loop {
        match input.read(&mut b) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(Error::InvalidPackage),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Io),
        }
    }
}
fn validated(document: &[u8]) -> Result<Configuration, Error> {
    let data = rad_codec::decode_data(document).map_err(|error| match error {
        rad_codec::CodecError::UnsupportedVersion => Error::UnsupportedVersion,
        _ => Error::InvalidConfiguration,
    })?;
    let bindings: Vec<_> =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .collect();
    let registry = IdentityRegistry::new(data.principal.installation())
        .map_err(|_| Error::InvalidConfiguration)?
        .restore(&bindings)
        .map_err(|_| Error::InvalidConfiguration)?;
    let config =
        Configuration::from_data(data, &registry).map_err(|_| Error::InvalidConfiguration)?;
    rad_codec::encode(&config).map_err(|_| Error::InvalidConfiguration)?;
    Ok(config)
}
/// Reads an exact finite stream; a deadline-aware adapter must interrupt stalled IO.
pub fn prepare(input: &mut impl Read, area: &StagingArea) -> Result<PreparedRestore, Error> {
    let mut header = [0; 60];
    exact(input, &mut header)?;
    if &header[..8] != b"RADPKG\r\n" {
        return Err(Error::InvalidPackage);
    }
    let version = u16::from_be_bytes(header[8..10].try_into().unwrap());
    if !matches!(version, 1 | 2) {
        return Err(Error::UnsupportedVersion);
    }
    let metadata_len = if version == 2 { 48 } else { 0 };
    if header[10..12] != [0, 0]
        || u16::from_be_bytes(header[26..28].try_into().unwrap()) != metadata_len
    {
        return Err(Error::InvalidPackage);
    }
    let total = u64::from_be_bytes(header[12..20].try_into().unwrap());
    let document_len = u32::from_be_bytes(header[20..24].try_into().unwrap()) as usize;
    let count = u16::from_be_bytes(header[24..26].try_into().unwrap()) as usize;
    if total > format::PACKAGE || document_len > format::DOCUMENT || count > 512 {
        return Err(Error::LimitExceeded);
    }
    if document_len == 0
        || total < 92 + document_len as u64 + u64::from(metadata_len) + 36 * count as u64
    {
        return Err(Error::InvalidPackage);
    }
    let mut stage = area.reserve(total)?;
    let mut hash = Sha256::new();
    format::emit(stage.file(), &mut hash, &header)?;
    let mut document = vec![0; document_len];
    exact(input, &mut document)?;
    if format::digest(&document) != header[28..60] {
        return Err(Error::InvalidPackage);
    }
    let config = validated(&document)?;
    let refs = format::references(&config);
    if refs.len() != count {
        return Err(Error::InvalidPackage);
    }
    let entities = config.data().folders.len()
        + config
            .data()
            .buttons
            .iter()
            .map(|b| 1 + b.actions.len())
            .sum::<usize>();
    format::emit(stage.file(), &mut hash, &document)?;
    if version == 2 {
        let mut metadata = [0; 48];
        exact(input, &mut metadata)?;
        IdentityMetadata::decode(&metadata, &config)?;
        format::emit(stage.file(), &mut hash, &metadata)?;
    }
    let plugins = referenced_plugins(&config);
    drop(document);
    let mut entries = Vec::with_capacity(count);
    let mut offsets = Vec::with_capacity(count);
    let mut position = 60 + document_len as u64 + u64::from(metadata_len);
    let mut media_bytes = 0u64;
    let mut last = None;
    for _ in 0..count {
        let mut record = [0; 36];
        exact(input, &mut record)?;
        let digest: [u8; 32] = record[..32].try_into().unwrap();
        let length = u32::from_be_bytes(record[32..].try_into().unwrap());
        if length == 0 || length > 512 * 1024 {
            return Err(Error::LimitExceeded);
        }
        media_bytes = media_bytes
            .checked_add(length as u64)
            .ok_or(Error::LimitExceeded)?;
        if media_bytes > format::MEDIA {
            return Err(Error::LimitExceeded);
        }
        if last.is_some_and(|prior| prior >= digest) || !refs.contains(&format::key(&digest)) {
            return Err(Error::InvalidPackage);
        }
        last = Some(digest);
        position = position.checked_add(36).ok_or(Error::LimitExceeded)?;
        if position + length as u64 > total - 32 {
            return Err(Error::InvalidPackage);
        }
        format::emit(stage.file(), &mut hash, &record)?;
        offsets.push(position);
        let mut bytes = vec![0; length as usize];
        exact(input, &mut bytes)?;
        if format::digest(&bytes) != digest {
            return Err(Error::InvalidPackage);
        }
        rad_media::NormalizedImage::validate(&bytes).map_err(|_| Error::InvalidPackage)?;
        format::emit(stage.file(), &mut hash, &bytes)?;
        position += length as u64;
        entries.push(MediaEntry { digest, length });
    }
    if position != total - 32 {
        return Err(Error::InvalidPackage);
    }
    let mut digest = [0; 32];
    exact(input, &mut digest)?;
    if hash.finalize().as_slice() != digest {
        return Err(Error::InvalidPackage);
    }
    eof(input)?;
    stage.file().write_all(&digest).map_err(|_| Error::Io)?;
    Ok(PreparedRestore {
        media: PreparedMedia {
            stage,
            entries,
            offsets,
            total,
            digest,
        },
        document_len,
        version,
        summary: RestoreSummary {
            plugins,
            entities,
            media_count: count,
            media_bytes,
            document_bytes: document_len,
        },
    })
}
impl PreparedRestore {
    /// Worker-only verification and extraction. Data must still pass Authority validation.
    pub fn into_verified_data(mut self) -> Result<(VerifiedRestore, PreparedMedia), Error> {
        self.media.verify()?;
        self.media
            .stage
            .file()
            .seek(SeekFrom::Start(60))
            .map_err(|_| Error::Io)?;
        let mut document = vec![0; self.document_len];
        exact(self.media.stage.file(), &mut document)?;
        let config = validated(&document)?;
        let identity = if self.version == 2 {
            let mut bytes = [0; 48];
            exact(self.media.stage.file(), &mut bytes)?;
            IdentityMetadata::decode(&bytes, &config)?
        } else {
            IdentityMetadata::legacy(&config)?
        };
        Ok((
            VerifiedRestore {
                data: config.data().clone(),
                identity,
            },
            self.media,
        ))
    }
    pub fn summary(&self) -> RestoreSummary {
        self.summary.clone()
    }
    pub fn discard(self) -> Result<(), Error> {
        self.media.discard()
    }
    /// Consume once against the CURRENT serialized configuration and full ledger.
    /// Origin comes from the endpoint, never a client role field. This returns a
    /// candidate only: the caller must commit via Store before publishing success.
    pub fn confirm(
        self,
        current: &Configuration,
        registry: &IdentityRegistry,
        origin: Origin,
        confirmed: bool,
    ) -> Result<(Candidate, PreparedMedia), Error> {
        if origin != Origin::Editor {
            return Err(Error::NotAllowed);
        }
        if !confirmed {
            return Err(Error::ConfirmationRequired);
        }
        let (verified, media) = self.into_verified_data()?;
        let restored = current
            .prepare_portable_restore(
                registry,
                origin,
                confirmed,
                verified.data,
                verified.identity.installation,
                verified.identity.counters,
            )
            .map_err(|e| match e {
                ConfigurationError::Identity(_) => Error::IdentityCollision,
                ConfigurationError::NotAllowed => Error::NotAllowed,
                ConfigurationError::ConfirmationRequired => Error::ConfirmationRequired,
                _ => Error::InvalidConfiguration,
            })?;
        Ok((restored.candidate, media))
    }
}
impl PreparedMedia {
    pub fn discard(self) -> Result<(), Error> {
        self.stage.discard()
    }
    fn verify(&mut self) -> Result<(), Error> {
        let file = self.stage.file();
        if file.metadata().map_err(|_| Error::Io)?.len() != self.total {
            return Err(Error::InvalidPackage);
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| Error::Io)?;
        let mut remaining = self.total - 32;
        let mut bytes = [0; 65536];
        let mut hash = Sha256::new();
        while remaining > 0 {
            let n = remaining.min(bytes.len() as u64) as usize;
            exact(file, &mut bytes[..n])?;
            hash.update(&bytes[..n]);
            remaining -= n as u64;
        }
        let mut tail = [0; 32];
        exact(file, &mut tail)?;
        if hash.finalize().as_slice() != self.digest || tail != self.digest {
            return Err(Error::InvalidPackage);
        }
        eof(file)
    }
}
impl MediaSource for PreparedMedia {
    fn catalog(&self) -> &[MediaEntry] {
        &self.entries
    }
    fn read(&mut self, key: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        let i = self
            .entries
            .iter()
            .position(|e| &e.digest == key)
            .ok_or(MediaReadError::Invalid)?;
        let file = self.stage.file();
        file.seek(SeekFrom::Start(self.offsets[i]))
            .map_err(|_| MediaReadError::Io)?;
        let mut bytes = vec![0; self.entries[i].length as usize];
        exact(file, &mut bytes).map_err(|e| {
            if e == Error::InvalidPackage {
                MediaReadError::Invalid
            } else {
                MediaReadError::Io
            }
        })?;
        if format::digest(&bytes) != *key {
            return Err(MediaReadError::Invalid);
        }
        rad_media::NormalizedImage::validate(&bytes).map_err(|_| MediaReadError::Invalid)
    }
}

pub fn referenced_plugins(configuration: &Configuration) -> Vec<PluginReference> {
    let data = configuration.data();
    let mut used: std::collections::BTreeSet<String> =
        data.plugin_settings.keys().cloned().collect();
    for button in &data.buttons {
        if let rad_core::configuration::VisualSource::External { type_key, .. } = &button.visual {
            used.insert(type_key.plugin().into());
        }
        for action in &button.actions {
            if let rad_core::configuration::ActionKind::External { type_key, .. } = &action.kind {
                used.insert(type_key.plugin().into());
            }
        }
    }
    used.into_iter()
        .map(|id| {
            let dependency = data.plugin_dependencies.get(&id);
            PluginReference {
                id: id.clone(),
                version: dependency.and_then(|d| d.version),
                origin: dependency.and_then(|d| d.origin.clone()),
            }
        })
        .collect()
}
