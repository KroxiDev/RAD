use rad_runtime::{
    bootstrap,
    lifecycle::{Options, Paths, ShutdownSignals, helper_command},
    server::{RuntimeError, Server},
};
use std::{os::fd::AsRawFd, process::ExitCode, time::Duration};

fn main() -> ExitCode {
    let result = run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rad-runtime: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), &'static str> {
    let options = Options::parse(std::env::args_os().skip(1))
        .map_err(|_| "argumentos inválidos; use --help")?;
    if options.help {
        println!(
            "rad-runtime [--xl-exclusive]\nSin hardware por defecto.\n--xl-exclusive: requiere StreamController cerrado y su auto-restart deshabilitado."
        );
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|_| "no se pudo localizar el ejecutable")?;
    let paths = Paths::from_values(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        &executable,
    )
    .map_err(|_| "se requiere XDG_RUNTIME_DIR absoluto y XDG_DATA_HOME o HOME absoluto")?;
    paths.prepare().map_err(
        |_| "rutas inseguras o inaccesibles; se requieren directorios privados sin symlinks",
    )?;
    // This must precede Server/worker creation: worker threads inherit the blocked mask.
    let signals = ShutdownSignals::install().map_err(|_| "no se pudo instalar signalfd")?;
    let session = bootstrap::random_id().map_err(|_| "no se pudo crear identificador de sesión")?;
    let mut server = Server::new(&paths.runtime, &paths.data, session).map_err(runtime_error)?;
    if server.recovered_previous {
        eprintln!(
            "rad-runtime: se recuperó la publicación anterior válida; revise la configuración antes de continuar."
        );
    }
    let fd = signals.as_raw_fd();
    server
        .register_worker_io(&mut mio::unix::SourceFd(&fd), mio::Interest::READABLE)
        .map_err(runtime_error)?;
    // Command inherits the desktop session, but pre_exec unblocks shutdown signals.
    let launcher = rad_launcher::Launcher::spawn(helper_command(&paths.helper))
        .map_err(|_| "no se pudo iniciar rad-launcher-helper hermano del runtime")?;
    server.attach_launcher(launcher).map_err(runtime_error)?;
    if options.xl_exclusive {
        eprintln!(
            "rad-runtime: --xl-exclusive: requiere StreamController CERRADO y auto-restart DESHABILITADO; no se detienen otros controladores automáticamente."
        );
        server
            .attach_hardware(rad_deck::DeviceConfig::xl_after_exclusive_handoff())
            .map_err(runtime_error)?;
    }
    loop {
        if signals
            .take_shutdown()
            .map_err(|_| "falló la lectura de signalfd")?
        {
            server.request_shutdown();
        }
        server.turn().map_err(runtime_error)?;
        if server.shutdown_complete() {
            return Ok(());
        }
        // A maximum sleep only, not a periodic signal poll: Mio wakes on signalfd,
        // worker/helper/device readiness; Server clamps to its actual deadlines.
        server
            .wait(Duration::from_secs(86_400))
            .map_err(runtime_error)?;
    }
}

fn runtime_error(error: RuntimeError) -> &'static str {
    match error {
        RuntimeError::Startup => {
            "no se pudo abrir la configuración. Material conservado: no se crea una configuración sustitutiva ni se mueve la base. Opciones: salir y conservar los archivos, o restaurar manualmente un respaldo válido con RAD detenido antes de reintentar."
        }
        RuntimeError::Transport => {
            "no se pudo operar el socket/instancia privada; compruebe si RAD ya está ejecutándose"
        }
        RuntimeError::Clock => "falló el reloj monotónico del sistema",
        RuntimeError::Storage => {
            "falló el almacenamiento; conserve los archivos y compruebe el estado antes de reintentar"
        }
        RuntimeError::Protocol => "falló la integración interna del runtime",
    }
}
