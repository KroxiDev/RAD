fn main() {
    if let Err(error) = rad_update::installer::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
