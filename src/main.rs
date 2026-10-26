#![windows_subsystem = "windows"]

mod gui;
mod tray;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--tray") => tray::run(),
        // Elevated helper that installs Equalizer APO on devices (see apo_setup).
        Some("--apo-setup") => equinox::apo_setup::elevated_main(&args[1..]),
        // `--page automation|settings|devices` opens on that page (devices: the output picker).
        Some("--page") => gui::run(args.get(1).map(String::as_str)),
        _ => gui::run(None),
    }
}
