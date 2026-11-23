use rad_plugin_audio::{AudioPlugin, Pactl, Timing, config};

fn main() {
    let program = std::env::args_os().nth(1).expect("path of the fake pactl");
    let pactl = Pactl::new(
        program,
        std::env::args_os()
            .nth(2)
            .or_else(|| std::env::var_os("XDG_RUNTIME_DIR")),
    );
    let plugin = AudioPlugin::new(Some(pactl), Timing::default());
    if let Err(error) = rad_plugin_sdk::run_stdio(config(), plugin) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
