//! Critical GUI scheduling contract, independent of a display server.
#[test]
fn editor_has_no_periodic_state_poll_or_repaint() {
    let app = include_str!("../src/app.rs");
    assert!(
        !app.contains("request_repaint_after"),
        "idle GUI must sleep"
    );
    assert!(
        !app.contains(".poll.due("),
        "state changes arrive as notices"
    );
    assert!(
        !app.contains(".result_poll.due("),
        "execution results arrive as notices"
    );
}
#[test]
fn continuous_notices_do_not_starve_any_ready_background_category() {
    let mut scheduler = rad_editor::schedule::Schedule::default();
    for _ in 0..4 {
        assert_eq!(
            (0..7)
                .map(|_| scheduler.choose([true; 7]).unwrap())
                .collect::<Vec<_>>(),
            (0..7).collect::<Vec<_>>()
        );
    }
}
#[test]
fn worker_wakes_after_publishing_a_result_without_gui_polling() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut worker = rad_editor::worker::Worker::spawn(|n: u8| n + 1);
    worker.set_waker(move || {
        let _ = tx.send(());
    });
    rx.recv().unwrap(); // registration covers already queued results
    worker.submit(4).unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(worker.poll(), Some(5));
}
