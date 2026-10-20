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
#[test]
fn static_png_and_jpeg_become_owned_png_without_modifying_input() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        let input = encoded(12, 6, format);
        let original = input.clone();
        let expected = pixels(&input);
        let normalized = normalize(&input).unwrap();
        assert_eq!(input, original);
        assert_eq!(normalized.dimensions(), (12, 6));
        assert_eq!(
            image::guess_format(normalized.png_bytes()).unwrap(),
            ImageFormat::Png
        );
        assert_eq!(pixels(normalized.png_bytes()), expected);
        drop(input);
        assert_eq!(pixels(normalized.png_bytes()).dimensions(), (12, 6));
    }
}

use rad_media::MediaError;
#[test]
fn empty_and_non_png_jpeg_formats_are_explicitly_rejected() {
    assert_eq!(normalize(&[]), Err(MediaError::EmptyInput));
    for input in [
        b"<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
        b"GIF89a\x01\0\x01\0\0\0\0;",
        b"\0\0\0\x18ftypmp42",
        b"RIFF\0\0\0\0WEBP",
        b"not an image",
    ] {
        assert_eq!(normalize(input), Err(MediaError::UnsupportedFormat));
    }
}

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
fn sized_image(format: ImageFormat, size: usize) -> Vec<u8> {
    let base = encoded(1, 1, format);
    if format == ImageFormat::Png {
        let mut text = b"note\0".to_vec();
        text.resize(size - base.len() - 12, b'X');
        return with_png_chunk(&base, b"tEXt", &text);
    }
    let mut result = base[..2].to_vec();
    let mut remaining = size - base.len();
    while remaining > 0 {
        let segment = if remaining > 65537 && remaining - 65537 < 4 {
            65533
        } else {
            remaining.min(65537)
        };
        assert!(segment >= 4);
        result.extend_from_slice(&[255, 254]);
        result.extend_from_slice(&((segment - 2) as u16).to_be_bytes());
        result.resize(result.len() + segment - 4, b'X');
        remaining -= segment;
    }
    result.extend_from_slice(&base[2..]);
    result
}
#[test]
fn real_png_and_jpeg_accept_exactly_eight_mib_and_reject_one_byte_more() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        for size in [8 * 1024 * 1024, 8 * 1024 * 1024 + 1] {
            let input = sized_image(format, size);
            assert_eq!(input.len(), size);
            assert_eq!(pixels(&input).dimensions(), (1, 1));
            if size == 8 * 1024 * 1024 {
                assert_eq!(normalize(&input).unwrap().dimensions(), (1, 1));
            } else {
                assert_eq!(normalize(&input), Err(MediaError::InputTooLarge));
            }
        }
    }
}

#[test]
fn each_input_axis_accepts_2048_and_rejects_2049_for_both_formats() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        for (width, height) in [(2048, 1), (1, 2048), (2049, 1), (1, 2049)] {
            let input = encoded(width, height, format);
            assert!(input.len() < 8 * 1024 * 1024);
            assert_eq!(pixels(&input).dimensions(), (width, height));
            if width == 2049 || height == 2049 {
                assert_eq!(
                    normalize(&input).err(),
                    Some(MediaError::DimensionsExceeded)
                );
            } else {
                assert!(normalize(&input).is_ok());
            }
        }
    }
}

#[test]
fn resizing_fits_both_axes_keeps_proportions_and_all_image_edges() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        for (width, height, expected) in [
            (512, 256, (256, 128)),
            (256, 512, (128, 256)),
            (2048, 1, (256, 1)),
            (1, 2048, (1, 256)),
            (7, 3, (7, 3)),
        ] {
            let normalized = normalize(&encoded(width, height, format)).unwrap();
            assert_eq!(normalized.dimensions(), expected);
        }
    }
    let striped = RgbaImage::from_fn(512, 256, |x, _| {
        if x < 256 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 128])
        }
    });
    let mut data = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(striped)
        .write_to(&mut data, ImageFormat::Png)
        .unwrap();
    let normalized = normalize(data.get_ref()).unwrap();
    let output = pixels(normalized.png_bytes());
    assert_eq!(*output.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
    assert_eq!(*output.get_pixel(255, 127), image::Rgba([0, 0, 255, 128]));
}

