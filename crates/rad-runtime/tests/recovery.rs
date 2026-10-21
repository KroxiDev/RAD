//! Recovery mode (ADR 0011) seen from the client sockets, in process: the runtime that
//! cannot open the configuration keeps serving the narrow recovery surface and leaves
//! the damaged file alone until «Empezar de nuevo».
use rad_client::{
    connection::{Client, Endpoint},
    response::{Outcome, Response, Status},
};
use rad_runtime::{
    bootstrap,
    lifecycle::Paths,
    recovery::Recovery,
    server::Server,
    startup::{Bound, Opened},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    num::NonZeroU128,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rad-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }
    fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    fn database(&self) -> PathBuf {
        self.data().join("state.sqlite3")
    }
    fn session(&self) -> NonZeroU128 {
        NonZeroU128::new(0xABCD_0000 + NEXT.fetch_add(1, Ordering::Relaxed) as u128).unwrap()
    }
    /// The data directory with exactly `database` in it.
    fn with_database(&self, database: &[u8]) {
        std::fs::create_dir(self.data()).unwrap();
        std::fs::set_permissions(self.data(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(self.database(), database).unwrap();
        std::fs::set_permissions(self.database(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn open(&self) -> Opened {
        Bound::bind(&self.root, self.session())
            .unwrap()
            .open(&self.data())
            .unwrap()
    }
    fn recovery(&self) -> Mode {
        match self.open() {
            Opened::Recovery(recovery) => Mode::Recovery(recovery),
            Opened::Serving(_) => panic!("recovery mode expected"),
        }
    }
    fn evidence(&self) -> Vec<PathBuf> {
        let mut found: Vec<_> = std::fs::read_dir(self.data())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("state.sqlite3.damaged-")
            })
            .collect();
        found.sort();
        found
    }
    /// Names, sizes and modification times of everything in the data directory.
    fn snapshot(&self) -> Vec<(String, u64, std::time::SystemTime)> {
        self.snapshot_of(&self.data())
    }
    fn snapshot_of(&self, directory: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let meta = std::fs::symlink_metadata(entry.path()).unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    meta.len(),
                    meta.modified().unwrap(),
                )
            })
            .collect();
        entries.sort();
        entries
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// What the binary's loop does: serve recovery, and become the Server in the same process
/// once a restart is done.
enum Mode {
    Recovery(Box<Recovery>),
    Serving(Box<Server>),
    Moving,
}
impl Mode {
    fn turn(&mut self) {
        if let Mode::Recovery(recovery) = self {
            recovery.turn().unwrap();
        }
        if matches!(self, Mode::Recovery(recovery) if recovery.reset_done()) {
            let Mode::Recovery(recovery) = std::mem::replace(self, Mode::Moving) else {
                unreachable!()
            };
            *self = Mode::Serving(Box::new(recovery.into_server().unwrap()));
        }
        if let Mode::Serving(server) = self {
            server.turn().unwrap();
        }
    }
    fn wait(&mut self, maximum: Duration) {
        match self {
            Mode::Recovery(recovery) => recovery.wait(maximum).unwrap(),
            Mode::Serving(server) => server.wait(maximum).unwrap(),
            Mode::Moving => unreachable!(),
        }
    }
    /// Runs the client `work` on its own thread while this serves.
    fn run<T: Send + 'static>(&mut self, work: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, receive) = mpsc::sync_channel(1);
        let client = std::thread::spawn(move || send.send(work()).unwrap());
        let end = Instant::now() + Duration::from_secs(10);
        let value = loop {
            self.turn();
            if let Ok(value) = receive.try_recv() {
                break value;
            }
            assert!(Instant::now() < end, "the runtime must answer in time");
            self.wait(Duration::from_millis(20));
        };
        client.join().unwrap();
        value
    }
}

fn connect(root: &Path, endpoint: Endpoint) -> Client {
    Client::connect(root, endpoint).expect("recovery mode negotiates the handshake")
}
/// `(error code, outcome)` of a refusal.
fn refusal(response: &Response) -> (&str, Option<Outcome>) {
    assert_eq!(response.status, Status::Error, "{response:?}");
    let error = response.error.as_ref().expect("error");
    (error.code.as_str(), error.outcome)
}
fn data(response: &Response) -> Value {
    serde_json::to_value(&response.data).unwrap()
}
/// `hello` as the wire carries it: `Client::connect` already consumed the first one.
fn announced(root: &Path, endpoint: &str) -> Value {
    let mut stream = UnixStream::connect(root.join("rad").join(endpoint)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = br#"{"v":1,"id":"1","op":"hello","args":{}}"#;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend(body);
    stream.write_all(&frame).unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let mut payload = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut payload).unwrap();
    serde_json::from_slice::<Value>(&payload).unwrap()["data"].clone()
}
fn recovery_view(root: PathBuf) -> Value {
    let mut editor = connect(&root, Endpoint::Editor);
    data(&editor.request("recovery", json!({})).unwrap())
}

