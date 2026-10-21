use crate::{CatalogEntry, LauncherError};
use std::{
    io,
    os::{
        fd::AsRawFd,
        unix::{net::UnixDatagram, process::CommandExt},
    },
    process::{Child, Command, Stdio},
};
const MAX_PACKET: usize = 16384;
static LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
use std::sync::atomic::Ordering;
struct IconRequest {
    id: rad_core::configuration::DesktopEntry,
    packet: Option<Vec<u8>>,
    bytes: Vec<u8>,
    preview: Option<crate::IconPreview>,
}
struct Job {
    deadline: u64,
    id: rad_core::execution::DispatchId,
    packet: Option<Vec<u8>>,
    completion: Option<crate::LaunchOutcome>,
}
pub struct Launcher {
    socket: UnixDatagram,
    child: Option<Child>,
    catalog: Vec<CatalogEntry>,
    ready: bool,
    jobs: std::collections::BTreeMap<u64, Job>,
    next: u64,
    failure: Option<LauncherError>,
    building: Vec<CatalogEntry>,
    refresh_requested: bool,
    refreshing: bool,
    stale: bool,
    icon: Option<IconRequest>,
}
impl Launcher {
    /// Trusted runtime bootstrap, outside the authority actor; never client-selected.
    pub fn spawn(command: Command) -> Result<Self, LauncherError> {
        if LIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(LauncherError::Unavailable);
        }
        let result = Self::spawn_inner(command);
        if result.is_err() {
            LIVE.store(false, Ordering::Release);
        }
        result
    }
    fn spawn_inner(mut command: Command) -> Result<Self, LauncherError> {
        let (socket, child_socket) = UnixDatagram::pair().map_err(|_| LauncherError::Io)?;
        socket
            .set_nonblocking(true)
            .map_err(|_| LauncherError::Io)?;
        child_socket
            .set_nonblocking(true)
            .map_err(|_| LauncherError::Io)?;
        let fd = child_socket.as_raw_fd();
        let parent = std::process::id();
        command
            .arg("--channel-fd=3")
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        // stderr is the caller's choice (the runtime's by default): the helper
        // keeps it for its own diagnostics and gives applications /dev/null.
        // SAFETY: child-only async-signal-safe fd syscalls; socket is held through spawn.
        unsafe {
            command.pre_exec(move || {
                // Linux per-process death signal; forked applications do not inherit it.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0
                    || libc::getppid() as u32 != parent
                {
                    return Err(io::Error::last_os_error());
                }
                if fd != 3 && libc::dup2(fd, 3) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|_| LauncherError::Io)?;
        Ok(Self {
            socket,
            child: Some(child),
            catalog: Vec::new(),
            ready: false,
            jobs: std::collections::BTreeMap::new(),
            next: 1,
            failure: None,
            building: Vec::new(),
            refresh_requested: false,
            refreshing: true,
            stale: true,
            icon: None,
        })
    }
    pub fn pump(&mut self) -> Result<(), LauncherError> {
        let result = self.pump_inner();
        if let Err(error) = result
            && error != LauncherError::Capacity
        {
            self.failure = Some(error);
            self.ready = false;
        }
        result
    }
    fn pump_inner(&mut self) -> Result<(), LauncherError> {
        if self
            .child
            .as_mut()
            .map(|c| c.try_wait())
            .transpose()
            .map_err(|_| LauncherError::Io)?
            .flatten()
            .is_some()
        {
            self.child = None;
            LIVE.store(false, Ordering::Release);
            self.ready = false;
            for job in self.jobs.values_mut() {
                if job.completion.is_none() {
                    job.completion = Some(crate::LaunchOutcome::HelperLost);
                }
                job.packet = None;
            }
            return Ok(());
        }
        if self.child.is_none() {
            return Ok(());
        }
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.refresh_requested && self.jobs.is_empty() && self.icon.is_none() && !self.refreshing
        {
            match self.socket.send(br#"{"kind":"refresh"}"#) {
                Ok(_) => {
                    self.refresh_requested = false;
                    self.refreshing = true;
                    self.building.clear();
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(LauncherError::Io),
            }
        }
        for job in self.jobs.values_mut() {
            if let Some(packet) = &job.packet {
                match self.socket.send(packet) {
                    Ok(n) if n == packet.len() => job.packet = None,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        break;
                    }
                    _ => return Err(LauncherError::Io),
                }
            }
        }
        if let Some(icon) = &mut self.icon
            && let Some(packet) = &icon.packet
        {
            match self.socket.send(packet) {
                Ok(n) if n == packet.len() => icon.packet = None,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                _ => return Err(LauncherError::Io),
            }
        }
        for _ in 0..32 {
            let mut bytes = [0u8; MAX_PACKET + 1];
            let n = match self.socket.recv(&mut bytes) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(LauncherError::Io),
            };
            if n > MAX_PACKET {
                return Err(LauncherError::Protocol);
            }
            let v: serde_json::Value =
                serde_json::from_slice(&bytes[..n]).map_err(|_| LauncherError::Protocol)?;
            match v["kind"].as_str() {
                Some("catalog_error") => {
                    self.building.clear();
                    self.refreshing = false;
                    self.stale = true;
                    if !self.ready {
                        self.failure = Some(LauncherError::Capacity);
                    }
                    return Err(LauncherError::Capacity);
                }
                Some("entry") => {
                    if !self.refreshing || self.building.len() >= 4096 {
                        return Err(LauncherError::Protocol);
                    }
                    let id = v["id"].as_str().ok_or(LauncherError::Protocol)?;
                    let name = v["name"].as_str().ok_or(LauncherError::Protocol)?;
                    if self
                        .building
                        .iter()
                        .map(|e| e.id.id().len() + e.name.len())
                        .sum::<usize>()
                        + id.len()
                        + name.len()
                        > 4194304
                        || self.building.iter().any(|e| e.id.id() == id)
                    {
                        return Err(LauncherError::Protocol);
                    }
                    self.building.push(CatalogEntry {
                        id: rad_core::configuration::DesktopEntry::new(
                            v["id"].as_str().ok_or(LauncherError::Protocol)?.into(),
                        )
                        .map_err(|_| LauncherError::Protocol)?,
                        name: v["name"].as_str().ok_or(LauncherError::Protocol)?.into(),
                    });
                }
                Some("catalog_end") => {
                    if !self.refreshing {
                        return Err(LauncherError::Protocol);
                    }
                    self.catalog = std::mem::take(&mut self.building);
                    self.ready = true;
                    self.refreshing = false;
                    self.stale = self.refresh_requested;
                }
                Some("icon") => {
                    let icon = self.icon.as_mut().ok_or(LauncherError::Protocol)?;
                    if icon.preview.is_some() || v["id"].as_str() != Some(icon.id.id()) {
                        return Err(LauncherError::Protocol);
                    }
                    let bytes = crate::unhex(v["data"].as_str().ok_or(LauncherError::Protocol)?)
                        .ok_or(LauncherError::Protocol)?;
                    if icon.bytes.len() + bytes.len() > 512 * 1024 {
                        return Err(LauncherError::Capacity);
                    }
                    icon.bytes.extend(bytes);
                    if v["last"].as_bool().ok_or(LauncherError::Protocol)? {
                        icon.preview = Some(crate::IconPreview {
                            id: icon.id.clone(),
                            image: rad_media::NormalizedImage::validate(&icon.bytes)
                                .map_err(|_| LauncherError::Protocol)?,
                            generic: v["generic"].as_bool().ok_or(LauncherError::Protocol)?,
                        });
                        icon.bytes.clear();
                    }
                }
                Some("completion") => {
                    let token = v["token"].as_u64().ok_or(LauncherError::Protocol)?;
                    let job = self.jobs.get_mut(&token).ok_or(LauncherError::Protocol)?;
                    if job.completion.is_some() {
                        return Err(LauncherError::Protocol);
                    }
                    job.completion = Some(match v["outcome"].as_str() {
                        Some("accepted") => crate::LaunchOutcome::Accepted,
                        Some("expired") => crate::LaunchOutcome::Expired,
                        Some("failed") => crate::LaunchOutcome::Failed,
                        _ => return Err(LauncherError::Protocol),
                    });
                }
                _ => return Err(LauncherError::Protocol),
            }
        }
        Ok(())
    }
    pub fn catalog(&self) -> Option<&[CatalogEntry]> {
        self.ready.then_some(&self.catalog)
    }
}
impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // Shutdown/reaping is outside authority; never wait on this caller thread.
            let _ = std::thread::Builder::new()
                .name("rad-launcher-reap".into())
                .spawn(move || {
                    if child.wait().is_ok() {
                        LIVE.store(false, Ordering::Release);
                    }
                });
        }
    }
}
impl Launcher {
    pub fn submit(
        &mut self,
        intent: &rad_core::execution::DispatchIntent,
    ) -> Result<(), LauncherError> {
        if !self.is_available() {
            return Err(LauncherError::Unavailable);
        }
        let rad_core::execution::Operation::Launcher(entry) = &intent.operation else {
            return Err(LauncherError::InvalidRequest);
        };
        if self.jobs.len() >= 32 {
            return Err(LauncherError::Capacity);
        }
        if self.jobs.values().any(|j| j.id == intent.id) {
            return Err(LauncherError::InvalidRequest);
        }
        let token = self.next;
        let next = token.checked_add(1).ok_or(LauncherError::Capacity)?;
        let bytes=serde_json::to_vec(&serde_json::json!({"kind":"launch","token":token,"entry":entry.id(),"deadline":intent.deadline_ms})).map_err(|_|LauncherError::Protocol)?;
        if bytes.len() > MAX_PACKET {
            return Err(LauncherError::Capacity);
        }
        self.jobs.insert(
            token,
            Job {
                deadline: intent.deadline_ms,
                id: intent.id,
                packet: Some(bytes),
                completion: None,
            },
        );
        self.next = next;
        Ok(())
    }
    pub fn take_completion(
        &mut self,
    ) -> Option<(rad_core::execution::DispatchId, crate::LaunchOutcome)> {
        let token = *self.jobs.iter().find(|(_, j)| j.completion.is_some())?.0;
        let job = self.jobs.remove(&token)?;
        Some((job.id, job.completion?))
    }
}
impl Launcher {
    /// Request termination of ONLY the helper; pump must observe reap before release.
    pub fn shutdown(&mut self) -> Result<(), LauncherError> {
        self.ready = false;
        self.child
            .as_mut()
            .ok_or(LauncherError::Unavailable)?
            .kill()
            .map_err(|_| LauncherError::Io)
    }
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn outstanding(&self) -> usize {
        self.jobs.len()
    }
}
impl Launcher {
    pub fn request_refresh(&mut self) -> Result<(), LauncherError> {
        if self.child.is_none() {
            return Err(LauncherError::Unavailable);
        }
        self.refresh_requested = true;
        self.stale = true;
        self.failure = None;
        Ok(())
    }
    pub fn catalog_stale(&self) -> bool {
        self.stale
    }
}
impl Launcher {
    pub fn request_icon(
        &mut self,
        entry: &rad_core::configuration::DesktopEntry,
        theme: &str,
    ) -> Result<(), LauncherError> {
        if !self.ready || self.refreshing || !self.jobs.is_empty() {
            return Err(LauncherError::Unavailable);
        }
        if self.icon.is_some() {
            return Err(LauncherError::Capacity);
        }
        if !self.catalog.iter().any(|e| e.id == *entry)
            || !crate::icons::valid_name(theme)
            || entry.id().len() > 4096
        {
            return Err(LauncherError::InvalidRequest);
        }
        let packet = serde_json::to_vec(
            &serde_json::json!({"kind":"icon","entry":entry.id(),"theme":theme}),
        )
        .map_err(|_| LauncherError::Protocol)?;
        self.icon = Some(IconRequest {
            id: entry.clone(),
            packet: Some(packet),
            bytes: Vec::new(),
            preview: None,
        });
        Ok(())
    }
    pub fn take_icon(&mut self) -> Option<crate::IconPreview> {
        self.icon.as_ref()?.preview.as_ref()?;
        self.icon.take()?.preview
    }
}

impl Launcher {
    pub fn can_spawn() -> bool {
        !LIVE.load(Ordering::Acquire)
    }
}
impl Launcher {
    pub fn is_available(&self) -> bool {
        self.ready
            && !self.refreshing
            && self.failure.is_none()
            && crate::monotonic_ms().is_ok_and(|now| {
                !self
                    .jobs
                    .values()
                    .any(|job| job.completion.is_none() && job.deadline <= now)
            })
    }
}

impl std::os::fd::AsFd for Launcher {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.socket.as_fd()
    }
}
impl Launcher {
    /// Register writable interest only while an intent/control packet is pending.
    pub fn needs_write(&self) -> bool {
        self.jobs.values().any(|j| j.packet.is_some())
            || self.icon.as_ref().is_some_and(|i| i.packet.is_some())
            || (self.refresh_requested
                && !self.refreshing
                && self.jobs.is_empty()
                && self.icon.is_none())
    }
}
