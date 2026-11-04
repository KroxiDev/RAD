//! Application palette thumbnails, loaded independently of the RPC worker.
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

const TTL: Duration = Duration::from_secs(30);
struct Entry<T> {
    value: T,
    shown: Duration,
}
#[derive(Default)]
struct Cache<T> {
    entries: HashMap<String, Entry<T>>,
}
impl<T> Cache<T> {
    fn expire(&mut self, now: Duration) {
        self.entries
            .retain(|_, entry| now.saturating_sub(entry.shown) < TTL);
    }
    fn show(&mut self, key: &str, now: Duration) -> Option<&T> {
        self.expire(now);
        self.entries.get_mut(key).map(|entry| {
            entry.shown = now;
            &entry.value
        })
    }
    fn insert(&mut self, key: String, value: T, now: Duration) {
        self.entries.insert(key, Entry { value, shown: now });
    }
}

fn load(icons: &crate::icons::Icons, desktop: &str) -> Option<crate::preview::Pixels> {
    use std::io::Read;
    const MAX_BYTES: u64 = 1024 * 1024;
    let file = std::fs::File::open(icons.find(desktop)?).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(36, 36).to_rgba8();
    Some(crate::preview::Pixels {
        size: [image.width() as usize, image.height() as usize],
        rgba: image.into_raw(),
    })
}

use crate::worker::Worker;
use eframe::egui;
type Loaded = (String, Option<crate::preview::Pixels>);
#[derive(Default)]
pub(super) struct ApplicationIcons {
    cache: Cache<(Option<egui::TextureHandle>, Duration)>,
    worker: Option<Worker<String, Loaded>>,
    visible: HashSet<String>,
}
impl ApplicationIcons {
    pub(super) fn begin(&mut self, ctx: &egui::Context, now: Duration) {
        // Rows remained on screen between frames, even when the UI was idle.
        for key in self.visible.drain() {
            if let Some(entry) = self.cache.entries.get_mut(&key) {
                entry.shown = now;
            }
        }
        self.cache.expire(now);
        if let Some(worker) = &mut self.worker {
            match worker.poll() {
                crate::worker::Polled::Ready((key, pixels)) => {
                    let texture = pixels.map(|pixels| {
                        ctx.load_texture(
                            format!("application:{key}"),
                            egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    self.cache.insert(key, (texture, now), now);
                }
                crate::worker::Polled::Failed { .. } | crate::worker::Polled::Closed { .. } => {
                    self.worker = None;
                }
                crate::worker::Polled::Pending => {}
            }
        }
        if let Some(deadline) = self
            .cache
            .entries
            .values()
            .map(|entry| entry.shown + TTL)
            .min()
        {
            ctx.request_repaint_after(deadline.saturating_sub(now));
        }
    }
    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        now: Duration,
    ) -> Option<(Option<egui::TextureHandle>, f32)> {
        self.visible.insert(key.to_owned());
        if let Some((texture, arrived)) = self.cache.show(key, now) {
            let alpha = (now.saturating_sub(*arrived).as_secs_f32() / 0.2).min(1.0);
            if alpha < 1.0 {
                ctx.request_repaint();
            }
            return Some((texture.clone(), alpha));
        }
        let worker = self.worker.get_or_insert_with(|| {
            let icons = crate::icons::Icons::from_environment();
            let worker = Worker::spawn(move |key: String| {
                let pixels = load(&icons, &key);
                (key, pixels)
            });
            let ctx = ctx.clone();
            worker.set_waker(move || ctx.request_repaint());
            worker
        });
        let _ = worker.submit(key.to_owned());
        None
    }
    #[cfg(test)]
    pub(super) fn requested(&self) -> bool {
        self.worker.as_ref().is_some_and(Worker::busy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn results_expire_after_thirty_seconds_without_display_including_missing_icons() {
        for result in [Some(1), None] {
            let mut cache = Cache::default();
            cache.insert("x".into(), result, Duration::ZERO);
            cache.expire(Duration::from_secs(29));
            assert_eq!(cache.entries.len(), 1);
            assert_eq!(cache.show("x", Duration::from_secs(29)), Some(&result));
            cache.expire(Duration::from_secs(58));
            assert_eq!(cache.entries.len(), 1);
            cache.expire(Duration::from_secs(59));
            assert!(cache.show("x", Duration::from_secs(59)).is_none());
        }
    }
    #[test]
    fn expired_result_is_requested_again_and_pending_badge_is_empty() {
        let ctx = egui::Context::default();
        let mut icons = ApplicationIcons::default();
        icons
            .cache
            .insert("x.desktop".into(), (None, Duration::ZERO), Duration::ZERO);
        assert!(icons.show(&ctx, "x.desktop", Duration::ZERO).is_some());
        assert!(!icons.requested());
        assert!(icons.show(&ctx, "x.desktop", TTL).is_none());
        assert!(icons.requested());
    }

    #[test]
    fn loader_uses_xdg_rasters_and_bounds_files_and_dimensions() {
        const CHILD: &str = "RAD_TEST_PALETTE_ICONS";
        if std::env::var_os(CHILD).is_some() {
            let icons = crate::icons::Icons::from_environment();
            let pixels = load(&icons, "x.desktop").expect("load installed PNG");
            assert_eq!(pixels.size, [36, 18]);
            assert_eq!(
                load(&icons, "jpeg.desktop").expect("load JPEG").size,
                [36, 18]
            );
            assert!(load(&icons, "svg.desktop").is_none());
            assert!(load(&icons, "large.desktop").is_none());
            assert!(load(&icons, "wide.desktop").is_none());
            return;
        }
        let dir = std::env::temp_dir().join(format!("rad-palette-icons-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("applications")).unwrap();
        let raster = dir.join("icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&raster).unwrap();
        for name in ["x", "jpeg", "svg", "large", "wide"] {
            std::fs::write(
                dir.join(format!("applications/{name}.desktop")),
                format!("[Desktop Entry]\nName={name}\nIcon={name}\n"),
            )
            .unwrap();
        }
        image::RgbaImage::from_pixel(48, 24, image::Rgba([20, 40, 60, 255]))
            .save(raster.join("x.png"))
            .unwrap();
        image::RgbImage::new(48, 24)
            .save(raster.join("jpeg.jpg"))
            .unwrap();
        image::RgbaImage::new(1025, 1)
            .save(raster.join("wide.png"))
            .unwrap();
        std::fs::write(raster.join("svg.svg"), "<svg/>").unwrap();
        std::fs::write(raster.join("large.png"), vec![0; 1024 * 1024 + 1]).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "app::application_icons::tests::loader_uses_xdg_rasters_and_bounds_files_and_dimensions", "--nocapture"])
            .env(CHILD, "1").env("XDG_DATA_DIRS", &dir).env("XDG_DATA_HOME", dir.join("own"))
            .status().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(status.success());
    }
}
