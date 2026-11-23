use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().and_then(Path::parent).unwrap().to_path_buf()
}

pub(crate) fn fake() -> PathBuf {
    let fake = profile_dir().join("examples").join("fake_pactl");
    assert!(
        fake.exists(),
        "run `cargo test`, which builds the fake_pactl example"
    );
    fake
}

pub(crate) struct Dir(pub(crate) PathBuf);

impl Dir {
    pub(crate) fn new(name: &str) -> Self {
        let dir = profile_dir().join(format!("unit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(crate) fn with_devices(name: &str) -> Self {
        let dir = Self::new(name);
        fs::write(
            dir.0.join("state.json"),
            r#"{"sinks":[{"name":"sink-a","description":"Speakers","mute":false}],
                "sources":[{"name":"mic-a","description":"Microphone","mute":false}],
                "default_sink":"sink-a","default_source":"mic-a"}"#,
        )
        .unwrap();
        dir
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn pids(dir: &Dir, tag: &str) -> Vec<u32> {
    fs::read_to_string(dir.0.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(tag).and_then(|p| p.trim().parse().ok()))
        .collect()
}

pub(crate) fn alive(dir: &Dir, tag: &str) -> usize {
    pids(dir, tag)
        .into_iter()
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| {
                    !stat
                        .rsplit(')')
                        .next()
                        .unwrap_or("")
                        .trim_start()
                        .starts_with('Z')
                })
                .unwrap_or(false)
        })
        .count()
}

pub(crate) fn gone(pid: u32) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists()
}

pub(crate) fn all_gone(dir: &Dir, tag: &str) -> bool {
    let pids = pids(dir, tag);
    !pids.is_empty() && pids.into_iter().all(gone)
}

pub(crate) fn emit_event(dir: &Dir, line: &str) {
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.0.join("events.log"))
        .unwrap();
    writeln!(log, "{line}").unwrap();
}
