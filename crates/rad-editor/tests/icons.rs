use rad_editor::{
    icons::{Icons, wanted},
    session::Inspection,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A fresh data directory tree for one test, under the build's own temporary directory.
fn tree(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("icons")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}
fn entry(root: &Path, id: &str, icon: &str) {
    write(
        &root.join("applications").join(id),
        &format!("[Desktop Entry]\nName=App\nIcon={icon}\nExec=app\n"),
    );
}

#[test]
fn themed_icon_prefers_the_size_nearest_256_and_the_first_data_directory() {
    let (home, system) = (tree("near-home"), tree("near-system"));
    entry(&system, "steam.desktop", "steam");
    for size in ["48x48", "512x512", "256x256", "128x128@2"] {
        write(
            &system.join(format!("icons/hicolor/{size}/apps/steam.png")),
            "png",
        );
    }
    write(&system.join("pixmaps/steam.png"), "png");
    let icons = Icons::new(vec![home.clone(), system.clone()]);
    assert_eq!(
        icons.find("steam.desktop"),
        Some(system.join("icons/hicolor/256x256/apps/steam.png"))
    );
    // The user's own directory wins at the same distance from 256.
    write(&home.join("icons/hicolor/256x256/apps/steam.jpg"), "jpg");
    assert_eq!(
        icons.find("steam.desktop"),
        Some(home.join("icons/hicolor/256x256/apps/steam.jpg"))
    );
}

#[test]
fn pixmaps_and_absolute_paths_serve_when_no_themed_icon_fits() {
    let root = tree("fallbacks");
    entry(&root, "code.desktop", "vscode");
    write(&root.join("pixmaps/vscode.png"), "png");
    let absolute = root.join("opt/game/icon.jpeg");
    write(&absolute, "jpeg");
    entry(&root, "game.desktop", absolute.to_str().unwrap());
    let icons = Icons::new(vec![root.clone()]);
    assert_eq!(
        icons.find("code.desktop"),
        Some(root.join("pixmaps/vscode.png"))
    );
    assert_eq!(icons.find("game.desktop"), Some(absolute));
}

#[test]
fn applications_without_a_png_or_jpeg_icon_have_none() {
    let root = tree("none");
    entry(&root, "vector.desktop", "vector");
    write(&root.join("icons/hicolor/scalable/apps/vector.svg"), "svg");
    write(&root.join("pixmaps/vector.svg"), "svg");
    entry(
        &root,
        "absolute.desktop",
        root.join("art/logo.svg").to_str().unwrap(),
    );
    write(&root.join("art/logo.svg"), "svg");
    entry(&root, "gone.desktop", "/nonexistent/icon.png");
    entry(&root, "escape.desktop", "../../secret");
    write(
        &root.join("applications/action.desktop"),
        "[Desktop Entry]\nName=App\n[Desktop Action New]\nIcon=steam\n",
    );
    write(&root.join("pixmaps/steam.png"), "png");
    let icons = Icons::new(vec![root.clone()]);
    for id in [
        "vector.desktop",
        "absolute.desktop",
        "gone.desktop",
        "escape.desktop",
        "action.desktop",
        "missing.desktop",
        "../applications/vector.desktop",
        "steam",
    ] {
        assert_eq!(icons.find(id), None, "{id}");
    }
}

fn inspection(image: Value, actions: Vec<Value>) -> Inspection {
    Inspection {
        button: json!({"id":"b1"}),
        appearance: json!({"text":"","image":image,"text_position":"middle"}),
        actions,
    }
}
fn launcher(desktop_entry: &str) -> Value {
    json!({"id":"a1","fields":{"type":"launcher","parameters":{"desktop_entry":desktop_entry}}})
}
#[test]
fn key_takes_the_icon_of_its_top_application_only_while_it_has_no_image() {
    let wait = json!({"id":"a2","fields":{"type":"wait","parameters":{"milliseconds":500}}});
    let top = inspection(
        Value::Null,
        vec![launcher("steam.desktop"), launcher("b.desktop")],
    );
    assert_eq!(wanted(&top), Some("steam.desktop"));
    let has_image = inspection(json!("owned-media"), vec![launcher("steam.desktop")]);
    assert_eq!(wanted(&has_image), None);
    let below = inspection(Value::Null, vec![wait, launcher("steam.desktop")]);
    assert_eq!(wanted(&below), None);
    assert_eq!(wanted(&inspection(Value::Null, vec![])), None);
}
