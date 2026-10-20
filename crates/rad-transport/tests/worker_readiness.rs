use rad_transport::Transport;
use std::{
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn idle_actor_wait_is_interrupted_by_worker_completion_and_helper_io() {
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-worker-{}", std::process::id())));
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut transport = Transport::bind(&directory.0, 984).unwrap();
    transport.drive(0).unwrap();
    let wake = Arc::new(
        transport
            .worker_waker()
            .expect("Store completion must wake the single actor wait"),
    );
    let worker_wake = wake.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        worker_wake.wake().unwrap();
    });
    let start = Instant::now();
    transport.wait_readiness(Duration::from_secs(2)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    thread.join().unwrap();
    let (mut read, mut write) = mio::net::UnixStream::pair().unwrap();
    transport
        .register_worker_io(&mut read, mio::Interest::READABLE)
        .unwrap();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        write.write_all(b"x").unwrap();
    });
    let start = Instant::now();
    transport.wait_readiness(Duration::from_secs(2)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    thread.join().unwrap();
    let mut byte = [0];
    read.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [b'x']);
}
#[test]
fn worker_edge_consumed_by_drive_keeps_the_actor_from_blocking() {
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-worker-edge-{}", std::process::id())));
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut transport = Transport::bind(&directory.0, 985).unwrap();
    transport.drive(0).unwrap();
    let (mut read, mut write) = mio::net::UnixStream::pair().unwrap();
    transport
        .register_worker_io(&mut read, mio::Interest::READABLE)
        .unwrap();
    // Ready after its owner was pumped this turn, but before `drive` polls.
    write.write_all(b"x").unwrap();
    transport.drive(1).unwrap();
    let start = Instant::now();
    transport.wait_readiness(Duration::from_secs(2)).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "an edge swallowed by drive must force another turn instead of blocking"
    );
    // Reported once: an idle actor blocks again rather than spinning.
    transport.drive(2).unwrap();
    let start = Instant::now();
    transport.wait_readiness(Duration::from_millis(60)).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(50));
}