#[test]
fn an_unreadable_configuration_serves_only_the_recovery_surface() {
    let fixture = Fixture::new();
    fixture.with_database(b"bytes that are not a database");
    let mut mode = fixture.recovery();
    let Mode::Recovery(recovery) = &mode else {
        unreachable!()
    };
    assert_eq!(recovery.cause(), rad_api::RecoveryCause::Unreadable);
    let root = fixture.root.clone();
    mode.run(move || {
        let editor = announced(&root, "editor.sock");
        assert_eq!(
            editor["capabilities"],
            json!([
                "hello",
                "recovery",
                "recovery_reset",
                "transfer_begin",
                "transfer_write",
                "transfer_finish",
                "transfer_drop",
                "restore_commit"
            ])
        );
        assert_eq!(editor["limits"]["frame_bytes"], 65536);
        let automation = announced(&root, "automation.sock");
        assert_eq!(automation["capabilities"], json!(["hello"]));
        assert_eq!(automation["limits"], editor["limits"]);

        let mut editor = connect(&root, Endpoint::Editor);
        let view = editor.request("recovery", json!({})).unwrap();
        assert_eq!(view.status, Status::Ok);
        assert_eq!(
            data(&view),
            json!({
                "cause":"unreadable",
                "actions":["restore","reset","details"],
                "detail":{"code":"corrupt","counters_read":false}
            })
        );
        for operation in ["status", "overview", "runtime_log", "board"] {
            let answer = editor.request(operation, json!({})).unwrap();
            assert_eq!(refusal(&answer), ("recovery_required", None), "{operation}");
        }
        for (operation, args) in [
            ("edit", json!({"changes":[]})),
            (
                "create_button",
                json!({"folder_id":"f1","position":0,"name":"x"}),
            ),
            ("undo", json!({})),
        ] {
            let answer = editor.request(operation, args).unwrap();
            assert_eq!(
                refusal(&answer),
                ("recovery_required", Some(Outcome::NotApplied)),
                "{operation}"
            );
        }
        let mut automation = connect(&root, Endpoint::Automation);
        assert_eq!(
            refusal(&automation.request("recovery", json!({})).unwrap()),
            ("permission_denied", None)
        );
        assert_eq!(
            refusal(&automation.request("status", json!({})).unwrap()),
            ("recovery_required", None)
        );
        assert_eq!(
            refusal(&automation.request("edit", json!({"changes":[]})).unwrap()),
            ("recovery_required", Some(Outcome::NotApplied))
        );
    });
    assert_eq!(
        std::fs::read(fixture.database()).unwrap(),
        b"bytes that are not a database"
    );
    assert!(fixture.evidence().is_empty());
}

#[test]
fn starting_again_needs_confirmation_keeps_the_evidence_and_the_same_connection() {
    let fixture = Fixture::new();
    let damaged = b"bytes that are not a database";
    fixture.with_database(damaged);
    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        let mut automation = connect(&root, Endpoint::Automation);
        assert_eq!(
            refusal(
                &editor
                    .request("recovery_reset", json!({"confirm":false}))
                    .unwrap()
            ),
            ("confirmation_required", Some(Outcome::NotApplied))
        );
        assert_eq!(
            refusal(
                &automation
                    .request("recovery_reset", json!({"confirm":true}))
                    .unwrap()
            ),
            ("permission_denied", Some(Outcome::NotApplied))
        );
        let reset = editor
            .request("recovery_reset", json!({"confirm":true}))
            .unwrap();
        assert_eq!(reset.status, Status::Applied, "{reset:?}");
        assert_eq!(data(&reset)["changed"], true);
        assert!(data(&reset)["revision"].is_string());
        // The same connections now find the normal runtime.
        let overview = editor.request("overview", json!({})).unwrap();
        assert_eq!(overview.status, Status::Ok, "{overview:?}");
        assert_eq!(
            automation.request("status", json!({})).unwrap().status,
            Status::Ok
        );
        let log = data(&editor.request("runtime_log", json!({})).unwrap());
        let entries: Vec<_> = log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["code"].as_str().unwrap(), entry["subject"].as_str()))
            .collect();
        assert_eq!(
            entries,
            [
                ("started", None),
                ("recovery_mode", Some("unreadable")),
                ("recovery_reset", None)
            ]
        );
        // Recovery is over: its operations are not served any more.
        assert_eq!(
            refusal(&editor.request("recovery", json!({})).unwrap()),
            ("invalid_request", None)
        );
    });
    let evidence = fixture.evidence();
    assert_eq!(evidence.len(), 1);
    assert_eq!(std::fs::read(&evidence[0]).unwrap(), damaged);
    assert!(matches!(mode, Mode::Serving(_)));
    drop(mode);
    assert!(bootstrap::open(&fixture.data(), fixture.session(), 1).is_ok());
}

