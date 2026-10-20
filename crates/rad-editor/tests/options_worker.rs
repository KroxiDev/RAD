//! The option lists use a connection of their own: a plugin that is slow to answer must not
//! delay Apply or anything else the user orders. A fake runtime holds the question open.
use rad_editor::{
    Request,
    native::{self, Job, OptionsJob, Reply},
    worker::{SubmitError, Worker},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

const SESSION: &str = "00000000000000000000000000000001";

fn read_request(stream: &mut UnixStream) -> Option<Value> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).ok()?;
    let mut body = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn write_response(stream: &mut UnixStream, request: &Value, status: &str, data: Value) {
    let response = json!({"v":1,"id":request["id"],"session_id":SESSION,"status":status,"data":data,"error":null});
    let bytes = serde_json::to_vec(&response).unwrap();
    let _ = stream.write_all(&(bytes.len() as u32).to_be_bytes());
    let _ = stream.write_all(&bytes);
}

/// Answers one connection until the peer closes it. An `options` question for the field `held`
/// is not answered until `released` gets a message; one for `once` is answered and then the
/// connection is closed, as the runtime closes an idle one.
fn serve(mut stream: UnixStream, asked: Sender<Value>, released: Arc<Mutex<Receiver<()>>>) {
    while let Some(request) = read_request(&mut stream) {
        match request["op"].as_str().unwrap() {
            "hello" => write_response(
                &mut stream,
                &request,
                "ok",
                json!({"protocol":1,"session_id":SESSION,"limits":{},"capabilities":[]}),
            ),
            "edit" => write_response(
                &mut stream,
                &request,
                "applied",
                json!({"changed":true,"revision":"2"}),
            ),
            "options" => {
                let field = request["args"]["field"].as_str().unwrap().to_owned();
                let _ = asked.send(request.clone());
                if field == "held" {
                    let _ = released.lock().unwrap().recv();
                }
                let data = json!({"type":"p:a","field":field,"availability":"available",
                    "items":[{"value":"v","label":"V"}],"next_cursor":null});
                write_response(&mut stream, &request, "ok", data);
                if field == "once" {
                    break;
                }
            }
            other => panic!("unexpected operation {other}"),
        }
    }
}

/// A runtime that answers `hello`, `edit` and `options`; see [`serve`].
struct FakeRuntime {
    root: PathBuf,
    questions: Receiver<Value>,
    release: Sender<()>,
    connections: Arc<Mutex<usize>>,
}

impl FakeRuntime {
    /// One thread per connection, as the real runtime serves several at once.
    fn start(name: &str) -> Self {
        Self::bind(name, false)
    }
    /// One connection at a time: the next is not even greeted until the previous is closed, so a
    /// client that opens a new connection before closing its old one is left waiting.
    fn serial(name: &str) -> Self {
        Self::bind(name, true)
    }
    fn bind(name: &str, serial: bool) -> Self {
        let root = std::env::temp_dir().join(format!("rad-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [root.clone(), root.join("rad")] {
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let listener = UnixListener::bind(root.join("rad/editor.sock")).unwrap();
        let (asked, questions) = channel();
        let (release, released) = channel();
        let released = Arc::new(Mutex::new(released));
        let connections = Arc::new(Mutex::new(0));
        let counted = connections.clone();
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                *counted.lock().unwrap() += 1;
                let (asked, released) = (asked.clone(), released.clone());
                if serial {
                    serve(stream, asked, released);
                } else {
                    std::thread::spawn(move || serve(stream, asked, released));
                }
            }
        });
        Self {
            root,
            questions,
            release,
            connections,
        }
    }
}

impl Drop for FakeRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn question(ticket: u64, field: &str) -> OptionsJob {
    OptionsJob {
        ticket,
        request: Request::new("options", json!({"type":"p:a","field":field,"cursor":null})),
    }
}

