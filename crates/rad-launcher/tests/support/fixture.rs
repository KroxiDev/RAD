use gio::prelude::*;
fn main() {
    let path = std::env::var_os("RAD_FIXTURE_OUTPUT").expect("private fixture output");
    if std::env::args().nth(1).as_deref() == Some("--dbus") {
        let app = gio::Application::new(Some("org.rad.Bus"), gio::ApplicationFlags::FLAGS_NONE);
        app.connect_activate(move |app| {
            std::fs::write(&path, b"activated").unwrap();
            let gate = std::env::var_os("RAD_FIXTURE_GATE").unwrap();
            let ready = std::env::var_os("RAD_FIXTURE_READY").unwrap();
            let app = app.clone();
            let hold = app.hold();
            glib::timeout_add_local(std::time::Duration::from_millis(2), move || {
                let _hold = &hold;
                if std::path::Path::new(&gate).exists() {
                    std::fs::write(&ready, b"ready").unwrap();
                    app.quit();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        });
        app.run_with_args(&["fixture"]);
        return;
    }
    // Optional: the process id seen by the OS, and a stderr marker that must
    // never reach the helper's diagnostics.
    if let Some(pid_path) = std::env::var_os("RAD_FIXTURE_PID") {
        std::fs::write(pid_path, std::process::id().to_string()).unwrap();
    }
    if std::env::var_os("RAD_FIXTURE_STDERR").is_some() {
        eprintln!("fixture-app-stderr-marker");
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::fs::write(&path, serde_json::to_vec(&args).unwrap()).unwrap();
}
