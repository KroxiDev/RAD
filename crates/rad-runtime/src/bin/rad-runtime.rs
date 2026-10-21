use rad_runtime::{
    bootstrap, exit,
    exit::{Exit, Reason},
    lifecycle::{Options, Paths, ShutdownSignals, helper_command},
    server::Server,
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
    paths.prepare_data().map_err(|_| {
        Exit::Unexpected(
            Reason::Storage,
            "carpeta de datos insegura o inaccesible; se requiere un directorio privado sin symlinks",
        )
    })?;
    // Only a verified private data directory may receive the record of how this run ends.
    exit::install_panic_record(paths.data.clone());
    serve(&options, &paths).inspect_err(|exit| {
        if let Some(reason) = exit.reason() {
            let _ = exit::record(&paths.data, reason, exit::now_unix_ms());
        }
    })
}

fn serve(options: &Options, paths: &Paths) -> Result<(), Exit> {
    let startup = |message| Exit::Unexpected(Reason::Startup, message);
    // This must precede Server/worker creation: worker threads inherit the blocked mask.
    let signals =
        ShutdownSignals::install().map_err(|_| startup("no se pudo instalar signalfd"))?;
    let session =
        bootstrap::random_id().map_err(|_| startup("no se pudo crear identificador de sesión"))?;
    let mut server = Server::new(&paths.runtime, &paths.data, session)?;
    if server.recovered_previous {
        eprintln!(
            "rad-runtime: se recuperó la publicación anterior válida; revise la configuración antes de continuar."
        );
    }
    let fd = signals.as_raw_fd();
    server.register_worker_io(&mut mio::unix::SourceFd(&fd), mio::Interest::READABLE)?;
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
    loop {
        if signals
            .take_shutdown()
            .map_err(|_| Exit::Unexpected(Reason::Protocol, "falló la lectura de signalfd"))?
        {
            server.request_shutdown();
        }
        server.turn()?;
        if server.shutdown_complete() {
            return Ok(());
        }
        // A maximum sleep only, not a periodic signal poll: Mio wakes on signalfd,
        // worker/helper/device readiness; Server clamps to its actual deadlines.
        server.wait(Duration::from_secs(86_400))?;
    }
}
