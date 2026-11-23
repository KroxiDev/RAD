mod support;
use std::{
    fs,
    time::{Duration, Instant},
};
use support::*;

fn threads() -> usize {
    fs::read_dir("/proc/self/task").unwrap().count()
}

fn settled_threads() -> usize {
    let mut last = threads();
    let mut steady = 0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while steady < 5 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        let now = threads();
        steady = if now == last { steady + 1 } else { 0 };
        last = now;
    }
    last
}

#[test]
fn quick_subscribe_unsubscribe_cycles_with_a_stuck_listing_never_pile_up_monitors() {
    let server = FakeServer::new();
    let timing = quick().refresh_deadline(Duration::from_millis(600));
    let mut host = Host::start(&server, timing);
    assert_eq!(
        host.options("action", "output", "device")["availability"],
        "available"
    );
    let baseline = settled_threads();
    server.hang(&["-f json list"]);

    let (mut peak_threads, mut peak_events, mut peak_hung) = (0, 0, 0);
    for _ in 0..40 {
        let started = server.subscribers_started();
        let subscription = host.subscribe("output", "@default");
        eventually(Duration::from_millis(30), || {
            server.subscribers_started() > started
        });
        host.unsubscribe(subscription);
        peak_threads = peak_threads.max(threads());
        peak_events = peak_events.max(server.live_subscribers());
        peak_hung = peak_hung.max(server.live_hung());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        peak_threads <= baseline + 4,
        "{peak_threads} threads at the peak, {baseline} before"
    );
    assert!(peak_events <= 1, "{peak_events} event children at once");
    assert!(peak_hung <= 1, "{peak_hung} hung listings at once");

    assert!(
        eventually(Duration::from_secs(3), || {
            server.live_subscribers() == 0 && server.live_hung() == 0 && threads() <= baseline
        }),
        "left behind: {} event children, {} hung, {} threads (baseline {baseline})",
        server.live_subscribers(),
        server.live_hung(),
        threads()
    );
    host.finish().unwrap();
}
