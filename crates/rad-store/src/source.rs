use rad_media::{MediaEntry, MediaReadError, MediaSource, NormalizedImage};

pub(crate) struct SliceSource<'a> {
    entries: Vec<MediaEntry>,
    images: std::collections::BTreeMap<[u8; 32], &'a NormalizedImage>,
}
impl<'a> SliceSource<'a> {
    pub fn new(images: &'a [NormalizedImage]) -> Result<Self, crate::StoreError> {
        if images.len() > 512
            || images
                .iter()
                .try_fold(0usize, |n, i| n.checked_add(i.png_bytes().len()))
                .is_none_or(|n| n > 128 * 1024 * 1024)
        {
            return Err(crate::StoreError::Capacity);
        }
        let mut unique = std::collections::BTreeMap::new();
        for image in images {
            let hash: [u8; 32] = crate::digest(image.png_bytes()).try_into().unwrap();
            if let Some(old) = unique.insert(hash, image)
                && old.png_bytes() != image.png_bytes()
            {
                return Err(crate::StoreError::Corrupt);
            }
        }
        let entries = unique
            .iter()
            .map(|(digest, image)| MediaEntry {
                digest: *digest,
                length: image.png_bytes().len() as u32,
            })
            .collect();
        Ok(Self {
            entries,
            images: unique,
        })
    }
}
impl MediaSource for SliceSource<'_> {
    fn catalog(&self) -> &[MediaEntry] {
        &self.entries
    }
    fn read(&mut self, key: &[u8; 32]) -> Result<NormalizedImage, MediaReadError> {
        self.images
            .get(key)
            .map(|i| (*i).clone())
            .ok_or(MediaReadError::Invalid)
    }
}
