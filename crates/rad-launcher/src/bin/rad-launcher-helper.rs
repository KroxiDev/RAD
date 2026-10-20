fn main() {
    if rad_launcher::helper::run().is_err() {
        std::process::exit(1);
    }
}
