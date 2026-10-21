use rad_runtime::{
    bootstrap, exit,
    exit::{Exit, Reason},
    lifecycle::{Options, Paths, ShutdownSignals, helper_command},
    recovery::Recovery,
    server::Server,
    startup::{Bound, Opened},
};
use std::{os::fd::AsRawFd, process::ExitCode, time::Duration};

/// The class of every end is `rad_runtime::exit`'s: 0 clean, 75 blocking, 1 unexpected.
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(exit::CLEAN),
        Err(exit) => {
            eprintln!("rad-runtime: {}", exit.message());
            ExitCode::from(exit.code())
        }
    }
}

fn run() -> Result<(), Exit> {
    let options = Options::parse(std::env::args_os().skip(1))
        .map_err(|_| Exit::Blocking("argumentos inválidos; use --help"))?;
    if options.help {
        println!(
            "rad-runtime [--xl-exclusive]\nSin hardware por defecto.\n--xl-exclusive: requiere StreamController cerrado y su auto-restart deshabilitado."
        );
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|_| Exit::Blocking("no se pudo localizar el ejecutable"))?;
    let paths = Paths::from_values(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        &executable,
    )
    .map_err(|_| {
        Exit::Blocking("se requiere XDG_RUNTIME_DIR absoluto y XDG_DATA_HOME o HOME absoluto")
    })?;
    paths.prepare_runtime().map_err(|_| {
        Exit::Blocking(
            "directorio de runtime inseguro o inaccesible; se requiere un directorio privado sin symlinks",
        )
    })?;
    // This must precede Server/worker creation: worker threads inherit the blocked mask.
    let signals = ShutdownSignals::install()
        .map_err(|_| Exit::Unexpected(Reason::Startup, "no se pudo instalar signalfd"))?;
    let session = bootstrap::random_id().map_err(|_| {
        Exit::Unexpected(Reason::Startup, "no se pudo crear identificador de sesión")
    })?;
    // The instance lock comes before anything in the data folder is looked at.
    let bound = Bound::bind(&paths.runtime, session)?;
    // Only a verified private data directory may receive the record of how this run ends.
    let data_verified = paths.prepare_data().is_ok();
    if data_verified {
        exit::install_panic_record(paths.data.clone());
    }
    serve(&options, &paths, signals, bound, data_verified).inspect_err(|exit| {
        if data_verified && let Some(reason) = exit.reason() {
            let _ = exit::record(&paths.data, reason, exit::now_unix_ms());
        }
    })
}

/// Recovery mode (ADR 0011) until the configuration is usable, then the normal runtime
/// in the same process.
enum Mode {
    Recovery(Box<Recovery>),
    Serving(Box<Server>),
}

fn serve(
    options: &Options,
    paths: &Paths,
    signals: ShutdownSignals,
    bound: Bound,
    data_verified: bool,
) -> Result<(), Exit> {
    let opened = if data_verified {
        bound.open(&paths.data)?
    } else {
        Opened::Recovery(Box::new(bound.unsafe_folder(&paths.data)))
    };
    let fd = signals.as_raw_fd();
    let mut mode = match opened {
        Opened::Recovery(recovery) => {
            recovery.register_worker_io(&mut mio::unix::SourceFd(&fd), mio::Interest::READABLE)?;
            Mode::Recovery(recovery)
        }
        Opened::Serving(mut server) => {
            server.register_worker_io(&mut mio::unix::SourceFd(&fd), mio::Interest::READABLE)?;
            attach(&mut server, options, paths)?;
            Mode::Serving(server)
        }
    };
    loop {
        if signals
            .take_shutdown()
            .map_err(|_| Exit::Unexpected(Reason::Protocol, "falló la lectura de signalfd"))?
        {
            match &mut mode {
                Mode::Recovery(recovery) => recovery.request_shutdown(),
                Mode::Serving(server) => server.request_shutdown(),
            }
        }
        match &mut mode {
            Mode::Recovery(recovery) => recovery.turn()?,
            Mode::Serving(server) => server.turn()?,
        }
        let finished = match &mode {
            Mode::Recovery(recovery) => recovery.shutdown_complete(),
            Mode::Serving(server) => server.shutdown_complete(),
        };
        if finished {
            return Ok(());
        }
        if matches!(&mode, Mode::Recovery(recovery) if recovery.reset_done()) {
            let Mode::Recovery(recovery) = mode else {
                unreachable!("checked above")
            };
            let mut server = Box::new(recovery.into_server()?);
            attach(&mut server, options, paths)?;
            mode = Mode::Serving(server);
            // The requests queued behind the restart belong to the server: no sleeping.
            continue;
        }
        // A maximum sleep only, not a periodic signal poll: Mio wakes on signalfd,
        // worker/helper/device readiness; the mode clamps to its actual deadlines.
        let maximum = Duration::from_secs(86_400);
        match &mut mode {
            Mode::Recovery(recovery) => recovery.wait(maximum)?,
            Mode::Serving(server) => server.wait(maximum)?,
        }
    }
}

/// What only the normal runtime has: the launcher helper and, on request, the deck.
fn attach(server: &mut Server, options: &Options, paths: &Paths) -> Result<(), Exit> {
    let startup = |message| Exit::Unexpected(Reason::Startup, message);
    if server.recovered_previous {
        eprintln!(
            "rad-runtime: se recuperó la publicación anterior válida; revise la configuración antes de continuar."
        );
    }
    // Command inherits the desktop session, but pre_exec unblocks shutdown signals.
    let launcher = rad_launcher::Launcher::spawn(helper_command(&paths.helper))
        .map_err(|_| startup("no se pudo iniciar rad-launcher-helper hermano del runtime"))?;
    server.attach_launcher(launcher)?;
    if options.xl_exclusive {
        eprintln!(
            "rad-runtime: --xl-exclusive: requiere StreamController CERRADO y auto-restart DESHABILITADO; no se detienen otros controladores automáticamente."
        );
        server.attach_hardware(rad_deck::DeviceConfig::xl_after_exclusive_handoff())?;
    }
    Ok(())
}
