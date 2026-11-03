// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `--self-test [--out <dir>]`: run diagnostics without a window, print
    // the zip path and exit (see `selftest.rs` / SMOKE_TEST.md).
    let args: Vec<String> = std::env::args().collect();
    if let Some(opts) = setup_coach_app::selftest::parse_args(&args) {
        std::process::exit(setup_coach_app::selftest::cli_main(opts));
    }
    setup_coach_app::run();
}
