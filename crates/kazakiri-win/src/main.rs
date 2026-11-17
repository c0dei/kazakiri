#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod history;
mod keymap;
mod menu;
#[cfg(windows)]
mod recent;
#[cfg_attr(not(windows), allow(dead_code))]
mod session;
mod settings;

#[cfg(windows)]
fn main() {
    // 落ちたときの手がかり：パニックの内容を、一時フォルダの kazakiri-crash.log に書く
    std::panic::set_hook(Box::new(|info| {
        let path = std::env::temp_dir().join("kazakiri-crash.log");
        let _ = std::fs::write(path, format!("{info}\n"));
    }));
    let _ = std::fs::remove_file(std::env::temp_dir().join("kazakiri-start.log"));
    app::crumb(&format!("start build={}", option_env!("GITHUB_SHA").unwrap_or("local")));
    if std::env::args().any(|a| a == "--gallery") {
        app::run_gallery();
        return;
    }
    app::run();
    app::crumb("exit");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("kazakiri の GUI は Windows 専用です。ベンチマークは kazakiri-bench を使ってください。");
}
