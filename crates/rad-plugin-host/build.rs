fn main() {
    println!(
        "cargo:rustc-env=RAD_PLUGIN_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
