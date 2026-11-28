use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dev {
    pub name: String,
    pub description: String,
    pub mute: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct State {
    pub sinks: Vec<Dev>,
    pub sources: Vec<Dev>,
    pub default_sink: String,
    pub default_source: String,
}

pub fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

pub fn update<R>(dir: &Path, change: impl FnOnce(&mut State) -> R) -> R {
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(file(dir, "state.lock"))
        .expect("lock file");
    lock.lock().expect("lock audio state");
    let mut state = read(dir);
    let result = change(&mut state);
    let temp = file(dir, "state.tmp");
    fs::write(&temp, serde_json::to_vec(&state).unwrap()).expect("write state");
    fs::rename(&temp, file(dir, "state.json")).expect("publish state");
    drop(lock);
    result
}

pub fn read(dir: &Path) -> State {
    fs::read(file(dir, "state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn append(dir: &Path, name: &str, line: &str) {
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file(dir, name))
        .expect("log file");
    log.write_all(format!("{line}\n").as_bytes())
        .expect("append");
}
