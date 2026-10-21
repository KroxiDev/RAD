//! The real seam, against a stand-in program: exact arguments, no shell, bounded time.
use rad_editor::service::{Systemctl, Unavailable, UserSystemctl};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

// One test, so no other thread forks while a script is still open for writing.
#[test]
fn the_real_seam_passes_user_and_exact_arguments_without_a_shell_and_is_bounded() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("service-process");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("args.log");
    let record = script(
        &dir,
        "record",
        &format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\" >> {}; done\necho out-text\necho err-text >&2\nexit 3",
            log.display()
        ),
    );
    let answer = UserSystemctl::new(&record, Duration::from_secs(5))
        .run(&["show", "two words", "$(touch pwned)", ";ls"])
        .unwrap();
    assert_eq!(answer.code, Some(3));
    assert_eq!(answer.stdout.trim(), "out-text");
    assert_eq!(answer.stderr.trim(), "err-text");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "--user\nshow\ntwo words\n$(touch pwned)\n;ls\n"
    );
    assert!(!dir.join("pwned").exists());

    let flood = script(&dir, "flood", "head -c 5000000 /dev/zero | tr '\\0' x");
    let answer = UserSystemctl::new(&flood, Duration::from_secs(5))
        .run(&["show"])
        .unwrap();
    assert_eq!(answer.code, Some(0));
    assert!(answer.stdout.len() <= 64 * 1024, "{}", answer.stdout.len());

    let slow = script(&dir, "slow", "exec sleep 30");
    let started = std::time::Instant::now();
    let Err(Unavailable(why)) = UserSystemctl::new(&slow, Duration::from_millis(300)).run(&["x"])
    else {
        panic!("a program past its deadline is an error");
    };
    assert!(why.contains("timed out"), "{why}");
    assert!(started.elapsed() < Duration::from_secs(10));

    let missing = UserSystemctl::new(dir.join("absent"), Duration::from_secs(1)).run(&["x"]);
    assert!(matches!(missing, Err(Unavailable(_))));
}
