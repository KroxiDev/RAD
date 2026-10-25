//! Critical contract: no spontaneous checks and explicit results, without a desktop/network.
use rad_editor::updates::{Backend, Controller, Verification};
use rad_update::{Cancellation, Candidate, CheckResult, Prepared, Progress};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fake(Arc<AtomicUsize>);
impl Backend for Fake {
    fn check(&self, _: &Cancellation) -> Result<CheckResult, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CheckResult::NoUpdate)
    }
    fn prepare(&self, _: &Candidate, _: &Cancellation, _: &Progress) -> Result<Prepared, String> {
        panic!("not requested")
    }
    fn handoff(&self, _: &Prepared) -> Result<(), String> {
        panic!("not requested")
    }
    fn discard(&self, _: &Prepared) {}
}
fn settle(controller: &mut Controller) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while matches!(controller.verification(), Verification::Checking) {
        controller.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn only_an_explicit_request_checks_and_publishes_one_result() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut screen = Controller::new(Arc::new(Fake(calls.clone())));
    screen.enter();
    screen.poll();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(screen.verification(), &Verification::Unchecked);
    assert!(screen.check(), "explicit request must be accepted");
    assert!(
        !screen.check(),
        "double click must not create a second request"
    );
    settle(&mut screen);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(screen.verification(), &Verification::Current);
}

#[test]
fn leaving_cancels_but_keeps_the_slot_until_the_real_worker_finishes() {
    use std::sync::{Mutex, mpsc};
    struct Blocked {
        started: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        cancelled: Arc<AtomicUsize>,
    }
    impl Backend for Blocked {
        fn check(&self, cancel: &Cancellation) -> Result<CheckResult, String> {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.cancelled
                .store(usize::from(cancel.is_cancelled()), Ordering::SeqCst);
            Ok(CheckResult::NoUpdate)
        }
        fn prepare(
            &self,
            _: &Candidate,
            _: &Cancellation,
            _: &Progress,
        ) -> Result<Prepared, String> {
            panic!("not requested")
        }
        fn handoff(&self, _: &Prepared) -> Result<(), String> {
            panic!("not requested")
        }
        fn discard(&self, _: &Prepared) {}
    }
    let (started, seen) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut screen = Controller::new(Arc::new(Blocked {
        started,
        release: Mutex::new(wait),
        cancelled: cancelled.clone(),
    }));
    screen.enter();
    assert!(screen.check());
    seen.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(screen.leave());
    assert_eq!(screen.verification(), &Verification::Unchecked);
    screen.enter();
    assert!(
        !screen.check(),
        "a cancelled thread still owns the only slot"
    );
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while screen.busy() {
        screen.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(
        screen.verification(),
        &Verification::Unchecked,
        "late reply must not claim up to date"
    );
}

fn candidate() -> Candidate {
    serde_json::from_value(serde_json::json!({
        "tag":"v0.2.0-1", "installed_version":"0.1.0-1", "metadata":{
            "schema":1,"product":"RAD","package":"rust-action-deck","version":"0.2.0","pkgrel":1,
            "architecture":"x86_64","file":"rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst",
            "size":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "base":"omarchy-4.0.6","commit":"0000000000000000000000000000000000000000"
        }
    }))
    .unwrap()
}

#[test]
fn failed_preparation_never_hands_off_or_reuses_an_old_candidate() {
    use rad_editor::updates::Installation;
    struct Failure(Arc<AtomicUsize>);
    impl Backend for Failure {
        fn check(&self, _: &Cancellation) -> Result<CheckResult, String> {
            Ok(CheckResult::Available {
                candidate: Box::new(candidate()),
                managed: true,
            })
        }
        fn prepare(
            &self,
            _: &Candidate,
            _: &Cancellation,
            _: &Progress,
        ) -> Result<Prepared, String> {
            Err("digest mismatch".into())
        }
        fn handoff(&self, _: &Prepared) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn discard(&self, _: &Prepared) {}
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let mut screen = Controller::new(Arc::new(Failure(calls.clone())));
    screen.enter();
    screen.check();
    settle(&mut screen);
    assert!(screen.confirm());
    assert!(screen.prepare());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while screen.busy() {
        screen.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        screen.installation(),
        &Installation::Failed("digest mismatch".into())
    );
    assert_eq!(screen.verification(), &Verification::Unchecked);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        !screen.confirm(),
        "failed preparation must require a fresh check"
    );
}

#[test]
fn a_prepared_package_is_retained_until_the_authorized_handoff_owns_it() {
    use rad_editor::updates::Installation;
    struct Ready {
        handed: Arc<AtomicUsize>,
        discarded: Arc<AtomicUsize>,
    }
    impl Backend for Ready {
        fn check(&self, _: &Cancellation) -> Result<CheckResult, String> {
            Ok(CheckResult::Available {
                candidate: Box::new(candidate()),
                managed: true,
            })
        }
        fn prepare(
            &self,
            candidate: &Candidate,
            _: &Cancellation,
            _: &Progress,
        ) -> Result<Prepared, String> {
            Ok(Prepared {
                directory: "/synthetic/stage".into(),
                candidate: candidate.clone(),
            })
        }
        fn handoff(&self, _: &Prepared) -> Result<(), String> {
            self.handed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn discard(&self, _: &Prepared) {
            self.discarded.fetch_add(1, Ordering::SeqCst);
        }
    }
    let handed = Arc::new(AtomicUsize::new(0));
    let discarded = Arc::new(AtomicUsize::new(0));
    let mut screen = Controller::new(Arc::new(Ready {
        handed: handed.clone(),
        discarded: discarded.clone(),
    }));
    screen.enter();
    screen.check();
    settle(&mut screen);
    assert!(!screen.prepare(), "confirmation is required");
    screen.confirm();
    screen.prepare();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while screen.busy() {
        screen.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(screen.installation(), &Installation::Ready);
    assert_eq!(
        discarded.load(Ordering::SeqCst),
        0,
        "verified bytes must not be deleted before handoff"
    );
    assert!(screen.handoff());
    assert!(
        !screen.leave(),
        "an authorized handoff cannot be abandoned as a cancelled query"
    );
    while screen.busy() {
        screen.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(screen.installation(), &Installation::Relayed);
    assert_eq!(handed.load(Ordering::SeqCst), 1);
    drop(screen);
    assert_eq!(
        discarded.load(Ordering::SeqCst),
        0,
        "coordinator now owns staging"
    );
}
