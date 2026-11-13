//! Standalone deterministic fuzz target. This intentionally does not depend on
//! libFuzzer or cargo-fuzz, so it runs reproducibly in offline CI.

fn main() {
    let code = raster::fuzz::run_cli(std::env::args().skip(1));
    std::process::exit(code);
}
