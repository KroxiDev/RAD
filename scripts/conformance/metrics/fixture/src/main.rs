//! Separate integration fixture. Same plugin/SDK; never the system reader.
use rad_plugin_metrics::{MetricsPlugin, config, polling::Sample, readings::Memory};
use std::{fs, io::Read, path::PathBuf, sync::mpsc};

fn healthy() -> Sample {
    Sample {
        cpu: Some(25),
        ram: Some(Memory {
            used: 8 * 1073741824,
            available: 8 * 1073741824,
            total: 16 * 1073741824,
        }),
        gpu: Some(73),
        vram: Some(Memory {
            used: 2 * 1073741824,
            available: 6 * 1073741824,
            total: 8 * 1073741824,
        }),
    }
}

fn main() {
    if let Some(code) = rad_plugin_sdk::licenses_exit() {
        std::process::exit(if code == std::process::ExitCode::SUCCESS {
            0
        } else {
            1
        });
    }
    let root =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("private runtime directory"));
    assert!(root.is_absolute());
    let mut count = 0_u64;
    let (keep_blocked, blocked) = mpsc::channel::<()>();
    let plugin = MetricsPlugin::with_reader(move || {
        count += 1;
        fs::write(root.join("f5-reader-count.next"), count.to_string()).unwrap();
        fs::rename(
            root.join("f5-reader-count.next"),
            root.join("f5-reader-count"),
        )
        .unwrap();
        let mut mode = String::new();
        fs::File::open(root.join("f5-reader-mode"))
            .unwrap()
            .take(16)
            .read_to_string(&mut mode)
            .unwrap();
        match mode.as_str() {
            "healthy" => healthy(),
            "block" => {
                // The same single reader waits forever; no replacement/retry worker.
                let _keep_sender_alive = &keep_blocked;
                let _ = blocked.recv();
                Sample::default()
            }
            _ => Sample::default(),
        }
    })
    .unwrap();
    if rad_plugin_sdk::run_stdio(config(), plugin).is_err() {
        std::process::exit(1);
    }
}
