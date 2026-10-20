//! Synchronous bounded client. Run on the CLI thread or an editor I/O worker,
//! never the GUI or runtime authority loop. Failed requests are never replayed.
use crate::response::{Outcome, Response, Status};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Automation,
    Editor,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub id: Option<u64>,
    pub session_id: Option<String>,
    pub outcome: Option<Outcome>,
}
pub struct Client {
    stream: UnixStream,
    session: Option<String>,
    next_id: u64,
    closed: bool,
}
impl Client {
    pub fn connect(runtime_dir: &Path, endpoint: Endpoint) -> Result<Self, Failure> {
        if !private_directory(runtime_dir) || !private_directory(&runtime_dir.join("rad")) {
            return Err(Failure {
                code: "runtime_unavailable",
                id: None,
                session_id: None,
                outcome: None,
            });
        }
        let name = match endpoint {
            Endpoint::Automation => "automation.sock",
            Endpoint::Editor => "editor.sock",
        };
        // Mio's connect is nonblocking, including a full AF_UNIX backlog. Failure
        // is reported, never retried. The safe std conversion retains ownership.
        let stream: mio::net::UnixStream =
            mio::net::UnixStream::connect(runtime_dir.join("rad").join(name)).map_err(|_| {
                Failure {
                    code: "runtime_unavailable",
                    id: None,
                    session_id: None,
                    outcome: None,
                }
            })?;
        let stream: UnixStream = stream.into();
        stream.set_nonblocking(false).map_err(|_| Failure {
            code: "runtime_unavailable",
            id: None,
            session_id: None,
            outcome: None,
        })?;
        let mut client = Self {
            stream,
            session: None,
            next_id: 1,
            closed: false,
        };
        let response = client.request("hello", json!({}))?;
        if response.status != Status::Ok
            || response.data["protocol"] != 1
            || response.data["session_id"]
                != response
                    .session_id
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null)
            || !response.data["limits"].is_object()
            || !response.data["capabilities"].is_array()
        {
            return Err(client.fail("invalid_request", Some(1), None));
        }
        client.session = response.session_id;
        Ok(client)
    }
    pub fn request(&mut self, operation: &str, arguments: Value) -> Result<Response, Failure> {
        self.request_until(
            operation,
            arguments,
            Instant::now() + Duration::from_secs(10),
        )
    }
    /// The deadline may shorten, but cannot lengthen, the ordinary ten seconds.
    pub fn request_until(
        &mut self,
        operation: &str,
        arguments: Value,
        deadline: Instant,
    ) -> Result<Response, Failure> {
        let mutating = !matches!(
            operation,
            "hello"
                | "status"
                | "get"
                | "get_section"
                | "list"
                | "schema"
                | "options"
                | "launcher_catalog"
                | "plugin_catalog"
                | "plugin_configuration"
                | "plugin_status"
                | "plugin_visual"
                | "plugin_visual_read"
                | "result"
                | "media_read"
                | "transfer_read"
        );
        let prior = mutating.then_some(Outcome::NotApplied);
        if self.closed {
            return Err(self.fail("runtime_unavailable", None, prior));
        }
        let id = self.next_id;
        if id == 0 {
            return Err(self.fail("resource_exhausted", None, prior));
        }
        let bytes =
            serde_json::to_vec(&json!({"v":1,"id":id.to_string(),"op":operation,"args":arguments}))
                .map_err(|_| self.fail("invalid_request", Some(id), prior))?;
        if operation.is_empty() || !arguments.is_object() || rad_wire::parse_json(&bytes).is_err() {
            return Err(self.fail("invalid_request", Some(id), prior));
        }
        self.next_id = id.checked_add(1).unwrap_or(0);
        let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
        frame.extend(bytes);
        let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
        let mut sent = false;
        let result = (|| {
            let mut remaining = frame.as_slice();
            while !remaining.is_empty() {
                self.stream.set_write_timeout(Some(time_left(deadline)?))?;
                match self.stream.write(remaining) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(n) => {
                        sent = true;
                        remaining = &remaining[n..];
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            let mut header = [0; 4];
            read_exact_until(&mut self.stream, &mut header, deadline)?;
            let length = u32::from_be_bytes(header) as usize;
            if !(1..=65536).contains(&length) {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut payload = vec![0; length];
            read_exact_until(&mut self.stream, &mut payload, deadline)?;
            let response = Response::decode(&payload, id, self.session.as_deref())
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            let expected = match operation {
                "run" => Status::Admitted,
                "edit" | "create_folder" | "create_button" | "add_action" | "duplicate"
                | "delete" | "move_folder" | "set_permissions" | "undo" | "redo"
                | "set_settings" | "restore_commit" => Status::Applied,
                _ => Status::Ok,
            };
            if mutating
                && response
                    .error
                    .as_ref()
                    .is_some_and(|error| error.outcome.is_none())
            {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if response.status != Status::Error && response.status != expected {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(response)
        })();
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                let code = if error.kind() == io::ErrorKind::InvalidData {
                    "invalid_request"
                } else if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    "deadline_exceeded"
                } else {
                    "runtime_unavailable"
                };
                Err(self.fail(
                    code,
                    Some(id),
                    mutating.then_some(if sent {
                        Outcome::Unknown
                    } else {
                        Outcome::NotApplied
                    }),
                ))
            }
        }
    }
    fn fail(&mut self, code: &'static str, id: Option<u64>, outcome: Option<Outcome>) -> Failure {
        self.closed = true;
        let _ = self.stream.shutdown(Shutdown::Both);
        Failure {
            code,
            id,
            session_id: self.session.clone(),
            outcome,
        }
    }
}
fn time_left(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}
fn read_exact_until(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(time_left(deadline)?))?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn private_directory(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() {
        return false;
    }
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return false;
        }
        current.push(component);
        if !std::fs::symlink_metadata(&current)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            return false;
        }
    }
    // SAFETY: geteuid has no arguments, pointer access or side effects.
    let uid = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(path).is_ok_and(|m| m.uid() == uid && m.mode() & 0o077 == 0)
}
