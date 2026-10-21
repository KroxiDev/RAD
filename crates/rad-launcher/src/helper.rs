//! Internal helper entrypoint; not an operation exposed by the authority.
use crate::{LaunchOutcome, LauncherError, monotonic_ms, resolve_before_deadline};
use gio::prelude::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    os::fd::{FromRawFd, OwnedFd},
    rc::Rc,
};
struct State {
    entries: Vec<(String, String)>,
    cursor: usize,
    catalog_done: bool,
    catalog_error: bool,
    jobs: BTreeMap<u64, Option<LaunchOutcome>>,
    writer: bool,
    icons: crate::icons::Cache,
    pending_icon: Option<(String, String)>,
    icon: Option<(String, rad_media::NormalizedImage, bool, usize)>,
}
fn writer(socket: &gio::Socket, context: &glib::MainContext, state: &Rc<RefCell<State>>) {
    if state.borrow().writer {
        return;
    }
    state.borrow_mut().writer = true;
    let state = state.clone();
    let source = SocketExtManual::create_source(
        socket,
        glib::IOCondition::OUT,
        None::<&gio::Cancellable>,
        Some("rad-launcher-output"),
        glib::Priority::DEFAULT,
        move |socket, _| {
            for _ in 0..16 {
                let mut s = state.borrow_mut();
                if s.jobs.is_empty()
                    && s.icon.is_none()
                    && let Some((id, theme)) = s.pending_icon.take()
                {
                    let (image, generic) = s.icons.get(&id, &theme);
                    s.icon = Some((id, image, generic, 0));
                }
                let completion = s
                    .jobs
                    .iter()
                    .find_map(|(id, outcome)| outcome.map(|o| (*id, o)));
                let value = if let Some((id, outcome)) = completion {
                    serde_json::json!({"kind":"completion","token":id,"outcome":match outcome{LaunchOutcome::Accepted=>"accepted",LaunchOutcome::Expired=>"expired",_=>"failed"}})
                } else if let Some((id, image, generic, offset)) = &s.icon {
                    let end = (*offset + 4096).min(image.png_bytes().len());
                    serde_json::json!({"kind":"icon","id":id,"generic":generic,"data":crate::hex(&image.png_bytes()[*offset..end]),"last":end==image.png_bytes().len()})
                } else if !s.catalog_done {
                    if s.catalog_error {
                        serde_json::json!({"kind":"catalog_error"})
                    } else if let Some((id, name)) = s.entries.get(s.cursor) {
                        serde_json::json!({"kind":"entry","id":id,"name":name})
                    } else {
                        serde_json::json!({"kind":"catalog_end"})
                    }
                } else {
                    s.writer = false;
                    return glib::ControlFlow::Break;
                };
                let bytes = serde_json::to_vec(&value).unwrap();
                match socket.send(&bytes, None::<&gio::Cancellable>) {
                    Ok(n) if n == bytes.len() => {
                        if let Some((id, _)) = completion {
                            s.jobs.remove(&id);
                        } else if let Some((_, image, _, offset)) = &mut s.icon {
                            *offset = (*offset + 4096).min(image.png_bytes().len());
                            if *offset == image.png_bytes().len() {
                                s.icon = None;
                            }
                        } else if s.cursor == s.entries.len() {
                            s.catalog_done = true;
                        } else {
                            s.cursor += 1;
                        }
                    }
                    Err(e) if e.matches(gio::IOErrorEnum::WouldBlock) => {
                        return glib::ControlFlow::Continue;
                    }
                    _ => {
                        s.writer = false;
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        },
    );
    source.attach(Some(context));
}
fn catalog() -> (Vec<(String, String)>, bool) {
    let mut entries = Vec::new();
    let mut retained = 0usize;
    // GIO materializes its native enumeration; Rust metadata is checked before retention.
    for app in gio::AppInfo::all() {
        if !app.should_show() {
            continue;
        }
        let Some(id) = app.id() else { continue };
        if rad_core::configuration::DesktopEntry::new(id.to_string()).is_err() {
            continue;
        }
        let name = app.display_name();
        let cost = id.len() + name.len();
        if entries.len() == 4096
            || retained.saturating_add(cost) > 4194304
            || serde_json::to_vec(
                &serde_json::json!({"kind":"entry","id":id.as_str(),"name":name.as_str()}),
            )
            .map_or(true, |b| b.len() > 16384)
        {
            return (Vec::new(), true);
        }
        retained += cost;
        entries.push((id.to_string(), name.to_string()));
    }
    entries.sort();
    (entries, false)
}
/// Own the private inherited descriptor. No environment/path IPC endpoints.
pub fn run() -> Result<(), LauncherError> {
    if std::env::args().nth(1).as_deref() != Some("--channel-fd=3") {
        return Err(LauncherError::Protocol);
    }
    // Before anything is launched: applications must inherit /dev/null as stderr.
    crate::scope::isolate_stderr().map_err(|_| LauncherError::Io)?;
    // SAFETY: validate fd and prevent inheritance by launched applications.
    if unsafe { libc::fcntl(3, libc::F_GETFD) } < 0
        || unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
    {
        return Err(LauncherError::Io);
    }
    // SAFETY: invocation transfers the unique validated inherited descriptor3.
    let fd = unsafe { OwnedFd::from_raw_fd(3) };
    let socket = gio::Socket::from_fd(fd).map_err(|_| LauncherError::Io)?;
    socket.set_blocking(false);
    let context = glib::MainContext::new();
    let mainloop = glib::MainLoop::new(Some(&context), false);
    context
        .with_thread_default(|| {
            let (entries, catalog_error) = catalog();
            let state = Rc::new(RefCell::new(State {
                entries,
                cursor: 0,
                catalog_done: false,
                catalog_error,
                jobs: BTreeMap::new(),
                writer: false,
                icons: crate::icons::Cache::default(),
                pending_icon: None,
                icon: None,
            }));
            writer(&socket, &context, &state);
            let read_context = context.clone();
            let launch_context = gio::AppLaunchContext::new();
            crate::scope::watch(&launch_context, &context);
            let read_loop = mainloop.clone();
            let reader = SocketExtManual::create_source(
                &socket,
                glib::IOCondition::IN,
                None::<&gio::Cancellable>,
                Some("rad-launcher-input"),
                glib::Priority::DEFAULT,
                move |socket, _| {
                    for _ in 0..32 {
                        let mut bytes = [0u8; 16385];
                        let n = match socket.receive(&mut bytes, None::<&gio::Cancellable>) {
                            Ok(n) if n <= 16384 => n,
                            Err(e) if e.matches(gio::IOErrorEnum::WouldBlock) => break,
                            _ => {
                                read_loop.quit();
                                return glib::ControlFlow::Break;
                            }
                        };
                        let request = serde_json::from_slice::<serde_json::Value>(&bytes[..n]).ok();
                        let request = request.as_ref();
                        if request.is_some_and(|v| v["kind"] == "icon") {
                            let values = request
                                .and_then(|v| Some((v["entry"].as_str()?, v["theme"].as_str()?)));
                            let Some((id, theme)) = values else {
                                read_loop.quit();
                                return glib::ControlFlow::Break;
                            };
                            if !state.borrow().entries.iter().any(|e| e.0 == id)
                                || !crate::icons::valid_name(theme)
                                || state.borrow().pending_icon.is_some()
                                || state.borrow().icon.is_some()
                            {
                                read_loop.quit();
                                return glib::ControlFlow::Break;
                            }
                            state.borrow_mut().pending_icon = Some((id.into(), theme.into()));
                            writer(socket, &read_context, &state);
                            continue;
                        }
                        if request.is_some_and(|v| v["kind"] == "refresh") {
                            if !state.borrow().jobs.is_empty() {
                                read_loop.quit();
                                return glib::ControlFlow::Break;
                            }
                            let (entries, error) = catalog();
                            {
                                let mut s = state.borrow_mut();
                                s.entries = entries;
                                s.icons = crate::icons::Cache::default();
                                s.catalog_error = error;
                                s.cursor = 0;
                                s.catalog_done = false;
                            }
                            writer(socket, &read_context, &state);
                            continue;
                        }
                        let parsed = request.and_then(|v| {
                            Some((
                                v["token"].as_u64()?,
                                v["deadline"].as_u64()?,
                                v["entry"].as_str()?,
                            ))
                        });
                        let Some((token, deadline, entry)) = parsed else {
                            read_loop.quit();
                            return glib::ControlFlow::Break;
                        };
                        if state.borrow().jobs.len() >= 32
                            || state.borrow().jobs.contains_key(&token)
                        {
                            read_loop.quit();
                            return glib::ControlFlow::Break;
                        }
                        state.borrow_mut().jobs.insert(token, None);
                        let entry = rad_core::configuration::DesktopEntry::new(entry.into());
                        let app = resolve_before_deadline(deadline, monotonic_ms, || {
                            entry
                                .ok()
                                .and_then(|e| gio_unix::DesktopAppInfo::new(e.id()))
                        });
                        match app {
                            Err(outcome) => {
                                state.borrow_mut().jobs.insert(token, Some(outcome));
                                writer(socket, &read_context, &state);
                            }
                            Ok(app) => {
                                let callback_state = state.clone();
                                let callback_socket = socket.clone();
                                let callback_context = read_context.clone();
                                let keep_app = app.clone();
                                let keep_context = launch_context.clone();
                                // Recheck immediately before effect, after all synchronous resolution/setup.
                                if !monotonic_ms().is_ok_and(|now| now < deadline) {
                                    state
                                        .borrow_mut()
                                        .jobs
                                        .insert(token, Some(LaunchOutcome::Expired));
                                    writer(socket, &read_context, &state);
                                    continue;
                                }
                                app.launch_uris_async(
                                    &[],
                                    Some(&launch_context),
                                    None::<&gio::Cancellable>,
                                    move |result| {
                                        let _keep_alive = (keep_app, keep_context);
                                        callback_state.borrow_mut().jobs.insert(
                                            token,
                                            Some(if result.is_ok() {
                                                LaunchOutcome::Accepted
                                            } else {
                                                LaunchOutcome::Failed
                                            }),
                                        );
                                        writer(
                                            &callback_socket,
                                            &callback_context,
                                            &callback_state,
                                        );
                                    },
                                );
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                },
            );
            reader.attach(Some(&context));
            mainloop.run();
        })
        .map_err(|_| LauncherError::Io)?;
    Ok(())
}
