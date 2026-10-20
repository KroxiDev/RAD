//! Bounded RADP packages; validation never executes package code.

use rad_plugin_contract::{Document, Manifest};
mod elf;

pub const MAX_MANIFEST_BYTES: usize = 65_536;
pub const MAX_EXECUTABLE_BYTES: usize = 64 * 1024 * 1024;
const HEADER_BYTES: usize = 20;

/// Validated metadata and borrowed payload: binary bytes are never copied by parsing.
/// Validation is structural, not a sandbox or a guarantee of a successful load.
pub struct Candidate<'a> {
    manifest: Manifest,
    manifest_bytes: &'a [u8],
    executable: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidHeader,
    InvalidSize,
    InvalidExecutable,
    UnsupportedTarget,
    Contract(rad_plugin_contract::Error),
}

impl<'a> Candidate<'a> {
    pub fn parse(bytes: &'a [u8], host_api: u32, target: &str) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != b"RADP\0\0\0\x01" {
            return Err(Error::InvalidHeader);
        }
        let manifest_len = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let executable_len = u64::from_be_bytes(bytes[12..20].try_into().unwrap());
        if !(1..=MAX_MANIFEST_BYTES).contains(&manifest_len)
            || !(1..=MAX_EXECUTABLE_BYTES as u64).contains(&executable_len)
        {
            return Err(Error::InvalidSize);
        }
        let split = HEADER_BYTES + manifest_len;
        if bytes.len() != split + executable_len as usize {
            return Err(Error::InvalidSize);
        }
        Self::from_parts(
            &bytes[HEADER_BYTES..split],
            &bytes[split..],
            host_api,
            target,
        )
    }

    pub fn from_parts(
        manifest_bytes: &'a [u8],
        executable: &'a [u8],
        host_api: u32,
        target: &str,
    ) -> Result<Self, Error> {
        if !(1..=MAX_MANIFEST_BYTES).contains(&manifest_bytes.len())
            || !(1..=MAX_EXECUTABLE_BYTES).contains(&executable.len())
        {
            return Err(Error::InvalidSize);
        }
        let manifest = Document::new(manifest_bytes)
            .and_then(|document| document.manifest(host_api, target))
            .map_err(Error::Contract)?;
        elf::validate(executable, target)?;
        Ok(Self {
            manifest,
            manifest_bytes,
            executable,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn manifest_bytes(&self) -> &[u8] {
        self.manifest_bytes
    }
    pub fn executable(&self) -> &[u8] {
        self.executable
    }
}

/// Encode a previously validated candidate in the sole RADP wire representation.
pub fn encode(candidate: &Candidate<'_>) -> Vec<u8> {
    let manifest = candidate.manifest_bytes();
    let executable = candidate.executable();
    let mut bytes = Vec::with_capacity(HEADER_BYTES + manifest.len() + executable.len());
    bytes.extend_from_slice(b"RADP\0\0\0\x01");
    bytes.extend_from_slice(&(manifest.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(executable.len() as u64).to_be_bytes());
    bytes.extend_from_slice(manifest);
    bytes.extend_from_slice(executable);
    bytes
}
