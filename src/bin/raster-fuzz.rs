fn main() {
    let code = raster::fuzz::run_cli(std::env::args().skip(1));
    std::process::exit(code);
}
