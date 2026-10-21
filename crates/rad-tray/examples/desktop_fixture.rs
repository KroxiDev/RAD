//! Headless editor fixture, exercising the real single-instance activation gate.
use std::io::Write;
fn log(line: &str) {
    let path = std::env::var_os("RAD_FIXTURE_LOG").expect("fixture log");
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{line}").unwrap();
}
fn main() {
    let instance = rad_editor::activation::Instance::claim(
        rad_editor::app_icon::APP_ID,
        || log("editor-activate"),
        || log("editor-quit"),
    )
    .unwrap();
    let Some(instance) = instance else {
        return;
    };
    log("editor-start");
    log(&format!("editor-pid {}", std::process::id()));
    while instance.is_alive() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
