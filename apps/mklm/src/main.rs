//! `mklm.exe`: the GUI of Multi Keyboard Layout Manager. Everything is in the `mklm_gui` library
//! (src/lib.rs); see docs/design/m3-gui.md.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let args = mklm_gui::args::parse(std::env::args().skip(1));
    mklm_gui::app::run(args)
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("mklm runs on Windows 11 only");
    std::process::ExitCode::FAILURE
}
