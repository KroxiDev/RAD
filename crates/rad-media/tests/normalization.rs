use image::{DynamicImage, ImageFormat, RgbaImage};
use rad_media::normalize;
use std::io::Cursor;
fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        width,
        height,
        image::Rgb([210, 40, 70]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}
fn pixels(bytes: &[u8]) -> RgbaImage {
    image::load_from_memory(bytes).unwrap().to_rgba8()
}

use rad_media::MediaError;

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    result.extend_from_slice(&(data.len() as u32).to_be_bytes());
    result.extend_from_slice(kind);
    result.extend_from_slice(data);
    result.extend_from_slice(&crc32fast::hash(&result[4..]).to_be_bytes());
    result
}
fn with_png_chunk(image: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    // Place metadata after IHDR and before IDAT.
    [&image[..33], png_chunk(kind, data).as_slice(), &image[33..]].concat()
}

fn profile_fixture(expanded: usize) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    let block = [0; 8192];
    for _ in 0..expanded / block.len() {
        encoder.write_all(&block).unwrap();
    }
    encoder.write_all(&block[..expanded % block.len()]).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!((32768..65520).contains(&compressed.len()));
    let mut profile = b"rad\0\0".to_vec();
    profile.extend_from_slice(&compressed);
    let mut base = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
        .write_to(&mut base, ImageFormat::Png)
        .unwrap();
    with_png_chunk(base.get_ref(), b"iCCP", &profile)
}
#[test]
fn png_decoder_budget_is_enforced_at_the_real_locked_metadata_boundary() {
    // Locked png0.18.1 reserves65408 for compressed chunk growth and4 for RGBA row.
    // This is decoder accounting, not RSS or a validated colour profile.
    let at_budget = profile_fixture(67_043_452);
    let over_budget = profile_fixture(67_043_453);
    assert!(at_budget.len() < 8 * 1024 * 1024);
    assert_eq!(normalize(&at_budget).unwrap().dimensions(), (1, 1));
    assert_eq!(normalize(&over_budget), Err(MediaError::DecoderLimit));
}

// Characterization/regression of behavior already green, not additional TDD cycles.
#[test]
fn strips_metadata_and_has_deterministic_reachable_encoder_bound() {
    let mut seed = 17u32;
    let noise = RgbaImage::from_fn(256, 256, |_, _| {
        let mut pixel = [0; 4];
        for byte in &mut pixel {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        image::Rgba(pixel)
    });
    let mut source = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(noise.clone())
        .write_to(&mut source, ImageFormat::Png)
        .unwrap();
    let tagged = with_png_chunk(source.get_ref(), b"tEXt", b"author\0private");
    let first = normalize(&tagged).unwrap();
    assert_eq!(pixels(first.png_bytes()), noise);
    assert_eq!(first, normalize(&tagged).unwrap());
    assert_eq!(first, normalize(source.get_ref()).unwrap());
    // Stored DEFLATE fallback:262400 raw +25 block +6 zlib +57 PNG bytes.
    assert_eq!(first.png_bytes().len(), 262_488);
    assert!(first.png_bytes().len() < 512 * 1024);
    let mut offset = 8;
    while offset < first.png_bytes().len() {
        let bytes = first.png_bytes();
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        assert!(matches!(
            &bytes[offset + 4..offset + 8],
            b"IHDR" | b"IDAT" | b"IEND"
        ));
        offset += length + 12;
    }
    let jpeg = encoded(10, 5, ImageFormat::Jpeg);
    let commented = [&jpeg[..2], &[255, 254, 0, 9], b"private", &jpeg[2..]].concat();
    assert_eq!(normalize(&jpeg).unwrap(), normalize(&commented).unwrap());
}

#[test]
fn largest_rgba16_pixel_buffer_is_bounded_and_normalized() {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2048, 2048);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![255; 2048 * 2048 * 8])
            .unwrap();
    }
    assert!(bytes.len() <= 8 * 1024 * 1024);
    let normal = normalize(&bytes).unwrap();
    assert_eq!(normal.dimensions(), (256, 256));
    assert_eq!(
        rad_media::NormalizedImage::validate(normal.png_bytes()).unwrap(),
        normal
    );
}
