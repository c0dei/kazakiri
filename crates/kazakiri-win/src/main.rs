#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod keymap;
mod menu;
#[cfg(windows)]
mod recent;
#[cfg_attr(not(windows), allow(dead_code))]
mod session;
mod settings;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("kazakiri の GUI は Windows 専用です。ベンチマークは kazakiri-bench を使ってください。");
}
