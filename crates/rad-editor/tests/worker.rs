use rad_editor::worker::{SubmitError, Worker};
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
    assert_eq!(w.receive(), Some("unknown"));
    assert!(!w.busy());
    assert!(rx.try_recv().is_err());
    assert_eq!(w.submit(9), Ok(()));
    assert_eq!(w.receive(), Some("unknown"));
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
    assert_eq!(w.receive(), Some(1));
    assert_eq!(
        w.submit(4),
        Err(SubmitError::Busy),
        "the command took the turn"
    );
    assert_eq!(w.receive(), Some(2));
    assert!(!w.busy());
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(w.enqueue(5), Ok(()), "idle worker runs a command at once");
    assert_eq!(w.receive(), Some(5));
}
