// On Windows, export the two flags that tell NVIDIA Optimus and AMD
// switchable-graphics drivers to run the game on the fast GPU of a laptop
// with two (the statics are in main.rs).
fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" {
        for sym in ["NvOptimusEnablement", "AmdPowerXpressRequestHighPerformance"] {
            if env == "msvc" {
                println!("cargo:rustc-link-arg-bins=/EXPORT:{sym},DATA");
            }
        }
        if env == "gnu" {
            println!("cargo:rustc-link-arg-bins=-Wl,--export-all-symbols");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
