use rad_runtime::lifecycle::{LifecycleError, Options};
#[test]
fn physical_hardware_is_explicit_opt_in_and_unknown_arguments_are_rejected() {
    assert_eq!(
        Options::parse([]).unwrap(),
        Options {
            xl_exclusive: false,
            help: false
        }
    );
    assert!(
        Options::parse(["--xl-exclusive".into()])
            .unwrap()
            .xl_exclusive
    );
    assert!(Options::parse(["--help".into()]).unwrap().help);
    assert_eq!(
        Options::parse(["--device".into()]),
        Err(LifecycleError::Arguments)
    );
    assert_eq!(
        Options::parse(["--xl-exclusive".into(), "--xl-exclusive".into()]),
        Err(LifecycleError::Arguments)
    );
}

#[test]
fn helper_child_does_not_inherit_blocked_shutdown_signals() {
    const CHILD: &str = "RAD_LIFECYCLE_SIGNAL_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        assert_eq!(
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask) },
            0
        );
        assert_eq!(unsafe { libc::sigismember(&mask, libc::SIGINT) }, 0);
        assert_eq!(unsafe { libc::sigismember(&mask, libc::SIGTERM) }, 0);
        return;
    }
    // Block only this test thread; child inherits its spawning thread's mask.
    let mut blocked = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    let mut original = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGINT);
        libc::sigaddset(&mut blocked, libc::SIGTERM);
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut original),
            0
        );
    }
    let result = rad_runtime::lifecycle::helper_command(&std::env::current_exe().unwrap())
        .args([
            "--exact",
            "helper_child_does_not_inherit_blocked_shutdown_signals",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .status();
    assert_eq!(
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut()) },
        0
    );
    assert!(result.unwrap().success());
}

#[test]
fn paths_use_explicit_xdg_and_home_fallback_without_relative_paths() {
    use rad_runtime::lifecycle::Paths;
    use std::{ffi::OsStr, path::Path};
    let p = Paths::from_values(
        Some(OsStr::new("/private/runtime")),
        Some(OsStr::new("/private/data")),
        None,
        Path::new("/app/rad-runtime"),
    )
    .unwrap();
    assert_eq!(p.runtime, Path::new("/private/runtime"));
    assert_eq!(p.data, Path::new("/private/data/rad"));
    assert_eq!(p.helper, Path::new("/app/rad-launcher-helper"));
    let p = Paths::from_values(
        Some(OsStr::new("/private/runtime")),
        None,
        Some(OsStr::new("/private/home")),
        Path::new("/app/rad-runtime"),
    )
    .unwrap();
    assert_eq!(p.data, Path::new("/private/home/.local/share/rad"));
    for bad in ["relative", "", "/private/../other"] {
        assert!(
            Paths::from_values(
                Some(OsStr::new(bad)),
                None,
                Some(OsStr::new("/home")),
                Path::new("/app/runtime")
            )
            .is_err()
        );
    }
    assert!(Paths::from_values(None, None, None, Path::new("/app/runtime")).is_err());
}

#[test]
fn private_directories_are_created_without_following_symlinks_or_chmod_existing() {
    use rad_runtime::lifecycle::Paths;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
    };
    let base = std::env::temp_dir().join(format!(
        "rad-lifecycle-paths-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&base).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = base.join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let data = base.join("data");
    let p = Paths::from_values(
        Some(runtime.as_os_str()),
        Some(data.as_os_str()),
        None,
        Path::new("/app/runtime"),
    )
    .unwrap();
    p.prepare().unwrap();
    assert_eq!(
        fs::metadata(&p.data).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::remove_dir(&p.data).unwrap();
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &p.data).unwrap();
    assert_eq!(p.prepare(), Err(LifecycleError::Path));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_file(&p.data).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(p.prepare(), Err(LifecycleError::Path));
    assert_eq!(
        fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o755
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn shutdown_signals_are_nonblocking_cloexec_and_wake_mio() {
    use rad_runtime::lifecycle::ShutdownSignals;
    use std::{os::fd::AsRawFd, time::Duration};
    let signals = ShutdownSignals::install().unwrap();
    assert!(!signals.take_shutdown().unwrap());
    let fd = signals.as_raw_fd();
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
        0
    );
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let mut poll = mio::Poll::new().unwrap();
    poll.registry()
        .register(
            &mut mio::unix::SourceFd(&fd),
            mio::Token(7),
            mio::Interest::READABLE,
        )
        .unwrap();
    // Target only this fixture test thread, never another process or the session.
    assert_eq!(
        unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGTERM) },
        0
    );
    let mut events = mio::Events::with_capacity(2);
    poll.poll(&mut events, Some(Duration::from_secs(1)))
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.token() == mio::Token(7) && event.is_readable())
    );
    assert!(signals.take_shutdown().unwrap());
    assert!(!signals.take_shutdown().unwrap());
    assert_eq!(
        unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGINT) },
        0
    );
    assert!(signals.take_shutdown().unwrap());
}

// Runtime binary smoke tests are enabled only in the complete isolated snapshot.
#[test]
fn binary_help_and_invalid_environment_never_start_runtime() {
    let Some(binary) = option_env!("CARGO_BIN_EXE_rad-runtime") else {
        return;
    };
    let help = std::process::Command::new(binary)
        .env_clear()
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--xl-exclusive"));
    let invalid = std::process::Command::new(binary)
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(75), "blocking, not unexpected");
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("XDG_RUNTIME_DIR"));
}

#[cfg(feature = "fixture-tests")]
#[test]
fn binary_private_session_shutdown_and_corrupt_startup_preserve_material() {
    use rad_client::connection::{Client, Endpoint};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let binary = env!("CARGO_BIN_EXE_rad-runtime");
    let helper = std::path::Path::new(binary).with_file_name("rad-launcher-helper");
    assert!(
        helper.is_file(),
        "build companion helper in the isolated harness first"
    );
    let base = std::env::temp_dir().join(format!(
        "rad-lifecycle-session-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(base.clone());
    for sub in ["runtime", "home", "data", "empty-system", "config", "cache"] {
        fs::create_dir_all(base.join(sub)).unwrap();
        fs::set_permissions(base.join(sub), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let command = || {
        let mut c = Command::new(binary);
        c.env_clear()
            .env("XDG_RUNTIME_DIR", base.join("runtime"))
            .env("XDG_DATA_HOME", base.join("data"))
            .env("HOME", base.join("home"))
            .env("XDG_DATA_DIRS", base.join("empty-system"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-rad-lifecycle-private-bus",
            )
            .env("PATH", "/nonexistent-rad-lifecycle-bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        c
    };
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(command().spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = loop {
        if let Ok(client) = Client::connect(&base.join("runtime"), Endpoint::Editor) {
            break client;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            use std::io::Read;
            let mut stderr = String::new();
            child
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!("runtime exited before ready: {status}: {stderr}");
        }
        assert!(Instant::now() < deadline, "runtime readiness timeout");
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = client.request("status", serde_json::json!({})).unwrap();
    assert!(status.error.is_none());
    drop(client);
    // Only the child created above is signalled; no user sessions or hardware.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "shutdown readiness timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    let database = base.join("data/rad/state.sqlite3");
    assert!(database.is_file());
    let invalid = b"not a sqlite database; preserve this material";
    fs::write(&database, invalid).unwrap();
    let output = command().output().unwrap();
    assert_eq!(output.status.code(), Some(1), "unexpected, not blocking");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Material conservado"), "{stderr}");
    assert!(stderr.contains("restaurar manualmente"));
    assert_eq!(fs::read(database).unwrap(), invalid);
}
