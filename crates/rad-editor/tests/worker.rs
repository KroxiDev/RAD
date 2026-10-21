use rad_editor::worker::{Polled, SubmitError, Worker};
#[test]
fn worker_holds_one_credit_until_consumption_and_never_replays() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut w = Worker::spawn(move |job: u8| {
        tx.send(job).unwrap();
        "unknown"
    });
    assert_eq!(w.submit(7), Ok(()));
    assert_eq!(rx.recv().unwrap(), 7);
    assert_eq!(w.submit(8), Err(SubmitError::Busy));
    assert_eq!(w.receive(), Polled::Ready("unknown"));
    assert!(!w.busy());
    assert!(rx.try_recv().is_err());
    assert_eq!(w.submit(9), Ok(()));
    assert_eq!(w.receive(), Polled::Ready("unknown"));
    assert_eq!(rx.recv().unwrap(), 9);
}
#[test]
fn command_offered_during_a_job_waits_its_turn_ahead_of_later_background_work() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut w = Worker::spawn(move |job: u8| {
        tx.send(job).unwrap();
        job
    });
    assert_eq!(w.submit(1), Ok(()), "background poll in flight");
    assert_eq!(w.enqueue(2), Ok(()), "a user command is kept, not refused");
    assert_eq!(w.enqueue(3), Err(SubmitError::Busy), "only one may wait");
    assert_eq!(w.receive(), Polled::Ready(1));
    assert_eq!(
        w.submit(4),
        Err(SubmitError::Busy),
        "the command took the turn"
    );
    assert_eq!(w.receive(), Polled::Ready(2));
    assert!(!w.busy());
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(w.enqueue(5), Ok(()), "idle worker runs a command at once");
    assert_eq!(w.receive(), Polled::Ready(5));
}

/// Polls until the worker says something other than "nothing yet", within a deadline.
fn settle<J: Send + 'static, R: Send + 'static>(w: &mut Worker<J, R>) -> Polled<R> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match w.poll() {
            Polled::Pending => {
                assert!(std::time::Instant::now() < deadline, "worker never settled");
                std::thread::yield_now();
            }
            other => return other,
        }
    }
}
#[test]
fn panic_with_a_job_in_flight_is_failed_and_the_worker_is_no_longer_busy() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut w = Worker::spawn(move |job: u8| -> u8 {
        tx.send(job).unwrap();
        panic!("synthetic worker death");
    });
    assert_eq!(w.submit(1), Ok(()));
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(settle(&mut w), Polled::Failed { unsent: false });
    assert!(!w.busy());
    assert_eq!(w.submit(2), Err(SubmitError::Disconnected));
    assert_eq!(w.enqueue(3), Err(SubmitError::Disconnected));
    assert!(rx.try_recv().is_err(), "a lost job is never replayed");
}
#[test]
fn thread_ending_with_nothing_in_flight_is_closed() {
    let me = std::thread::current().id();
    let mut w = Worker::spawn(|job: u8| job);
    // Only the worker thread's wake-up panics, after its result has been delivered.
    w.set_waker(move || {
        if std::thread::current().id() != me {
            panic!("synthetic wake failure");
        }
    });
    assert_eq!(w.submit(1), Ok(()));
    assert_eq!(settle(&mut w), Polled::Ready(1));
    assert_eq!(settle(&mut w), Polled::Closed { unsent: false });
    assert!(!w.busy());
    assert_eq!(w.submit(2), Err(SubmitError::Disconnected));
}
#[test]
fn command_waiting_when_the_thread_dies_is_reported_as_unsent() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut w = Worker::spawn(move |job: u8| -> u8 {
        tx.send(job).unwrap();
        panic!("synthetic worker death");
    });
    assert_eq!(w.submit(1), Ok(()));
    assert_eq!(w.enqueue(2), Ok(()));
    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(settle(&mut w), Polled::Failed { unsent: true });
    assert!(!w.busy());
    assert!(rx.try_recv().is_err(), "the waiting command never ran");
}
#[test]
fn a_death_is_reported_once() {
    let mut w = Worker::spawn(|_: u8| -> u8 { panic!("synthetic worker death") });
    assert_eq!(w.submit(1), Ok(()));
    assert_eq!(settle(&mut w), Polled::Failed { unsent: false });
    assert_eq!(w.poll(), Polled::Pending);
}

#[test]
fn respawner_replaces_a_dead_worker_three_times_in_a_row_and_an_answer_restarts_the_count() {
    use rad_editor::worker::Respawner;
    let made = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = made.clone();
    let mut spawner = Respawner::new(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Worker::spawn(|job: u8| job)
    });
    let mut w = Worker::spawn(|job: u8| job);
    for _ in 0..3 {
        assert!(spawner.replace(&mut w, || {}));
    }
    assert!(!spawner.replace(&mut w, || {}), "the fourth death is final");
    assert!(spawner.gave_up());
    assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 3);

    spawner.revive(&mut w, || {});
    assert!(!spawner.gave_up());
    assert!(spawner.replace(&mut w, || {}));
    spawner.answered();
    for _ in 0..3 {
        assert!(
            spawner.replace(&mut w, || {}),
            "an answer restarted the count"
        );
    }
    assert!(!spawner.replace(&mut w, || {}));
}
