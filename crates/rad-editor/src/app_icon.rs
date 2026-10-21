//! Desktop identity and the single replaceable artwork source (E1).
pub const APP_ID: &str = "io.github.KroxiDev.RAD";
pub const PNG: &[u8] = include_bytes!("../../../packaging/rad.png");

/// Rasterize the embedded artwork for a window or desktop surface.
pub fn png_rgba(size: u32) -> image::RgbaImage {
    image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the packaged RAD icon is a valid PNG")
        .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
        .into_rgba8()
}
