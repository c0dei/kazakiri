//! 前回開いていたファイル（%APPDATA%\kazakiri\session.txt）。
//!
//! 1 行目が表示中のタブの番号、2 行目以降がファイルのパス。引数なしで起動したときに復元する。

use std::path::{Path, PathBuf};

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(Path::new(&base).join("kazakiri").join("session.txt"))
}

pub fn load() -> (Vec<PathBuf>, usize) {
    let Some(p) = store_path() else { return (Vec::new(), 0) };
    let Ok(text) = std::fs::read_to_string(p) else { return (Vec::new(), 0) };
    parse(&text)
}

fn parse(text: &str) -> (Vec<PathBuf>, usize) {
    let mut lines = text.lines();
    let active = lines.next().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    let paths: Vec<PathBuf> = lines.filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect();
    (paths, active)
}

/// 保存の失敗は無視する（なくても困らないので）
pub fn save(paths: &[PathBuf], active: usize) {
    let Some(store) = store_path() else { return };
    if let Some(dir) = store.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(store, format(paths, active));
}

fn format(paths: &[PathBuf], active: usize) -> String {
    let mut s = format!("{active}\n");
    for p in paths {
        s.push_str(&format!("{}\n", p.display()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let paths = vec![PathBuf::from("C:\\a b\\x.log"), PathBuf::from("D:\\日本語.csv")];
        let (p, a) = parse(&format(&paths, 1));
        assert_eq!((p, a), (paths, 1));
    }

    #[test]
    fn broken_input() {
        assert_eq!(parse(""), (vec![], 0));
        assert_eq!(parse("x\n\nfoo\n"), (vec![PathBuf::from("foo")], 0));
    }
}
