//! 最近使ったファイル（%APPDATA%\kazakiri\recent.txt に保存）。

use std::path::{Path, PathBuf};

use crate::menu::RECENT_MAX;

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(Path::new(&base).join("kazakiri").join("recent.txt"))
}

pub fn load() -> Vec<PathBuf> {
    let Some(p) = store_path() else { return Vec::new() };
    std::fs::read_to_string(p)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).take(RECENT_MAX).collect())
        .unwrap_or_default()
}

/// 先頭に追加して保存する（保存の失敗は無視：使い勝手に関わるだけなので）
pub fn push(list: &mut Vec<PathBuf>, path: &Path) {
    let path = std::fs::canonicalize(path).map(strip_verbatim).unwrap_or_else(|_| path.to_path_buf());
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(RECENT_MAX);
    if let Some(store) = store_path() {
        if let Some(dir) = store.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
        let _ = std::fs::write(store, body);
    }
}

/// canonicalize が付ける \\?\ を外して見やすくする
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}
