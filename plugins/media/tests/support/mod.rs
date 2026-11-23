#![allow(dead_code)]
mod bus;
mod host;
mod player;

#[allow(unused_imports)]
pub use bus::{MuteBus, PrivateBus};
#[allow(unused_imports)]
pub use host::{Host, Visual, quick};
#[allow(unused_imports)]
pub use player::FakePlayer;

use std::path::{Path, PathBuf};

pub fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test binary path");
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile>/deps");
    let path = profile_dir.join("examples").join(name);
    assert!(
        path.exists(),
        "{} is missing: run `cargo test` or `cargo build --examples`",
        path.display()
    );
    path
}

pub struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch_dir(label: &str, short: bool) -> Scratch {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let base = if short {
        std::env::temp_dir()
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
    };
    let dir = base.join(format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}