/// A change made right after another may find the previous publication still settling.
fn create_button(editor: &mut Client, folder: &str, position: usize, name: &str) -> Response {
    for _ in 0..100 {
        let made = editor
            .request(
                "create_button",
                json!({"folder_id":folder,"position":position,"name":name}),
            )
            .unwrap();
        if made.error.as_ref().is_none_or(|error| error.code != "busy") {
            return made;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the runtime stayed busy")
}

/// A copy of the configuration as the real server exports it: base64 chunks with the
/// bytes each one holds.
type Copy = Vec<(String, usize)>;
fn export_copy(editor: &mut Client) -> Copy {
    let token = request_ok(
        editor,
        "transfer_begin",
        json!({"kind":"export","size":null}),
    )["transfer_id"]
        .clone();
    let (mut pieces, mut offset) = (vec![], 0);
    loop {
        let part = request_ok(
            editor,
            "transfer_read",
            json!({"transfer_id":token,"offset":offset.to_string()}),
        );
        let text = part["base64"].as_str().unwrap().to_owned();
        let bytes = text.len() / 4 * 3 - text.bytes().rev().take_while(|b| *b == b'=').count();
        offset += bytes;
        pieces.push((text, bytes));
        if part["eof"] == true {
            return pieces;
        }
    }
}
/// Uploads `copy` and returns the token with the answer to `transfer_finish`.
fn upload_copy(editor: &mut Client, copy: &Copy) -> (Value, Value) {
    let size: usize = copy.iter().map(|(_, bytes)| bytes).sum();
    let token = request_ok(
        editor,
        "transfer_begin",
        json!({"kind":"restore","size":size.to_string()}),
    )["transfer_id"]
        .clone();
    let mut offset = 0;
    for (text, bytes) in copy {
        request_ok(
            editor,
            "transfer_write",
            json!({"transfer_id":token,"offset":offset.to_string(),"base64":text}),
        );
        offset += bytes;
    }
    let prepared = request_ok(editor, "transfer_finish", json!({"transfer_id":token}));
    (token, prepared)
}
fn request_ok(client: &mut Client, operation: &str, args: Value) -> Value {
    let answer = client.request(operation, args).unwrap();
    assert!(answer.error.is_none(), "{operation}: {answer:?}");
    data(&answer)
}

/// Creates two buttons through a real server, so both the current and the previous
/// revision name the first one; returns the installation of that data.
fn populated(fixture: &Fixture) -> u128 {
    populate(fixture, false).0
}
/// The same, plus a copy exported after the two buttons and a third button the copy does
/// not hold.
fn populated_with_copy(fixture: &Fixture) -> (u128, Copy) {
    populate(fixture, true)
}
fn populate(fixture: &Fixture, exporting: bool) -> (u128, Copy) {
    let Opened::Serving(server) = fixture.open() else {
        panic!("a new folder starts normally")
    };
    let mut mode = Mode::Serving(server);
    let root = fixture.root.clone();
    let copy = mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        for (position, name) in ["Marker-button-name", "Other"].into_iter().enumerate() {
            let made = create_button(&mut editor, "f1", position, name);
            assert_eq!(made.status, Status::Applied, "{made:?}");
        }
        if !exporting {
            return vec![];
        }
        let copy = export_copy(&mut editor);
        let made = create_button(&mut editor, "f1", 2, "Later");
        assert_eq!(made.status, Status::Applied, "{made:?}");
        copy
    });
    drop(mode);
    (installation(fixture), copy)
}
fn installation(fixture: &Fixture) -> u128 {
    let boot = bootstrap::open(&fixture.data(), fixture.session(), 1).unwrap();
    boot.store.load().unwrap().registry.installation()
}
/// Flips one byte inside every stored copy of `marker`: SQLite still reads the file, but
/// the digest of every revision document no longer matches.
fn spoil_every_revision(file: &Path, marker: &[u8]) {
    let mut bytes = std::fs::read(file).unwrap();
    let mut spoiled = 0;
    for start in 0..bytes.len() - marker.len() {
        if &bytes[start..start + marker.len()] == marker {
            bytes[start] ^= 0x01;
            spoiled += 1;
        }
    }
    assert!(
        spoiled >= 2,
        "the current and the previous revision name it"
    );
    std::fs::write(file, bytes).unwrap();
}

