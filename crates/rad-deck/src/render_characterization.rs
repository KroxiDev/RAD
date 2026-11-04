use crate::{KeyVisual, TextPlacement, render};
use image::ImageEncoder;
use std::sync::Arc;

// Captured from the pre-style renderer: the default style must preserve every pixel.
#[test]
fn default_text_rendering_preserves_baseline_pixels() {
    let image = image::RgbaImage::from_fn(96, 96, |x, y| {
        image::Rgba([(x * 2) as u8, (y * 2) as u8, (x + y) as u8, 255])
    });
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), 96, 96, image::ExtendedColorType::Rgba8)
        .unwrap();
    let picture = Arc::new(rad_media::normalize(&png).unwrap());
    let expected = [
        0x4bb48f83b6620d08,
        0x3a2f5a94b5e27f20,
        0x91109bb2b2e07771,
        0x9f83e28a8c750d05,
        0x4fdc0c9b1c666e08,
        0x58c1de081f2b0da0,
        0x03e890afccf1af6f,
        0x5f69cd462821cf61,
        0x2cd1d126d275cf08,
        0xc3ec6d3b543aaaa0,
        0xbdc91d936c920dca,
        0x1f5fbb9a06e8772e,
        0x4bb48f83b6620d08,
        0x3a2f5a94b5e27f20,
        0xce65a3634b2350dc,
        0xc7b0a32f8966ba64,
        0xc962e8eef8b73208,
        0x49b8d4d67745cc20,
        0xbaa5691ac3992158,
        0x34078aad58e91530,
    ];
    let mut expected = expected.into_iter();
    for placement in [
        TextPlacement::Top,
        TextPlacement::Middle,
        TextPlacement::Bottom,
        TextPlacement::AbovePicture,
        TextPlacement::BelowPicture,
    ] {
        for with_image in [false, true] {
            for text in ["RAD", "First\nSecond"] {
                let key = KeyVisual::new(text, with_image.then(|| picture.clone()), false, false)
                    .text_at(placement);
                let pixels = render::rgb(&key).unwrap();
                assert_eq!(pixels.dimensions(), (96, 96));
                // Explicit FNV-1a keeps the fixture independent of Rust's DefaultHasher.
                let hash = pixels
                    .as_raw()
                    .iter()
                    .fold(0xcbf29ce484222325_u64, |hash, byte| {
                        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                    });
                assert_eq!(
                    hash,
                    expected.next().unwrap(),
                    "{placement:?}, image={with_image}, text={text:?}"
                );
            }
        }
    }
}

#[test]
fn text_color_and_disabled_band_reach_pixels() {
    let image = image::RgbImage::from_pixel(96, 96, image::Rgb([70, 100, 140]));
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), 96, 96, image::ExtendedColorType::Rgb8)
        .unwrap();
    let picture = Arc::new(rad_media::normalize(&png).unwrap());
    let key = KeyVisual::new("MMMM", Some(picture), false, false)
        .text_at(TextPlacement::Middle)
        .text_style(13, [255, 0, 0], false);
    let pixels = render::rgb(&key).unwrap();
    assert_eq!(
        pixels.get_pixel(0, 48).0,
        [70, 100, 140],
        "disabled band leaves the image intact"
    );
    assert!(
        pixels
            .pixels()
            .any(|p| p[0] > 200 && p[1] < 30 && p[2] < 40),
        "text blends toward its selected color"
    );
}

#[test]
fn two_lines_at_size_limits_fit_without_clipping() {
    for size in [8, 24] {
        for placement in [
            TextPlacement::Top,
            TextPlacement::Middle,
            TextPlacement::Bottom,
            TextPlacement::AbovePicture,
            TextPlacement::BelowPicture,
        ] {
            let count = |text| {
                render::rgb(
                    &KeyVisual::new(text, None, false, false)
                        .text_at(placement)
                        .text_style(size, [255; 3], true),
                )
                .unwrap()
                .pixels()
                .filter(|p| p.0 != [0; 3])
                .count()
            };
            assert_eq!(
                count("M\nM"),
                2 * count("M"),
                "size={size}, placement={placement:?}"
            );
        }
    }
}

#[test]
fn selected_color_tints_the_text() {
    let pixels =
        render::rgb(&KeyVisual::new("MMMM", None, false, false).text_style(13, [255, 0, 0], true))
            .unwrap();
    assert!(
        pixels
            .pixels()
            .any(|p| p[0] > 200 && p[1] == 0 && p[2] == 0)
    );
}

#[test]
fn selected_size_changes_glyph_height() {
    let height = |size| {
        let pixels = render::rgb(
            &KeyVisual::new("M", None, false, false)
                .text_at(TextPlacement::Middle)
                .text_style(size, [255; 3], true),
        )
        .unwrap();
        let rows: Vec<_> = (0..96)
            .filter(|y| (0..96).any(|x| pixels.get_pixel(x, *y).0 != [0; 3]))
            .collect();
        rows.last().unwrap() - rows.first().unwrap() + 1
    };
    assert!(height(8) <= 8, "8 px text must have a small glyph");
    assert!(height(24) >= 16, "24 px text must have a larger glyph");
}