/// `poll` until a result arrives, so a worker that never answers fails the test rather than hang it.
fn wait<J: Send + 'static, R: Send + 'static>(worker: &mut Worker<J, R>) -> R {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = worker.poll() {
            return result;
        }
        assert!(Instant::now() < deadline, "no result within 5 s");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_question_in_flight_delays_neither_apply_nor_anything_else_and_queues_nothing() {
    let runtime = FakeRuntime::start("options-held");
    let mut main = native::start(runtime.root.clone(), runtime.root.join("no-theme"));
    main.submit(Job::Connect).unwrap();
    assert!(matches!(wait(&mut main).result, Ok(Reply::Connected)));
    let mut options = native::start_options(runtime.root.clone());

    options.submit(question(1, "held")).unwrap();
    runtime
        .questions
        .recv_timeout(Duration::from_secs(5))
        .expect("the runtime received the question");
    // The plugin has not answered. The user applies the form meanwhile.
    let apply = Request::new("edit", json!({"changes":[]}));
    main.enqueue(Job::PluginApply(apply)).unwrap();
    let applied = wait(&mut main);
    assert!(
        matches!(applied.result, Ok(Reply::PluginApplied(_))),
        "Apply completed while the question was still open"
    );
    assert!(options.busy(), "and the question is still open");
    assert_eq!(
        options.submit(question(2, "other")),
        Err(SubmitError::Busy),
        "no queue grows behind a slow question"
    );
    assert_eq!(
        options.enqueue(question(2, "other")),
        Ok(()),
        "one may wait, as in any worker"
    );
    assert_eq!(
        options.enqueue(question(3, "other")),
        Err(SubmitError::Busy)
    );

    runtime.release.send(()).unwrap();
    let first = wait(&mut options);
    assert_eq!(first.ticket, 1, "the answer is tagged with its own ticket");
    assert_eq!(first.result.unwrap()["items"][0]["value"], "v");
    let second = wait(&mut options);
    assert_eq!(second.ticket, 2);
    assert!(!options.busy());
}

#[test]
fn the_options_connection_is_made_when_first_needed_and_made_again_after_it_is_lost() {
    let runtime = FakeRuntime::start("options-lost");
    let mut options = native::start_options(runtime.root.clone());
    assert_eq!(
        *runtime.connections.lock().unwrap(),
        0,
        "nothing is opened before a question"
    );

    options.submit(question(1, "once")).unwrap();
    assert_eq!(wait(&mut options).result.unwrap()["field"], "once");
    // The runtime closed the connection after answering; the next question finds it dead.
    options.submit(question(2, "next")).unwrap();
    let lost = wait(&mut options);
    assert_eq!(lost.ticket, 2);
    assert_eq!(lost.result.unwrap_err().code, "runtime_unavailable");
    // The one after that connects again instead of staying broken.
    options.submit(question(3, "next")).unwrap();
    let again = wait(&mut options);
    assert_eq!(again.result.unwrap()["field"], "next");
    assert_eq!(*runtime.connections.lock().unwrap(), 2);
}

#[test]
fn a_connection_idle_for_too_long_is_closed_before_the_new_one_is_opened_and_a_busy_one_is_kept() {
    let runtime = FakeRuntime::serial("options-idle");
    let mut options = native::start_options_idle(runtime.root.clone(), Duration::from_millis(500));
    for (ticket, field) in [(1, "a"), (2, "b")] {
        options.submit(question(ticket, field)).unwrap();
        wait(&mut options).result.unwrap();
    }
    assert_eq!(
        *runtime.connections.lock().unwrap(),
        1,
        "questions in a row share one connection"
    );

    std::thread::sleep(Duration::from_millis(800));
    options.submit(question(3, "c")).unwrap();
    let after_idle = wait(&mut options);
    assert_eq!(
        after_idle.result.unwrap()["field"],
        "c",
        "the new connection works"
    );
    assert_eq!(
        *runtime.connections.lock().unwrap(),
        2,
        "the idle one was replaced, and only after it was closed (the runtime serves one at a time)"
    );
}