#[test]
fn identifiers_of_a_damaged_but_readable_file_are_not_reused() {
    let fixture = Fixture::new();
    let installation_before = populated(&fixture);
    spoil_every_revision(&fixture.database(), b"Marker-button-name");

    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    let created = mode.run(move || {
        let view = recovery_view(root.clone());
        assert_eq!(view["cause"], "unreadable");
        assert_eq!(view["detail"]["counters_read"], true);
        let mut editor = connect(&root, Endpoint::Editor);
        let reset = editor
            .request("recovery_reset", json!({"confirm":true}))
            .unwrap();
        assert_eq!(reset.status, Status::Applied, "{reset:?}");
        let made = create_button(&mut editor, "f2", 0, "After");
        assert_eq!(made.status, Status::Applied, "{made:?}");
        data(&editor.request("get", json!({"id":"b3"})).unwrap())
    });
    // The new Principal folder is f2 (f1 was the old one) and the new button follows the
    // two the damaged file had handed out.
    assert_eq!(created["fields"]["name"], "After");
    drop(mode);
    assert_eq!(installation(&fixture), installation_before);
}

#[test]
fn a_newer_version_is_reported_and_never_replaced() {
    let fixture = Fixture::new();
    populated(&fixture);
    let mut bytes = std::fs::read(fixture.database()).unwrap();
    bytes[60..64].copy_from_slice(&3u32.to_be_bytes());
    std::fs::write(fixture.database(), &bytes).unwrap();

    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    mode.run(move || {
        assert_eq!(
            recovery_view(root.clone()),
            json!({
                "cause":"newer_version",
                "actions":["details"],
                "detail":{"code":"unsupported_version","counters_read":false}
            })
        );
        let mut editor = connect(&root, Endpoint::Editor);
        assert_eq!(
            refusal(
                &editor
                    .request("recovery_reset", json!({"confirm":true}))
                    .unwrap()
            ),
            ("operation_not_allowed", Some(Outcome::NotApplied))
        );
    });
    assert_eq!(std::fs::read(fixture.database()).unwrap(), bytes);
    assert!(fixture.evidence().is_empty());
}

#[test]
fn an_unsafe_database_file_is_reported_and_nothing_is_written() {
    // A symlink and a second link are both refused, whatever is behind them.
    for (name, expected) in [
        ("symlink", "unsafe_database_file"),
        ("hard link", "unsafe_database_file"),
    ] {
        let fixture = Fixture::new();
        drop(bootstrap::open(&fixture.data(), fixture.session(), 1).unwrap());
        match name {
            "symlink" => {
                let real = fixture.root.join("elsewhere.sqlite3");
                std::fs::rename(fixture.database(), &real).unwrap();
                std::os::unix::fs::symlink(&real, fixture.database()).unwrap();
            }
            _ => std::fs::hard_link(fixture.database(), fixture.data().join("extra")).unwrap(),
        }
        let before = fixture.snapshot();
        let mut mode = fixture.recovery();
        let root = fixture.root.clone();
        mode.run(move || {
            assert_eq!(
                recovery_view(root.clone()),
                json!({
                    "cause":"unsafe_folder",
                    "actions":["details"],
                    "detail":{"code":expected,"counters_read":false}
                })
            );
            let mut editor = connect(&root, Endpoint::Editor);
            assert_eq!(
                refusal(
                    &editor
                        .request("recovery_reset", json!({"confirm":true}))
                        .unwrap()
                ),
                ("operation_not_allowed", Some(Outcome::NotApplied)),
                "{name}"
            );
        });
        assert_eq!(fixture.snapshot(), before, "{name}");
    }
}

