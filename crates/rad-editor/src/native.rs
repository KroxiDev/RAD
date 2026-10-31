//! The sole owner of blocking socket, transfer and theme IO; off the GUI thread.
use crate::{
    Request,
    backups::ExportFile,
    icons::Icons,
    session::{Board, Error, Inspection, Refresh, Selected, Session},
    theme::Palette,
    transfer::{self, Download, Progress, Upload},
    worker::Worker,
};
use rad_client::{
    connection::{Client, Endpoint},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    fs::File,
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Image,
    Restore,
    Export,
}
#[derive(Debug)]
pub enum Job {
    PluginApply(Request),
    PluginCatalog,
    Plugins,
    PluginForm {
        key: String,
        entity: Option<String>,
        target: crate::plugins::FormTarget,
    },
    PluginVisuals {
        ids: Vec<String>,
        revision: String,
    },
    Connect,
    Refresh {
        board: bool,
        overview: bool,
        /// Read with a board of a newer revision, so both change together.
        selected: Option<Selected>,
    },
    Inspect(String),
    /// A key's inspector read before it is selected.
    InspectAhead(String),
    /// A folder one press away from the one on show.
    FolderBoard(String),
    Request(Request),
    SaveIcon {
        button: String,
        ticket: u64,
        png: std::sync::Arc<Vec<u8>>,
        request: Request,
        /// What drew `png`, remembered on this computer once the image is stored.
        choice: crate::icon_library::Choice,
    },
    Theme,
    Transfer {
        path: PathBuf,
        kind: TransferKind,
    },
    /// Export to the private default backup folder, resolved on this worker.
    ExportBackup,
    TransferStep,
    /// Key pictures, read in the order given.
    Pictures(Vec<PictureRead>),
    /// Give `button` the icon of the application its top action opens, if it has no image.
    /// With `application`, only when that desktop entry is the one on top.
    AppIcon {
        button: String,
        application: Option<String>,
    },
}
pub enum Reply {
    PluginApplied(Response),
    PluginCatalog(Vec<crate::plugins::CatalogEntry>),
    Plugins {
        catalog: Vec<crate::plugins::CatalogEntry>,
        status: Value,
    },
    PluginForm {
        target: crate::plugins::FormTarget,
        loaded: crate::plugins::LoadedForm,
    },
    PluginVisuals {
        revision: String,
        visuals: Vec<crate::plugins::Visual>,
    },
    IconSaved {
        button: String,
        ticket: u64,
        result: Result<Response, Error>,
    },
    Connected,
    /// The runtime is in recovery mode and refused a request that is not about plugins.
    RecoveryRequired,
    Refresh(Refresh),
    Inspection(Inspection),
    InspectedAhead(Inspection),
    Response {
        operation: String,
        response: Response,
    },
    Theme,
    /// Each picture asked for, with its pixels unless it could not be read.
    Pictures(Vec<(String, Option<crate::preview::Pixels>)>),
    FolderBoard(Board),
    TransferProgress(u64),
    TransferFinished {
        kind: TransferKind,
        data: Value,
        path: Option<PathBuf>,
    },
    /// The key keeps its look: it has an image, or its application ships no usable icon.
    NoAppIcon,
}
/// The picture of a key: the key, its media reference and the revision it was seen at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureRead {
    pub button: String,
    pub reference: String,
    pub revision: String,
}
/// Reads pictures one after another. One that cannot be read is reported missing; a lost
/// connection fails the whole read.
fn read_pictures<P: crate::session::Port>(
    pictures: Vec<PictureRead>,
    s: &mut Session<P>,
) -> Result<Reply, Error> {
    let mut read = Vec::with_capacity(pictures.len());
    for picture in pictures {
        let pixels = s
            .media_bytes(&picture.button, &picture.revision)
            .and_then(|bytes| crate::preview::decode(&bytes));
        match pixels {
            Ok(pixels) => read.push((picture.reference, Some(pixels))),
            Err(error) if !s.is_open() => return Err(error),
            Err(_) => read.push((picture.reference, None)),
        }
    }
    Ok(Reply::Pictures(read))
}
/// One question to the runtime about the options of a plugin field.
#[derive(Debug)]
pub struct OptionsJob {
    pub ticket: u64,
    pub request: Request,
}
pub struct OptionsEvent {
    pub ticket: u64,
    /// The `data` of an `ok` answer.
    pub result: Result<Value, Error>,
}
pub struct Event {
    pub plugin_apply: bool,
    pub plugin_admin: bool,
    pub plugin_manifest: bool,
    pub plugin_dependencies: bool,
    pub plugin_form: bool,
    pub plugin_visual: bool,
    /// A read ahead or a picture: its failure is not the user's business.
    pub background: bool,
    pub connected: bool,
    pub transfer_job: bool,
    pub result: Result<Reply, Error>,
    pub palette: Palette,
}
enum Transfer {
    Upload(TransferKind, Upload<File>),
    Download(Download<ExportFile>),
}
fn begin_export(writer: ExportFile, s: &mut Session<Client>) -> Result<Transfer, Error> {
    let data = s
        .request(crate::Request::TransferBegin {
            kind: rad_api::TransferKind::Export,
            size: None,
        })?
        .data;
    let rad_api::ResponseData::Transfer(data) = data else {
        return Err(Error::protocol());
    };
    Ok(Transfer::Download(Download::new(writer, data.transfer_id)))
}
fn begin(path: PathBuf, kind: TransferKind, s: &mut Session<Client>) -> Result<Transfer, Error> {
    if kind == TransferKind::Export {
        let writer = ExportFile::at(path).map_err(|_| transfer::io_error())?;
        begin_export(writer, s)
    } else {
        upload(path, kind, s).map(|upload| Transfer::Upload(kind, upload))
    }
}
fn automatic_backup(s: &mut Session<Client>) -> Result<Transfer, Error> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(transfer::io_error)?;
    let writer = ExportFile::automatic(&home, std::time::SystemTime::now())
        .map_err(|_| transfer::io_error())?;
    begin_export(writer, s)
}
/// Opens `path` and announces its upload; the runtime validates what arrives.
fn upload(
    path: PathBuf,
    kind: TransferKind,
    s: &mut Session<Client>,
) -> Result<Upload<File>, Error> {
    let file = File::open(path).map_err(|_| transfer::io_error())?;
    let meta = file.metadata().map_err(|_| transfer::io_error())?;
    let cap = if kind == TransferKind::Image {
        transfer::IMAGE_CAP
    } else {
        transfer::PACKAGE_CAP
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > cap {
        return Err(transfer::io_error());
    }
    let data = s
        .request(crate::Request::TransferBegin {
            kind: match kind {
                TransferKind::Image => rad_api::TransferKind::ImageImport,
                TransferKind::Restore => rad_api::TransferKind::Restore,
                TransferKind::Export => rad_api::TransferKind::Export,
            },
            size: Some(meta.len().to_string()),
        })?
        .data;
    let rad_api::ResponseData::Transfer(data) = data else {
        return Err(Error::protocol());
    };
    let id = data.transfer_id;
    Ok(Upload::new(file, id, meta.len()))
}
/// Imports the icon of the application on top of `button` and sets it as the key's image.
/// An icon the runtime will not take leaves the key as it was: the icon is a courtesy.
fn app_icon(
    button: &str,
    application: Option<&str>,
    icons: &Icons,
    s: &mut Session<Client>,
) -> Result<Reply, Error> {
    let inspection = s.inspect(button)?;
    let path = crate::icons::wanted(&inspection)
        .filter(|top| application.is_none_or(|expected| expected == *top))
        .and_then(|top| icons.find(top));
    let Some(path) = path else {
        return Ok(Reply::NoAppIcon);
    };
    let imported = upload(path, TransferKind::Image, s).and_then(|mut upload| {
        loop {
            if let Progress::Finished(data) = upload.step(s)? {
                return Ok(data);
            }
        }
    });
    let image = match imported {
        Ok(data) if data["media_ref"].is_string() => data["media_ref"].clone(),
        Ok(_) => return Err(Error::protocol()),
        Err(_) if s.is_open() => return Ok(Reply::NoAppIcon),
        Err(error) => return Err(error),
    };
    let appearance = json!({"text":inspection.appearance["text"],"image":image});
    s.request(crate::Request::Edit {
        changes: vec![rad_api::Change::Set {
            id: button.into(),
            edit: rad_api::FieldEdit::Appearance(
                serde_json::from_value(appearance).map_err(|_| Error::protocol())?,
            ),
        }],
    })
    .map(|response| Reply::Response {
        operation: "edit".into(),
        response,
    })
}
fn step(active: &mut Option<Transfer>, s: &mut Session<Client>) -> Result<Reply, Error> {
    let started = Instant::now();
    let mut offset = 0;
    for _ in 0..64 {
        let transfer = active.as_mut().ok_or_else(Error::protocol)?;
        let result = match transfer {
            Transfer::Upload(_, u) => u.step(s),
            Transfer::Download(d) => d.step(s),
        };
        match result {
            Err(e) => {
                *active = None;
                return Err(e);
            }
            Ok(Progress::Chunk(n)) => offset = n,
            Ok(Progress::Finished(data)) => {
                let transfer = active.take().unwrap();
                let (kind, path) = match transfer {
                    Transfer::Upload(kind, _) => (kind, None),
                    Transfer::Download(download) => {
                        let path = download
                            .into_inner()
                            .finish()
                            .map_err(|_| transfer::io_error())?;
                        (TransferKind::Export, Some(path))
                    }
                };
                return Ok(Reply::TransferFinished { kind, data, path });
            }
        }
        if started.elapsed() >= Duration::from_millis(20) {
            break;
        }
    }
    Ok(Reply::TransferProgress(offset))
}
pub fn start(runtime: PathBuf, theme: PathBuf) -> Worker<Job, Event> {
    start_with_clock(runtime, theme, Instant::now)
}
/// Replace an idle RPC connection only when admitting new work; never replay a request.
pub fn start_with_clock(
    runtime: PathBuf,
    theme: PathBuf,
    mut now: impl FnMut() -> Instant + Send + 'static,
) -> Worker<Job, Event> {
    let mut session: Option<Session<Client>> = None;
    let mut last_rpc = now();
    let mut palette = Palette::default();
    let mut active: Option<Transfer> = None;
    let icons = Icons::from_environment();
    let mut visual_epoch = String::new();
    let mut visual_revisions = std::collections::BTreeMap::new();
    Worker::spawn(move |job| {
        let plugin_apply = matches!(&job, Job::PluginApply(_));
        let plugin_admin = matches!(&job,Job::Request(r) if r.operation().as_str()=="plugin_admin");
        let plugin_manifest = matches!(&job, Job::Request(Request::PluginManifest { .. }));
        let plugin_dependencies =
            matches!(&job,Job::Request(r) if r.operation().as_str()=="plugin_dependencies");
        let plugin_form = matches!(&job, Job::PluginForm { .. });
        let plugin_visual = matches!(&job, Job::PluginVisuals { .. });
        let background = matches!(
            &job,
            Job::InspectAhead(_) | Job::FolderBoard(_) | Job::Pictures(_)
        );
        let transfer_job = matches!(
            job,
            Job::Transfer { .. } | Job::ExportBackup | Job::TransferStep
        );
        // Only the plugin manager also answers `recovery_required`, so any other job
        // refused with it was refused by the runtime's recovery mode.
        let recovery_aware = match &job {
            Job::Request(request) => !request.operation().as_str().starts_with("plugin"),
            Job::Refresh { .. }
            | Job::Inspect(_)
            | Job::InspectAhead(_)
            | Job::FolderBoard(_)
            | Job::Pictures(_)
            | Job::AppIcon { .. }
            | Job::Transfer { .. }
            | Job::ExportBackup
            | Job::TransferStep => true,
            _ => false,
        };
        palette.read(&theme);
        // Transfers belong to their original session and must never be resumed on a new one.
        // Local theme work neither renews nor keeps the RPC alive.
        let uses_rpc = !matches!(job, Job::Theme);
        let ready = if uses_rpc
            && !matches!(job, Job::Connect)
            && active.is_none()
            && session.is_some()
            && now().saturating_duration_since(last_rpc) >= Duration::from_secs(30)
        {
            session = None; // release the old slot before opening its replacement
            Client::connect(&runtime, Endpoint::Editor)
                .map(|client| {
                    session = Some(Session::new(client));
                })
                .map_err(|e| Error {
                    code: e.code.into(),
                    cause: None,
                    source_change: None,
                    unknown: false,
                })
        } else {
            Ok(())
        };
        if uses_rpc {
            last_rpc = now();
        }
        let result = ready.and_then(|()| match job {
            Job::Theme => Ok(Reply::Theme),
            Job::Connect => {
                active = None;
                // A disconnected UI discarded its textures, even if runtime revisions survive.
                visual_epoch.clear();
                visual_revisions.clear();
                match Client::connect(&runtime, Endpoint::Editor) {
                    Ok(client) => {
                        session = Some(Session::new(client));
                        Ok(Reply::Connected)
                    }
                    Err(e) => {
                        session = None;
                        Err(Error {
                            code: e.code.into(),
                            cause: None,
                            source_change: None,
                            unknown: false,
                        })
                    }
                }
            }
            other => match session.as_mut() {
                None => Err(Error {
                    code: "runtime_unavailable".into(),
                    cause: None,
                    source_change: None,
                    unknown: false,
                }),
                Some(s) => match other {
                    Job::PluginApply(request) => s.request(request).map(Reply::PluginApplied),
                    Job::PluginCatalog => s.plugin_catalog().map(Reply::PluginCatalog),
                    Job::Plugins => s.plugin_catalog().and_then(|catalog| {
                        s.request(crate::Request::PluginStatus {})
                            .map(|r| Reply::Plugins {
                                catalog,
                                status: serde_json::to_value(r.data).expect("typed plugin status"),
                            })
                    }),
                    Job::PluginForm {
                        key,
                        entity,
                        target,
                    } => {
                        let allowed = if let crate::plugins::FormTarget::AddAction(id) = &target {
                            s.inspect(id).and_then(|v| {
                                if crate::plugins::accepts_action(&v.actions) {
                                    Ok(())
                                } else {
                                    Err(Error {
                                        code: "operation_not_allowed".into(),
                                        cause: None,
                                        source_change: None,
                                        unknown: false,
                                    })
                                }
                            })
                        } else {
                            Ok(())
                        };
                        allowed
                            .and_then(|()| s.plugin_form(&key, entity.as_deref()))
                            .map(|loaded| Reply::PluginForm { target, loaded })
                    }
                    Job::PluginVisuals { ids, revision } => {
                        if visual_epoch != revision {
                            visual_revisions.clear();
                            visual_epoch = revision.clone();
                        }
                        visual_revisions.retain(|id, _| ids.contains(id));
                        s.plugin_visuals_isolated(&ids, &visual_revisions)
                            .map(|visuals| {
                                for v in &visuals {
                                    if v.has_image && v.availability != "unavailable" {
                                        visual_revisions.insert(v.id.clone(), v.revision.clone());
                                    } else {
                                        visual_revisions.remove(&v.id);
                                    }
                                }
                                Reply::PluginVisuals { revision, visuals }
                            })
                    }
                    Job::Refresh {
                        board,
                        overview,
                        selected,
                    } => s
                        .refresh_views(board, overview, selected.as_ref())
                        .map(Reply::Refresh),
                    Job::Inspect(id) => s.inspect(&id).map(Reply::Inspection),
                    Job::InspectAhead(id) => s.inspect(&id).map(Reply::InspectedAhead),
                    Job::FolderBoard(folder) => s.folder_board(&folder).map(Reply::FolderBoard),
                    Job::Pictures(pictures) => read_pictures(pictures, s),
                    Job::SaveIcon {
                        button,
                        ticket,
                        png,
                        request,
                        choice,
                    } => {
                        let result = if active.is_some() {
                            Err(Error {
                                code: "busy".into(),
                                cause: None,
                                source_change: None,
                                unknown: false,
                            })
                        } else {
                            crate::icon_save::save(s, &button, &png, request)
                        };
                        // A convenience for reopening the library: failing to remember
                        // never changes the outcome of the save.
                        if let Ok(saved) = &result
                            && let Some(path) = crate::icon_memory::default_path()
                            && let Err(error) =
                                crate::icon_memory::remember(&path, &saved.image, choice)
                        {
                            eprintln!("RAD icon choice not remembered: {error}");
                        }
                        Ok(Reply::IconSaved {
                            button,
                            ticket,
                            result: result.map(|saved| saved.response),
                        })
                    }
                    Job::Request(request) => {
                        let operation = request.operation().as_str().to_owned();
                        s.request(request).map(|response| Reply::Response {
                            operation,
                            response,
                        })
                    }
                    Job::Transfer { path, kind } => {
                        if active.is_some() {
                            Err(Error {
                                code: "busy".into(),
                                cause: None,
                                source_change: None,
                                unknown: false,
                            })
                        } else {
                            begin(path, kind, s).map(|t| {
                                active = Some(t);
                                Reply::TransferProgress(0)
                            })
                        }
                    }
                    Job::ExportBackup => {
                        if active.is_some() {
                            Err(Error {
                                code: "busy".into(),
                                cause: None,
                                source_change: None,
                                unknown: false,
                            })
                        } else {
                            automatic_backup(s).map(|transfer| {
                                active = Some(transfer);
                                Reply::TransferProgress(0)
                            })
                        }
                    }
                    Job::TransferStep => step(&mut active, s),
                    // An import under way owns the transfer; the icon can be asked for later.
                    Job::AppIcon { .. } if active.is_some() => Ok(Reply::NoAppIcon),
                    Job::AppIcon {
                        button,
                        application,
                    } => app_icon(&button, application.as_deref(), &icons, s),
                    _ => unreachable!(),
                },
            },
        });
        let result = match result {
            Err(error) if recovery_aware && error.code == "recovery_required" => {
                Ok(Reply::RecoveryRequired)
            }
            other => other,
        };
        Event {
            plugin_apply,
            plugin_admin,
            plugin_manifest,
            plugin_dependencies,
            plugin_form,
            plugin_visual,
            background,
            connected: session.as_ref().is_some_and(Session::is_open),
            transfer_job,
            result,
            palette: palette.clone(),
        }
    })
}

/// The runtime closes a connection after 60 s without requests, and only options questions
/// use this one, so one idle for half of that is replaced before it is trusted.
const OPTIONS_IDLE: Duration = Duration::from_secs(30);

/// Worker for the option lists of plugin forms. A plugin may take its whole time limit to
/// answer; this worker holds its own connection to the runtime, so that wait delays neither
/// Apply nor polling, which stay with the worker of [`start`]. The connection is made when a
/// question needs it and again after it is lost or has gone idle: a failed question is
/// reported to the form, never replayed.
///
/// An idle connection is not closed on a timer: the worker only runs when a question arrives,
/// so it stays open until the next question replaces it or the runtime closes it (60 s without
/// requests). It takes one of the runtime's eight connection slots meanwhile.
pub fn start_options(runtime: PathBuf) -> Worker<OptionsJob, OptionsEvent> {
    start_options_idle(runtime, OPTIONS_IDLE)
}
/// [`start_options`] with the idle time after which its connection is replaced.
pub fn start_options_idle(runtime: PathBuf, idle: Duration) -> Worker<OptionsJob, OptionsEvent> {
    let mut connection: Option<(Session<Client>, Instant)> = None;
    Worker::spawn(move |job: OptionsJob| {
        let result = options_session(&runtime, idle, &mut connection)
            .and_then(|session| session.request(job.request))
            .and_then(|response| {
                serde_json::to_value(response.data).map_err(|_| Error::protocol())
            });
        OptionsEvent {
            ticket: job.ticket,
            result,
        }
    })
}
fn options_session<'a>(
    runtime: &std::path::Path,
    idle: Duration,
    connection: &'a mut Option<(Session<Client>, Instant)>,
) -> Result<&'a mut Session<Client>, Error> {
    if connection
        .as_ref()
        .is_none_or(|(session, used)| !session.is_open() || used.elapsed() >= idle)
    {
        // Close the old connection first: the runtime counts it until then, and the editor
        // would briefly hold three of the eight slots (this, the new one and the main worker's).
        *connection = None;
        let client = Client::connect(runtime, Endpoint::Editor).map_err(|e| Error {
            code: e.code.into(),
            cause: None,
            source_change: None,
            unknown: false,
        })?;
        *connection = Some((Session::new(client), Instant::now()));
    }
    let (session, used) = connection.as_mut().expect("connected above");
    *used = Instant::now();
    Ok(session)
}
