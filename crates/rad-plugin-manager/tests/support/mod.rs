use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rad-manager-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
pub const TARGET: &str = "x86_64-unknown-linux-gnu";

pub fn package(major: u32) -> Vec<u8> {
    package_for("org.rad.fixture", major)
}

pub fn package_for(id: &str, major: u32) -> Vec<u8> {
    let manifest = serde_json::json!({"contract":1,"capabilities":[],"id":id,"version":{"major":major,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[TARGET],"actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":[]},"credential_slots":[]}],"visuals":[]});
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let executable = fs::read(std::env::current_exe().unwrap()).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        &manifest,
        &executable,
        rad_plugin_contract::HOST_API,
        TARGET,
    )
    .unwrap();
    rad_plugin_package::encode(&candidate)
}