#[test]
fn an_unsafe_data_directory_is_reported_before_anything_is_written_in_it() {
    let fixture = Fixture::new();
    fixture.with_database(b"left alone");
    std::fs::set_permissions(fixture.data(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = fixture.snapshot();

    // Through the runtime's own checks...
    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    mode.run(move || {
        assert_eq!(
            recovery_view(root),
            json!({
                "cause":"unsafe_folder",
                "actions":["details"],
                "detail":{"code":"unsafe_directory","counters_read":false}
            })
        );
    });
    drop(mode);
    assert_eq!(fixture.snapshot(), before);
    // ...and through the binary's: `Paths` refuses the folder, so the Bound goes straight
    // to recovery without opening or reading anything.
    let paths = Paths::from_values(
        Some(fixture.root.as_os_str()),
        Some(fixture.root.join("xdg").as_os_str()),
        None,
        Path::new("/usr/bin/rad-runtime"),
    )
    .unwrap();
    std::fs::create_dir(fixture.root.join("xdg")).unwrap();
    std::fs::rename(fixture.data(), &paths.data).unwrap();
    let before = fixture.snapshot_of(&paths.data);
    assert!(paths.prepare_data().is_err());
    let recovery = Bound::bind(&fixture.root, fixture.session())
        .unwrap()
        .unsafe_folder(&paths.data);
    let mut mode = Mode::Recovery(Box::new(recovery));
    let root = fixture.root.clone();
    mode.run(move || {
        assert_eq!(recovery_view(root)["detail"]["code"], "unsafe_directory");
    });
    assert_eq!(fixture.snapshot_of(&paths.data), before);
}

#[test]
fn a_hot_journal_that_survives_the_failed_open_is_kept_with_the_evidence() {
    let fixture = Fixture::new();
    fixture.with_database(b"bytes that are not a database");
    // SQLite replays a valid hot journal when it opens and deletes it; one whose header
    // is zero is left where it is, and would be replayed into whatever base came next.
    let mut journal = vec![0u8; 28];
    journal.extend_from_slice(b"journal bytes that must survive");
    let journal_path = fixture.data().join("state.sqlite3-journal");
    std::fs::write(&journal_path, &journal).unwrap();
    let mut mode = fixture.recovery();
    assert!(
        journal_path.exists(),
        "the journal survived the failed open"
    );
    let root = fixture.root.clone();
    mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        let reset = editor
            .request("recovery_reset", json!({"confirm":true}))
            .unwrap();
        assert_eq!(reset.status, Status::Applied, "{reset:?}");
        assert_eq!(
            editor.request("overview", json!({})).unwrap().status,
            Status::Ok
        );
    });
    assert!(!journal_path.exists());
    let evidence = fixture.evidence();
    let mut names: Vec<_> = evidence
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[1], format!("{}-journal", names[0]));
    assert_eq!(std::fs::read(&evidence[1]).unwrap(), journal);
}

/// Restores `copy` from recovery mode on `fixture`, whose database is already damaged,
/// and runs `after` on the editor connection that did it.
fn restore_into<T: Send + 'static>(
    fixture: &Fixture,
    copy: Copy,
    after: impl FnOnce(&mut Client) -> T + Send + 'static,
) -> T {
    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    let value = mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        let (token, prepared) = upload_copy(&mut editor, &copy);
        assert_eq!(prepared["summary"]["entities"], 3);
        let restored = editor
            .request(
                "restore_commit",
                json!({"transfer_id":token,"confirm_replace":true}),
            )
            .unwrap();
        assert_eq!(restored.status, Status::Applied, "{restored:?}");
        assert_eq!(data(&restored)["changed"], true);
        // The same connection now finds the normal runtime, with the copy in it.
        assert_eq!(
            editor.request("overview", json!({})).unwrap().status,
            Status::Ok
        );
        after(&mut editor)
    });
    assert!(matches!(mode, Mode::Serving(_)));
    value
}
fn name_of(editor: &mut Client, id: &str) -> Option<String> {
    let answer = editor.request("get", json!({"id":id})).unwrap();
    (answer.status == Status::Ok)
        .then(|| data(&answer)["fields"]["name"].as_str().unwrap().to_owned())
}

