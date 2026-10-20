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
        let response = client.call(rad_api::Request::Hello {})?;
        let valid = matches!(&response.data, rad_api::ResponseData::Hello(hello)
            if hello.protocol == rad_api::VERSION && Some(hello.session_id.as_str()) == response.session_id.as_deref());
        if response.status != Status::Ok || !valid {
            return Err(client.fail("invalid_request", Some(1), None));
        }
        client.session = response.session_id;
        Ok(client)
    }
    pub fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
        self.call_until(request, Instant::now() + Duration::from_secs(10))
    }
    /// Consume a dedicated connection; ordinary RPC and subscription frames are
    /// never multiplexed through the synchronous response reader.
    pub fn subscribe(mut self) -> Result<Subscription, Failure> {
        let response = self.call(rad_api::Request::Subscribe {})?;
        if response.status != Status::Ok {
            let code = response
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or("invalid_request");
            return Err(self.fail(code, None, None));
        }
        self.stream
            .set_read_timeout(None)
            .map_err(|_| self.fail("runtime_unavailable", None, None))?;
        Ok(Subscription {
            stream: self.stream,
            session: self.session.expect("connected session"),
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
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
        let request = rad_api::Request::parse(operation, arguments)
            .map_err(|_| self.fail("invalid_request", None, None))?;
        self.call_until(request, deadline)
    }
    pub fn call_until(
        &mut self,
        request: rad_api::Request,
        deadline: Instant,
    ) -> Result<Response, Failure> {
        let operation = request.operation().as_str();
        let arguments = request.arguments();
        let operation_code = request.operation();
        let mutating = operation_code.is_mutation();
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
            let response = rad_wire::parse_json(&payload)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
                .and_then(|value| {
                    Response::decode_value(value, id, self.session.as_deref(), operation_code)
                        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
                })
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
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

/// Blocking event source for an editor worker. There is no periodic timeout.
pub struct Subscription {
    stream: UnixStream,
    session: String,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
pub struct SubscriptionCancel(UnixStream, std::sync::Arc<std::sync::atomic::AtomicBool>);
impl SubscriptionCancel {
    pub fn cancel(&self) {
        self.1.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.0.shutdown(Shutdown::Both);
    }
}
impl Subscription {
    pub fn cancellation(&self) -> io::Result<SubscriptionCancel> {
        self.stream
            .try_clone()
            .map(|stream| SubscriptionCancel(stream, self.cancelled.clone()))
    }
    pub fn recv(&mut self) -> Result<rad_api::Notice, Failure> {
        let result = (|| {
            if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let mut header = [0; 4];
            self.stream.read_exact(&mut header)?;
            let size = u32::from_be_bytes(header) as usize;
            if !(1..=4096).contains(&size) {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut bytes = vec![0; size];
            self.stream.read_exact(&mut bytes)?;
            let value = rad_wire::parse_json(&bytes)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            let notice = rad_api::Notice::decode(value)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            if notice.v != rad_api::VERSION
                || notice.session_id != self.session
                || notice.revision.parse::<u64>().is_err()
            {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            Ok(notice)
        })();
        result.map_err(|e: io::Error| {
            let _ = self.stream.shutdown(Shutdown::Both);
            Failure {
                code: if e.kind() == io::ErrorKind::InvalidData {
                    "invalid_request"
                } else {
                    "runtime_unavailable"
                },
                id: None,
                session_id: Some(self.session.clone()),
                outcome: None,
            }
        })
    }
}
