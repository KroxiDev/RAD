use crate::NormalizedImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaEntry {
    pub digest: [u8; 32],
    pub length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaReadError {
    Invalid,
    Io,
}

/// Reiterable prepared source, not a client callback or live upload.
/// The catalog and backing bytes must remain stable throughout a call.
pub trait MediaSource {
    fn catalog(&self) -> &[MediaEntry];
    fn read(&mut self, key: &[u8; 32]) -> Result<NormalizedImage, MediaReadError>;
}
