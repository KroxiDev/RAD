use crate::{DeckError, KeyVisual, TextPlacement};

const SIDE: u32 = 96;
/// Height a picture gets beside text at the top or the bottom, and the text band's start.
const PICTURE: u32 = 62;
const TEXT_BAND: u32 = SIDE - 32;

pub fn rgb(key: &KeyVisual) -> Result<image::RgbImage, DeckError> {
    let rgba = key.background.unwrap_or([0, 0, 0, 255]);
    let background = image::Rgb(std::array::from_fn(|c| {
        (u16::from(rgba[c]) * u16::from(rgba[3]) / 255) as u8
    }));
    let mut canvas = image::RgbImage::from_pixel(SIDE, SIDE, background);
    // Rows the picture may use: all of them unless the text is placed beside it.
    let (top, height) = match (key.text.is_empty(), key.text_placement) {
        (false, TextPlacement::BelowPicture) => (0, PICTURE),
        (false, TextPlacement::AbovePicture) => (SIDE - PICTURE, PICTURE),
        _ => (0, SIDE),
    };
    if let Some(image) = &key.image {
        let decoded =
            image::load_from_memory_with_format(image.png_bytes(), image::ImageFormat::Png)
                .map_err(|_| DeckError::Protocol)?;
        let fitted = decoded
            .resize(SIDE, height, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let x = (SIDE - fitted.width()) / 2;
        let y = top + (height - fitted.height()) / 2;
        for (dx, dy, p) in fitted.enumerate_pixels() {
            canvas.put_pixel(
                x + dx,
                y + dy,
                image::Rgb(std::array::from_fn(|c| {
                    ((u16::from(p[c]) * u16::from(p[3])
                        + u16::from(background[c]) * u16::from(255 - p[3]))
                        / 255) as u8
                })),
            );
        }
    }
    // The generic launcher picture needs room of its own: centred text leaves it none.
    let centred_text = !key.text.is_empty() && key.text_placement == TextPlacement::Middle;
    let generic = key.image.is_none() && key.generic_launcher && !centred_text;
    let moon = key.image.is_none() && key.sleep_icon;
    if moon {
        let center_y = top as f32 + height as f32 / 2.;
        let radius = 22.;
        for y in top..top + height {
            for x in 0..SIDE {
                let dx = x as f32 + 0.5 - 48.;
                let dy = y as f32 + 0.5 - center_y;
                if dx * dx + dy * dy <= radius * radius
                    && (dx - 11.).powi(2) + (dy + 8.).powi(2) > radius * radius
                {
                    canvas.put_pixel(x, y, image::Rgb([225, 230, 245]));
                }
            }
        }
    }
    if generic && !moon {
        let offset = top + (height - PICTURE) / 2;
        for y in 12..54 {
            for x in 22..74 {
                let c = if !(25..=70).contains(&x) || !(21..=50).contains(&y) {
                    180
                } else {
                    45
                };
                canvas.put_pixel(x, offset + y, image::Rgb([c, c, c]));
            }
        }
    }
    if !key.text.is_empty() {
        text(
            &mut canvas,
            &key.text,
            key.text_placement,
            key.image.is_some() || generic || moon,
        );
    }
    // An amber pause marks last-known content. Unavailable takes precedence.
    if key.stale && !key.unavailable {
        for y in 4..16 {
            for x in [80, 81, 82, 87, 88, 89] {
                canvas.put_pixel(x, y, image::Rgb([255, 190, 45]));
            }
        }
    }
    if key.unavailable {
        for i in 0..12 {
            for thick in 0..3 {
                canvas.put_pixel(79 + i, 3 + i + thick, image::Rgb([255, 45, 45]));
                canvas.put_pixel(90 - i, 3 + i + thick, image::Rgb([255, 45, 45]));
            }
        }
    }
    Ok(image::imageops::rotate180(&canvas))
}
/// Paints up to two centred rows. Where they lie over a picture, the rows behind them are
/// dimmed first, out to the edge of the key, so the text stays readable.
fn text(canvas: &mut image::RgbImage, value: &str, placement: TextPlacement, over_picture: bool) {
    use epaint::{
        Color32, FontId,
        text::{FontDefinitions, Fonts, LayoutJob, TextOptions},
    };
    let mut fonts = Fonts::new(
        TextOptions {
            max_texture_side: 1024,
            ..Default::default()
        },
        FontDefinitions::default(),
    );
    let mut job = LayoutJob::simple(
        value.into(),
        FontId::proportional(13.0),
        Color32::WHITE,
        92.0,
    );
    job.wrap.max_rows = 2;
    let galley = fonts.with_pixels_per_point(1.0).layout_job(job);
    let block = galley.size().y.min(32.0);
    let side = SIDE as f32;
    // Where the block starts, and the rows dimmed behind it when it lies over a picture.
    let (origin, band) = match placement {
        TextPlacement::Top => (2.0, Some(0.0..block + 5.0)),
        TextPlacement::Middle => {
            let origin = ((side - block) / 2.0).floor();
            (origin, Some(origin - 3.0..origin + block + 3.0))
        }
        TextPlacement::Bottom => {
            let origin = (side - block - 3.0).floor();
            (origin, Some(origin - 3.0..side))
        }
        TextPlacement::AbovePicture => (2.0, None),
        TextPlacement::BelowPicture => (TEXT_BAND as f32, None),
    };
    if let (Some(band), true) = (band, over_picture) {
        for y in band.start.max(0.0) as u32..(band.end.ceil() as u32).min(SIDE) {
            for x in 0..SIDE {
                let p = canvas.get_pixel_mut(x, y);
                p.0 = p.0.map(|c| (u16::from(c) * 2 / 5) as u8);
            }
        }
    }
    let atlas = fonts.image();
    for row in &galley.rows {
        let ox = (SIDE as f32 - row.size.x) / 2.0;
        for glyph in &row.glyphs {
            let uv = glyph.uv_rect;
            let width = uv.max[0] - uv.min[0];
            let height = uv.max[1] - uv.min[1];
            let left = (ox + row.pos.x + glyph.pos.x + uv.offset.x).round() as i32;
            let top = (origin + row.pos.y + glyph.pos.y + uv.offset.y).round() as i32;
            for y in 0..height {
                for x in 0..width {
                    let dx = left + x as i32;
                    let dy = top + y as i32;
                    if (0..SIDE as i32).contains(&dx) && (0..SIDE as i32).contains(&dy) {
                        let pixel = atlas.pixels
                            [(uv.min[1] + y) as usize * atlas.size[0] + (uv.min[0] + x) as usize];
                        let a = pixel.a();
                        let old = canvas.get_pixel(dx as u32, dy as u32);
                        let rgb = std::array::from_fn(|c| {
                            ((u16::from(old[c]) * (255 - u16::from(a)) + 255 * u16::from(a)) / 255)
                                as u8
                        });
                        canvas.put_pixel(dx as u32, dy as u32, image::Rgb(rgb));
                    }
                }
            }
        }
    }
}
pub fn jpeg(key: &KeyVisual) -> Result<Vec<u8>, DeckError> {
    let image = rgb(key)?;
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode(image.as_raw(), 96, 96, image::ExtendedColorType::Rgb8)
        .map_err(|_| DeckError::Protocol)?;
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use std::sync::Arc;
    #[test]
    fn sleep_crescent_is_visible_without_text_and_never_overwrites_a_custom_image() {
        let plain = KeyVisual::default();
        let moon = plain.clone().sleep_icon(true);
        let pixels = rgb(&moon).unwrap();
        let lit = pixels.pixels().filter(|p| p.0 != [0, 0, 0]).count();
        assert!(
            lit > 200 && lit < 1500,
            "a crescent, not blank or a full key"
        );
        assert_ne!(pixels, rgb(&plain).unwrap());
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let image = Arc::new(rad_media::normalize(&bytes).unwrap());
        let custom = KeyVisual::new("", Some(image), false, false);
        assert_eq!(
            rgb(&custom.clone().sleep_icon(true)).unwrap(),
            rgb(&custom).unwrap()
        );
    }
    #[test]
    fn image_is_letterboxed_and_rotated_for_xl_without_cropping() {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &[255, 0, 0, 255, 0, 255, 0, 255],
                2,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let normalized = rad_media::normalize(&bytes).unwrap();
        let before = normalized.png_bytes().to_vec();
        let key = KeyVisual::new("", Some(Arc::new(normalized)), false, false);
        let rgb = rgb(&key).unwrap();
        assert_eq!(rgb.get_pixel(10, 10).0, [0, 0, 0]);
        assert!(rgb.get_pixel(10, 48)[1] > 200);
        assert!(rgb.get_pixel(85, 48)[0] > 200);
        assert_eq!(key.image.as_ref().unwrap().png_bytes(), before);
        let encoded = jpeg(&key).unwrap();
        assert_eq!(image::load_from_memory(&encoded).unwrap().width(), 96);
    }
}
#[cfg(test)]
mod text_tests {
    use super::*;
    #[test]
    fn text_generic_and_unavailable_have_visible_distinct_rasters() {
        let empty = rgb(&KeyVisual::default()).unwrap();
        let text = rgb(&KeyVisual::new("Atrás Ω", None, false, false)).unwrap();
        assert_ne!(text, empty);
        assert!(
            text.pixels()
                .any(|p| p[0] > 150 && p[1] > 150 && p[2] > 150)
        );
        let generic = rgb(&KeyVisual::new("", None, false, true)).unwrap();
        assert_ne!(generic, empty);
        let unavailable = rgb(&KeyVisual::new("", None, true, true)).unwrap();
        assert_ne!(generic, unavailable);
        assert!(unavailable.pixels().any(|p| p[0] > 200 && p[1] < 100));
    }
}
#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::TextPlacement;
    use image::ImageEncoder;
    use std::sync::Arc;
    /// The key as a person sees it: the XL shows the transmitted image upside down.
    fn upright(key: &KeyVisual) -> image::RgbImage {
        image::imageops::rotate180(&rgb(key).unwrap())
    }
    /// First and last row holding a pixel that `matches`.
    fn rows(image: &image::RgbImage, matches: impl Fn(&image::Rgb<u8>) -> bool) -> (u32, u32) {
        let hit: Vec<u32> = (0..96)
            .filter(|y| (0..96).any(|x| matches(image.get_pixel(x, *y))))
            .collect();
        (*hit.first().expect("some row"), *hit.last().unwrap())
    }
    fn white(p: &image::Rgb<u8>) -> bool {
        p.0.iter().all(|c| *c > 200)
    }
    fn red(p: &image::Rgb<u8>) -> bool {
        p[0] > 150 && p[1] < 60 && p[2] < 60
    }
    /// The red picture, at full strength or dimmed behind text.
    fn picture(p: &image::Rgb<u8>) -> bool {
        p[0] > 80 && p[1] < 60 && p[2] < 60
    }
    fn red_square() -> Arc<rad_media::NormalizedImage> {
        let pixels: Vec<u8> = (0..64 * 64).flat_map(|_| [255, 0, 0, 255]).collect();
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, 64, 64, image::ExtendedColorType::Rgba8)
            .unwrap();
        Arc::new(rad_media::normalize(&bytes).unwrap())
    }
    #[test]
    fn text_alone_sits_in_the_third_of_the_key_it_is_placed_at() {
        let at = |placement| {
            rows(
                &upright(&KeyVisual::new("Mute", None, false, false).text_at(placement)),
                white,
            )
        };
        let (first, last) = at(TextPlacement::Top);
        assert!(last < 32, "top text spans {first}..={last}");
        let (first, last) = at(TextPlacement::Middle);
        assert!(
            first >= 32 && last < 64,
            "middle text spans {first}..={last}"
        );
        let (first, last) = at(TextPlacement::Bottom);
        assert!(first >= 64, "bottom text spans {first}..={last}");
    }
    #[test]
    fn picture_keeps_clear_of_text_placed_beside_it() {
        let with = |placement| {
            upright(&KeyVisual::new("Mute", Some(red_square()), false, false).text_at(placement))
        };
        let below = with(TextPlacement::BelowPicture);
        let (first, last) = rows(&below, red);
        assert!(first == 0 && last < 64, "picture spans {first}..={last}");
        assert!(rows(&below, white).0 >= 64);
        let above = with(TextPlacement::AbovePicture);
        let (first, last) = rows(&above, red);
        assert!(first >= 32 && last == 95, "picture spans {first}..={last}");
        assert!(rows(&above, white).1 < 32);
    }
    #[test]
    fn picture_fills_the_key_and_is_dimmed_only_behind_text_drawn_over_it() {
        let with = |placement| {
            upright(&KeyVisual::new("Mute", Some(red_square()), false, false).text_at(placement))
        };
        // (placement, a row behind the text, a row clear of it)
        for (placement, behind, clear) in [
            (TextPlacement::Top, 8, 90),
            (TextPlacement::Middle, 48, 2),
            (TextPlacement::Bottom, 88, 2),
        ] {
            let face = with(placement);
            assert_eq!(rows(&face, picture), (0, 95), "{placement:?}");
            assert!(red(face.get_pixel(2, clear)), "{placement:?}");
            assert!(!red(face.get_pixel(2, behind)), "{placement:?}");
            let (first, last) = rows(&face, white);
            assert!(
                first + 16 > behind && last < behind + 16,
                "{placement:?} text spans {first}..={last}"
            );
        }
    }
}

#[cfg(test)]
mod plugin_visual_tests {
    use super::*;
    #[test]
    fn color_only_is_visible_and_alpha_is_composited_without_executable_state() {
        let key = KeyVisual::default().background_color(Some([100, 80, 60, 255]));
        assert_eq!(rgb(&key).unwrap().get_pixel(40, 40).0, [100, 80, 60]);
        assert_eq!(
            rgb(&key.background_color(Some([100, 80, 60, 0])))
                .unwrap()
                .get_pixel(40, 40)
                .0,
            [0, 0, 0]
        );
    }
    #[test]
    fn stale_marks_last_content_distinctly_without_changing_unavailable() {
        let current = KeyVisual::new("Back", None, false, false);
        let stale = current.clone().stale(true);
        assert!(rgb(&current).unwrap() != rgb(&stale).unwrap());
        assert!(!stale.unavailable);
        assert_eq!(stale.text, current.text);
        assert_eq!(stale.clone().stale(false), current);
        let unavailable = KeyVisual::new("Back", None, true, false);
        assert!(rgb(&stale).unwrap() != rgb(&unavailable).unwrap());
        assert_eq!(
            rgb(&unavailable.clone().stale(true)).unwrap(),
            rgb(&unavailable).unwrap()
        );
    }
}
