#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("hayate の GUI は Windows 専用です。ベンチマークは hayate-bench を使ってください。");
}
