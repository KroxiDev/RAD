//! Discover embedded catalogs at build time: adding a language requires only its JSON file.
use std::{env, fs, path::PathBuf};
fn main() {
    let directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("src/locales");
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut files: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    let mut generated = String::from("pub const EMBEDDED: &[(&str, &str)] = &[\n");
    for path in files {
        let locale = path.file_stem().unwrap().to_str().unwrap();
        assert!(
            locale
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        );
        generated.push_str(&format!(
            "({locale:?}, include_str!({:?})),\n",
            path.to_str().unwrap()
        ));
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("catalogs.rs"),
        generated,
    )
    .unwrap();
}
