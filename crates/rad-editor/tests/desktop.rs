//! The editor puts RAD in the application menu: its entry and icon under the user's data
//! directory, rewritten only when they change and pointing at the editor in use (E2).
use rad_editor::{
    app_icon::APP_ID,
    desktop::{data_directory, install},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const TEMPLATE: &str = include_str!("../../../packaging/io.github.KroxiDev.RAD.desktop");

/// A fresh data directory for one test, under the build's own temporary directory.
fn tree(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("desktop")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn entry(data: &Path) -> String {
    std::fs::read_to_string(data.join(format!("applications/{APP_ID}.desktop"))).unwrap()
}
fn icon(data: &Path) -> PathBuf {
    data.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"))
}

#[test]
fn the_entry_runs_the_editor_in_use_and_keeps_the_rest_of_the_template() {
    let data = tree("fresh");
    assert_eq!(
        install(&data, Path::new("/opt/RAD build/rad-editor")),
        Ok(true)
    );

    let installed = entry(&data);
    let (mine, template): (Vec<_>, Vec<_>) =
        (installed.lines().collect(), TEMPLATE.lines().collect());
    assert_eq!(mine.len(), template.len());
    let changed: Vec<_> = mine.iter().zip(&template).filter(|(a, b)| a != b).collect();
    assert_eq!(
        changed,
        [(&r#"Exec="/opt/RAD build/rad-editor""#, &"Exec=rad-editor")]
    );
    assert!(installed.contains(&format!("Icon={APP_ID}\n")));

    let picture = image::open(icon(&data)).unwrap();
    assert_eq!((picture.width(), picture.height()), (256, 256));
}

#[test]
fn reserved_characters_in_the_editor_path_are_escaped_and_odd_paths_refused() {
    let data = tree("escaped");
    install(&data, Path::new(r#"/tmp/a"b$c\d%e/rad-editor"#)).unwrap();
    assert!(entry(&data).contains("Exec=\"/tmp/a\\\\\"b\\\\$c\\\\\\\\d%%e/rad-editor\"\n"));

    assert!(install(&data, Path::new("rad-editor")).is_err());
    assert!(install(&data, Path::new("/tmp/new\nline/rad-editor")).is_err());
}

#[test]
fn nothing_is_rewritten_until_the_entry_the_icon_or_the_editor_changes() {
    let data = tree("again");
    let editor = Path::new("/opt/rad/rad-editor");
    assert_eq!(install(&data, editor), Ok(true));
    let (first_entry, first_icon) = (entry(&data), std::fs::read(icon(&data)).unwrap());
    assert_eq!(install(&data, editor), Ok(false));

    let stale = data.join(format!("applications/{APP_ID}.desktop"));
    std::fs::write(&stale, "[Desktop Entry]\nExec=old\n").unwrap();
    std::fs::remove_file(icon(&data)).unwrap();
    assert_eq!(install(&data, editor), Ok(true));
    assert_eq!(entry(&data), first_entry);
    assert_eq!(std::fs::read(icon(&data)).unwrap(), first_icon);

    assert_eq!(install(&data, Path::new("/srv/rad/rad-editor")), Ok(true));
    assert!(entry(&data).contains("Exec=/srv/rad/rad-editor\n"));
    let leftovers: Vec<_> = std::fs::read_dir(data.join("applications"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "no temporary file stays behind");
}

#[test]
fn the_data_directory_follows_xdg_and_ignores_relative_paths() {
    let os = |s: &str| Some(OsString::from(s));
    assert_eq!(
        data_directory(os("/x/data"), os("/home/u")),
        Some(PathBuf::from("/x/data"))
    );
    assert_eq!(
        data_directory(os("relative"), os("/home/u")),
        Some(PathBuf::from("/home/u/.local/share"))
    );
    assert_eq!(data_directory(None, os("rel")), None);
}