#[test]
fn a_copy_of_this_installation_keeps_its_identifiers_and_does_not_reuse_the_damaged_ones() {
    let fixture = Fixture::new();
    let (installation_before, copy) = populated_with_copy(&fixture);
    spoil_every_revision(&fixture.database(), b"Marker-button-name");
    let damaged = std::fs::read(fixture.database()).unwrap();

    let created = restore_into(&fixture, copy, |editor| {
        assert_eq!(name_of(editor, "b1").as_deref(), Some("Marker-button-name"));
        assert_eq!(name_of(editor, "b2").as_deref(), Some("Other"));
        assert_eq!(name_of(editor, "b3"), None, "the damaged file's own button");
        let made = create_button(editor, "f1", 2, "After");
        assert_eq!(made.status, Status::Applied, "{made:?}");
        name_of(editor, "b4")
    });
    // The new button follows the three the damaged file had handed out.
    assert_eq!(created.as_deref(), Some("After"));
    let evidence = fixture.evidence();
    assert_eq!(evidence.len(), 1);
    assert_eq!(std::fs::read(&evidence[0]).unwrap(), damaged);
    assert_eq!(installation(&fixture), installation_before);
}

#[test]
fn a_copy_of_another_installation_is_remapped_over_the_damaged_ones_identity() {
    let fixture = Fixture::new();
    let installation_before = populated(&fixture);
    spoil_every_revision(&fixture.database(), b"Marker-button-name");
    let other = Fixture::new();
    let (installation_other, copy) = populated_with_copy(&other);
    assert_ne!(installation_other, installation_before);

    restore_into(&fixture, copy, |editor| {
        // The copy's b1 and b2 are not the new identifiers: those follow the damaged file's.
        assert_eq!(name_of(editor, "b1"), None);
        assert_eq!(name_of(editor, "b2"), None);
        let mut names: Vec<_> = ["b3", "b4"]
            .into_iter()
            .map(|id| name_of(editor, id).expect(id))
            .collect();
        names.sort();
        assert_eq!(names, ["Marker-button-name", "Other"]);
    });
    assert_eq!(installation(&fixture), installation_before);
}

#[test]
fn restoring_needs_confirmation_and_only_an_unreadable_configuration_may_do_it() {
    let fixture = Fixture::new();
    fixture.with_database(b"bytes that are not a database");
    let mut mode = fixture.recovery();
    let root = fixture.root.clone();
    mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        assert_eq!(
            refusal(
                &editor
                    .request(
                        "restore_commit",
                        json!({"transfer_id":"x","confirm_replace":false})
                    )
                    .unwrap()
            ),
            ("confirmation_required", Some(Outcome::NotApplied))
        );
        assert_eq!(
            refusal(
                &editor
                    .request("transfer_begin", json!({"kind":"export","size":null}))
                    .unwrap()
            ),
            ("operation_not_allowed", Some(Outcome::NotApplied))
        );
        assert_eq!(
            refusal(
                &editor
                    .request("transfer_read", json!({"transfer_id":"x","offset":"0"}))
                    .unwrap()
            ),
            ("recovery_required", None)
        );
        let mut automation = connect(&root, Endpoint::Automation);
        assert_eq!(
            refusal(
                &automation
                    .request("transfer_begin", json!({"kind":"restore","size":"10"}))
                    .unwrap()
            ),
            ("permission_denied", Some(Outcome::NotApplied))
        );
    });
    drop(mode);

    let newer = Fixture::new();
    populated(&newer);
    // The server that made the data left its own transfer folder; recovery must not add one.
    std::fs::remove_dir_all(newer.data().join("transfers")).unwrap();
    let mut bytes = std::fs::read(newer.database()).unwrap();
    bytes[60..64].copy_from_slice(&3u32.to_be_bytes());
    std::fs::write(newer.database(), &bytes).unwrap();
    let mut mode = newer.recovery();
    let root = newer.root.clone();
    mode.run(move || {
        let mut editor = connect(&root, Endpoint::Editor);
        assert_eq!(
            refusal(
                &editor
                    .request("transfer_begin", json!({"kind":"restore","size":"10"}))
                    .unwrap()
            ),
            ("operation_not_allowed", Some(Outcome::NotApplied))
        );
    });
    assert!(!newer.data().join("transfers").exists());
    assert_eq!(std::fs::read(newer.database()).unwrap(), bytes);
}
