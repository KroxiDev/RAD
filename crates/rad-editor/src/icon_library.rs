//! Trusted, embedded SVGs and their bounded raster cache. No filesystem or runtime state.
use crate::preview::Pixels;
use image::ImageEncoder;
use std::collections::BTreeMap;
mod catalog;
pub use catalog::ENTRIES;

pub const GROUPS: &[&str] = &[
    "icons.group.audio",
    "icons.group.playback",
    "icons.group.navigation",
    "icons.group.files",
    "icons.group.system",
    "icons.group.capture",
    "icons.group.communication",
    "icons.group.tools",
];
pub struct Entry {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub tags: &'static str,
    pub svg: &'static str,
}
impl Entry {
    pub fn matches(&self, query: &str) -> bool {
        let haystack = searchable(&format!("{} {}", self.id, self.tags));
        searchable(query)
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }
}
fn searchable(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            '-' | '_' => ' ',
            other => other,
        })
        .collect()
}
/// Size of the symbol inside the key, in percent: 100 is the full fit, 50 the smallest.
pub const SCALES: std::ops::RangeInclusive<u8> = 50..=100;
pub const SCALE_STEP: u8 = 5;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub icon: usize,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    /// Percent within [`SCALES`].
    pub scale: u8,
}
impl Default for Choice {
    fn default() -> Self {
        Self {
            icon: 0,
            foreground: [235, 237, 240],
            background: [24, 26, 31],
            scale: *SCALES.end(),
        }
    }
}
pub struct Rendered {
    pub pixels: Pixels,
    pub png: Vec<u8>,
}
/// At most 200 small masks and one full-size mask, independent of color history.
#[derive(Default)]
pub struct Renderer {
    thumbnails: BTreeMap<usize, Vec<u8>>,
    selected: Option<((usize, u8), Vec<u8>)>,
}
impl Renderer {
    pub fn thumbnail(&mut self, icon: usize) -> Result<Pixels, String> {
        if let std::collections::btree_map::Entry::Vacant(slot) = self.thumbnails.entry(icon) {
            slot.insert(mask(icon, 64, *SCALES.end())?);
        }
        let rgba = self.thumbnails[&icon]
            .iter()
            .flat_map(|a| [255, 255, 255, *a])
            .collect();
        Ok(Pixels {
            size: [64, 64],
            rgba,
        })
    }
    pub fn render(&mut self, choice: Choice) -> Result<Rendered, String> {
        let key = (choice.icon, choice.scale);
        if self.selected.as_ref().is_none_or(|(held, _)| *held != key) {
            self.selected = Some((key, mask(choice.icon, 256, choice.scale)?));
        }
        let alpha = &self.selected.as_ref().expect("mask loaded").1;
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for a in alpha {
            let a = u32::from(*a);
            for (fg, bg) in choice.foreground.into_iter().zip(choice.background) {
                rgba.push(((u32::from(fg) * a + u32::from(bg) * (255 - a) + 127) / 255) as u8);
            }
            rgba.push(255);
        }
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, 256, 256, image::ExtendedColorType::Rgba8)
            .map_err(|e| format!("icon PNG: {e}"))?;
        Ok(Rendered {
            pixels: Pixels {
                size: [256, 256],
                rgba,
            },
            png,
        })
    }
}
fn mask(icon: usize, size: u32, scale: u8) -> Result<Vec<u8>, String> {
    let entry = ENTRIES.get(icon).ok_or("unknown bundled icon")?;
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(entry.svg, &options)
        .map_err(|e| format!("{}: {e}", entry.id))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon image allocation")?;
    // At full size the symbol leaves an 8% margin; a smaller one stays centered. The
    // SVG's own viewBox remains proportional.
    let percent = f32::from(scale.clamp(*SCALES.start(), *SCALES.end())) / 100.;
    let room = size as f32 * 0.84 * percent;
    let offset = (size as f32 - room) / 2.;
    let factor = room / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(factor, 0., 0., factor, offset, offset),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect())
}
