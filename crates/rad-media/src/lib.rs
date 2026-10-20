//! In-memory static image normalization. No filesystem or Store responsibilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedImage {
    png: Vec<u8>,
    width: u32,
    height: u32,
}
impl NormalizedImage {
    /// Reconstruct only canonical PNG bytes produced by this locked encoder.
    /// A future encoder migration must explicitly renormalize older encodings.
    pub fn validate(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() > 512 * 1024 {
            return Err(MediaError::OutputTooLarge);
        }
        let normalized = normalize(bytes)?;
        if normalized.png != bytes {
            return Err(MediaError::NotNormalized);
        }
        Ok(normalized)
    }
    pub fn png_bytes(&self) -> &[u8] {
        &self.png
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaError {
    EmptyInput,
    InputTooLarge,
    DimensionsExceeded,
    DecoderLimit,
    AnimatedUnsupported,
    UnsupportedFormat,
    DecodeFailed,
    EncodeFailed,
    OutputTooLarge,
    NotNormalized,
}
pub fn normalize(input: &[u8]) -> Result<NormalizedImage, MediaError> {
    use image::ImageEncoder;
    if input.len() > 8 * 1024 * 1024 {
        return Err(MediaError::InputTooLarge);
    }
    if input.is_empty() {
        return Err(MediaError::EmptyInput);
    }
    let format = image::guess_format(input).map_err(|_| MediaError::UnsupportedFormat)?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
        return Err(MediaError::UnsupportedFormat);
    }
    if format == image::ImageFormat::Png {
        check_static_png(input)?;
    } else {
        check_single_jpeg(input)?;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(input), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(decode_error)?;
    let decoded = if decoded.width() > 256 || decoded.height() > 256 {
        decoded.thumbnail(256, 256)
    } else {
        decoded
    };
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
    .map_err(|_| MediaError::EncodeFailed)?;
    if png.len() > 512 * 1024 {
        return Err(MediaError::OutputTooLarge);
    }
    Ok(NormalizedImage { png, width, height })
}

fn decode_error(error: image::ImageError) -> MediaError {
    match error {
        image::ImageError::Limits(error)
            if error.kind() == image::error::LimitErrorKind::DimensionError =>
        {
            MediaError::DimensionsExceeded
        }
        image::ImageError::Limits(_) => MediaError::DecoderLimit,
        _ => MediaError::DecodeFailed,
    }
}

// Inspect actual chunk boundaries, never arbitrary byte substrings.
fn check_static_png(input: &[u8]) -> Result<(), MediaError> {
    let mut offset = 8usize;
    while let Some(header) = input.get(offset..).and_then(|tail| tail.get(..8)) {
        let length = u32::from_be_bytes(
            header[..4]
                .try_into()
                .map_err(|_| MediaError::DecodeFailed)?,
        ) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .ok_or(MediaError::DecodeFailed)?;
        if end > input.len() {
            return Err(MediaError::DecodeFailed);
        }
        let checksum = u32::from_be_bytes(
            input[end - 4..end]
                .try_into()
                .map_err(|_| MediaError::DecodeFailed)?,
        );
        if crc32fast::hash(&input[offset + 4..end - 4]) != checksum {
            return Err(MediaError::DecodeFailed);
        }
        let kind = &header[4..8];
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(MediaError::AnimatedUnsupported);
        }
        if kind == b"IEND" {
            return if length == 0 && end == input.len() {
                Ok(())
            } else {
                Err(MediaError::DecodeFailed)
            };
        }
        offset = end;
    }
    Err(MediaError::DecodeFailed)
}

// Check the envelope only; pixel/scan correctness remains the JPEG decoder's job.
// Segment payloads are skipped so embedded marker bytes cannot terminate a file.
fn check_single_jpeg(input: &[u8]) -> Result<(), MediaError> {
    let mut offset = 2usize;
    let mut scan = false;
    while offset < input.len() {
        if scan {
            offset += input[offset..]
                .iter()
                .position(|&byte| byte == 0xff)
                .ok_or(MediaError::DecodeFailed)?;
        }
        if input.get(offset) != Some(&0xff) {
            return Err(MediaError::DecodeFailed);
        }
        while input.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *input.get(offset).ok_or(MediaError::DecodeFailed)?;
        offset += 1;
        match marker {
            0x00 | 0xd0..=0xd7 if scan => continue,
            0x01 => continue,
            0xd9 => {
                return if offset == input.len() {
                    Ok(())
                } else {
                    Err(MediaError::DecodeFailed)
                };
            }
            0x00 | 0xd0..=0xd8 => return Err(MediaError::DecodeFailed),
            _ => {}
        }
        let length_bytes = input
            .get(offset..offset + 2)
            .ok_or(MediaError::DecodeFailed)?;
        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return Err(MediaError::DecodeFailed);
        }
        offset = offset
            .checked_add(length)
            .filter(|&end| end <= input.len())
            .ok_or(MediaError::DecodeFailed)?;
        scan = marker == 0xda;
    }
    Err(MediaError::DecodeFailed)
}