#[test]
fn real_apng_is_rejected_but_animation_marker_inside_text_is_not() {
    let mut apng = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut apng, 3, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255; 24]).unwrap();
        writer.write_image_data(&[0; 24]).unwrap();
        writer.finish().unwrap();
    }
    let info = png::Decoder::new(Cursor::new(&apng)).read_info().unwrap();
    assert_eq!(info.info().animation_control.unwrap().num_frames, 2);
    assert_eq!(normalize(&apng), Err(MediaError::AnimatedUnsupported));
    let static_png = with_png_chunk(
        &encoded(1, 1, ImageFormat::Png),
        b"tEXt",
        b"note\0acTL fcTL fdAT",
    );
    assert!(normalize(&static_png).is_ok());
}

#[test]
fn malformed_png_chunks_crc_truncation_and_zero_dimensions_are_rejected() {
    let valid = encoded(1, 1, ImageFormat::Png);
    let mut bad_crc = valid.clone();
    *bad_crc.last_mut().unwrap() ^= 1; // IEND is beyond the frame pixels.
    assert_eq!(normalize(&bad_crc), Err(MediaError::DecodeFailed));
    assert_eq!(
        normalize(&valid[..valid.len() - 1]),
        Err(MediaError::DecodeFailed)
    );
    let mut huge_chunk = valid.clone();
    huge_chunk[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(normalize(&huge_chunk), Err(MediaError::DecodeFailed));
    for axis in [0..4, 4..8] {
        let mut header = valid[16..29].to_vec();
        header[axis].fill(0);
        let zero = [
            &valid[..8],
            png_chunk(b"IHDR", &header).as_slice(),
            &valid[33..],
        ]
        .concat();
        assert_eq!(normalize(&zero), Err(MediaError::DecodeFailed));
    }
    let mut bad_pixels = valid.clone();
    bad_pixels[45] ^= 0xff;
    assert_eq!(normalize(&bad_pixels), Err(MediaError::DecodeFailed));
    let late_animation = [
        &valid[..valid.len() - 12],
        png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]).as_slice(),
        &valid[valid.len() - 12..],
    ]
    .concat();
    assert_eq!(
        normalize(&late_animation),
        Err(MediaError::AnimatedUnsupported)
    );
}

#[test]
fn jpeg_requires_a_complete_single_image_not_a_truncated_or_concatenated_stream() {
    let jpeg = encoded(4, 3, ImageFormat::Jpeg);
    assert_eq!(
        normalize(&[jpeg.as_slice(), jpeg.as_slice()].concat()),
        Err(MediaError::DecodeFailed)
    );
    assert_eq!(
        normalize(&jpeg[..jpeg.len() - 2]),
        Err(MediaError::DecodeFailed)
    );
    assert_eq!(
        normalize(&jpeg[..jpeg.len() / 2]),
        Err(MediaError::DecodeFailed)
    );
    // EOI-like bytes inside a legal COM payload are not the stream terminator.
    let comment = [255, 254, 0, 6, 255, 217, 255, 216];
    let commented = [&jpeg[..2], &comment, &jpeg[2..]].concat();
    assert!(normalize(&commented).is_ok());
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

#[test]
fn normalized_png_can_be_reconstructed_without_exposing_mutable_fields() {
    let original = normalize(&encoded(512, 256, ImageFormat::Png)).unwrap();
    let restored = rad_media::NormalizedImage::validate(original.png_bytes()).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn reconstruction_rejects_noncanonical_metadata_jpeg_and_oversized_input() {
    let normal = normalize(&encoded(2, 1, ImageFormat::Png)).unwrap();
    let tagged = with_png_chunk(normal.png_bytes(), b"tEXt", b"author\0private");
    for bytes in [
        tagged,
        encoded(2, 1, ImageFormat::Jpeg),
        encoded(512, 256, ImageFormat::Png),
    ] {
        assert!(rad_media::NormalizedImage::validate(&bytes).is_err());
    }
    let padded = sized_image(ImageFormat::Png, 512 * 1024 + 1);
    assert!(rad_media::NormalizedImage::validate(&padded).is_err());
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
