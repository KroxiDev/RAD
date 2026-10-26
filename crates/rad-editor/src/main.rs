fn main() -> Result<(), eframe::Error> {
    use std::{path::PathBuf, sync::Arc};
    let _update_guard = match rad_update::gate::startup_guard() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("RAD startup update gate: {error}");
            return Ok(());
        }
    };
    let open_updates = std::env::args()
        .skip(1)
        .any(|argument| argument == "--updates");
    let activation = Arc::new(rad_editor::activation::WindowActivation::default());
    let (notify, quit) = (activation.clone(), activation.clone());
    let _instance = match rad_editor::activation::Instance::claim(
        rad_editor::app_icon::APP_ID,
        move || notify.activate(),
        move || quit.close(),
    ) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => return Ok(()),
        Err(error) => {
            // A desktop without a session bus still gets a usable editor.
            eprintln!("RAD editor activation unavailable: {error}");
            None
        }
    };
    // The menu entry follows the editor in use (E2); failing to write it never blocks the editor.
    std::thread::spawn(|| {
        if let Err(error) = rad_editor::desktop::install_for_user() {
            eprintln!("RAD menu entry not installed: {error}");
        }
    });
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let theme = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".local/state/omarchy/current/theme");
    let icon = rad_editor::app_icon::png_rgba(256);
    let mut options = eframe::NativeOptions {
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
    // Hidden Wayland windows get no frame callbacks: a vsynced swap blocks the main
    // thread, so the compositor marks the application as unresponsive.
    options.glow_options.vsync = false;
    eframe::run_native(
        rad_editor::texts::text("en", "RAD"),
        options,
        Box::new(move |creation| {
            activation.bind(&creation.egui_ctx);
            let mut app = rad_editor::app::App::new(runtime, theme);
            if open_updates {
                app.open_update_result();
            }
            Ok(Box::new(app))
        }),
    )
}
