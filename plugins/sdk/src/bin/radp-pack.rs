use rad_plugin_sdk::pack;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [manifest, executable, output] = args.as_slice() else {
        eprintln!("usage: radp-pack <manifest.json> <executable> <output.radp>");
        return ExitCode::from(2);
    };
    match build(manifest.as_ref(), executable.as_ref(), output.as_ref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("radp-pack: {message}");
            ExitCode::FAILURE
        }
    }
}

fn build(manifest: &Path, executable: &Path, output: &Path) -> Result<(), String> {
    let size = |path: &Path| {
        fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    pack::check_sizes(size(manifest)?, size(executable)?).map_err(|e| e.to_string())?;
    let read =
        |path: &Path| fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()));
    let package = pack::encode(&read(manifest)?, &read(executable)?).map_err(|e| e.to_string())?;
    write_whole(output, &package).map_err(|e| format!("cannot write {}: {e}", output.display()))
}

fn write_whole(output: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let scratch: PathBuf = output.with_file_name(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&scratch)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&scratch, output));
    if result.is_err() {
        let _ = fs::remove_file(&scratch);
    }
    result
}
