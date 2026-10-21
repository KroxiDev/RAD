fn main() -> Result<(), eframe::Error> {
    use std::path::PathBuf;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let theme = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".local/state/omarchy/current/theme");
    let icon = rad_editor::app_icon::png_rgba(256);
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id(rad_editor::app_icon::APP_ID)
            .with_icon(eframe::egui::IconData {
                rgba: icon.into_raw(),
                width: 256,
                height: 256,
            })
            .with_inner_size([1180., 780.])
            .with_min_inner_size([980., 680.]),
        ..Default::default()
    };
    eframe::run_native(
        rad_editor::texts::text("en", "RAD"),
        options,
        Box::new(move |_| Ok(Box::new(rad_editor::app::App::new(runtime, theme)))),
    )
}
