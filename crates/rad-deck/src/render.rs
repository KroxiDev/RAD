use crate::{DeckError, KeyVisual, TextPlacement};

const SIDE: u32 = 96;
/// Reference height for native pictures beside the default 13 px text.
const PICTURE: u32 = 62;

pub fn rgb(key: &KeyVisual) -> Result<image::RgbImage, DeckError> {
    let rgba = key.background.unwrap_or([0, 0, 0, 255]);
    let background = image::Rgb(std::array::from_fn(|c| {
        (u16::from(rgba[c]) * u16::from(rgba[3]) / 255) as u8
    }));
    let mut canvas = image::RgbImage::from_pixel(SIDE, SIDE, background);
    let text_band = (32.0 * f32::from(key.text_size) / 13.0).ceil() as u32;
    let picture_height = SIDE - text_band - 2;
    // Rows the picture may use: all of them unless the text is placed beside it.
    let (top, height) = match (key.text.is_empty(), key.text_placement) {
        (false, TextPlacement::BelowPicture) => (0, picture_height),
        (false, TextPlacement::AbovePicture) => (SIDE - picture_height, picture_height),
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
    let sun = key.image.is_none() && key.brightness_icon && !moon;
    if moon {
        let center_y = top as f32 + height as f32 / 2.;
        let scale = (height as f32 / PICTURE as f32).min(1.0);
        let radius = 22. * scale;
        for y in top..top + height {
            for x in 0..SIDE {
                let dx = x as f32 + 0.5 - 48.;
                let dy = y as f32 + 0.5 - center_y;
                if dx * dx + dy * dy <= radius * radius
                    && (dx - 11. * scale).powi(2) + (dy + 8. * scale).powi(2) > radius * radius
                {
                    canvas.put_pixel(x, y, image::Rgb([225, 230, 245]));
                }
            }
        }
    }
    if sun {
        let center_y = top as f32 + height as f32 / 2.;
        let scale = (height as f32 / PICTURE as f32).min(1.0);
        let rays: [(f32, f32); 8] = std::array::from_fn(|ray| {
            let angle = std::f32::consts::TAU * ray as f32 / 8.;
            (angle.cos(), angle.sin())
        });
        for y in top..top + height {
            for x in 0..SIDE {
                let dx = x as f32 + 0.5 - 48.;
                let dy = y as f32 + 0.5 - center_y;
                let distance = dx * dx + dy * dy;
                let ring = ((7.5 * scale).powi(2)..=(11. * scale).powi(2)).contains(&distance);
                let ray = rays.iter().any(|(vx, vy)| {
                    (15.4 * scale..=22. * scale).contains(&(dx * vx + dy * vy))
                        && (dx * vy - dy * vx).abs() <= 1.7 * scale
                });
                if ring || ray {
                    canvas.put_pixel(x, y, image::Rgb([225, 230, 245]));
                }
            }
        }
    }
    if generic && !moon && !sun {
        let icon_height = height.min(PICTURE);
        let offset = top + (height - icon_height) / 2;
        for y in 12..54 {
            for x in 22..74 {
                let c = if !(25..=70).contains(&x) || !(21..=50).contains(&y) {
                    180
                } else {
                    45
                };
                canvas.put_pixel(
                    48 - 26 * icon_height / PICTURE + (x - 22) * icon_height / PICTURE,
                    offset + y * icon_height / PICTURE,
                    image::Rgb([c, c, c]),
                );
            }
        }
    }
    if !key.text.is_empty() {
        text(
            &mut canvas,
            key,
            key.image.is_some() || generic || moon || sun,
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
fn text(canvas: &mut image::RgbImage, key: &KeyVisual, over_picture: bool) {
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
        key.text.clone(),
        FontId::proportional(f32::from(key.text_size)),
        Color32::WHITE,
        92.0,
    );
    job.wrap.max_rows = 2;
    let galley = fonts.with_pixels_per_point(1.0).layout_job(job);
    let text_band = (32.0 * f32::from(key.text_size) / 13.0).ceil();
    let block = galley.size().y.min(text_band);
    let side = SIDE as f32;
    // Where the block starts, and the rows dimmed behind it when it lies over a picture.
    let (origin, band) = match key.text_placement {
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
        TextPlacement::BelowPicture => (SIDE as f32 - text_band, None),
    };
    if let (Some(band), true) = (band, over_picture && key.text_band) {
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
                            ((u16::from(old[c]) * (255 - u16::from(a))
                                + u16::from(key.text_color[c]) * u16::from(a))
                                / 255) as u8
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
