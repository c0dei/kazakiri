//! 保存時のバックアップと、自動保存のファイルの置き場所。

use std::io;
use std::path::{Path, PathBuf};

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into())
}

/// 置き場所の違うフォルダでも名前がぶつからないよう、元のパスの短い印を付ける
fn path_tag(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

/// バックアップの保存先。`dir` が None なら元のファイルと同じフォルダに `名前.拡張子.bak`。
/// 別のフォルダなら `dir/名前.拡張子.印.bak`。`numbered` なら、すでにあるとき `.bak2`、`.bak3` … と上書きしない名前にする
pub fn backup_path_in(path: &Path, dir: Option<&Path>, numbered: bool) -> PathBuf {
    let name = match dir {
        Some(_) => format!("{}.{}", file_name(path), path_tag(path)),
        None => file_name(path),
    };
    let base = dir.map(Path::to_path_buf).unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
    let mut n = 1u32;
    loop {
        let suffix = if n == 1 { ".bak".to_string() } else { format!(".bak{n}") };
        let candidate = base.join(format!("{name}{suffix}"));
        if !numbered || !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 元のファイルをバックアップにコピーする。元のファイルがなければ何もしない（None）
pub fn make_backup(src: &Path, dir: Option<&Path>, numbered: bool) -> io::Result<Option<PathBuf>> {
    if !src.is_file() {
        return Ok(None);
    }
    if let Some(d) = dir {
        std::fs::create_dir_all(d)?;
    }
    let dest = backup_path_in(src, dir, numbered);
    std::fs::copy(src, &dest)?;
    Ok(Some(dest))
}

/// 自動保存のファイル：`名前.拡張子.autosave`（フォルダを指定したときは `名前.拡張子.印.autosave`）
pub fn autosave_path(path: &Path, dir: Option<&Path>) -> PathBuf {
    match dir {
        Some(d) => d.join(format!("{}.{}.autosave", file_name(path), path_tag(path))),
        None => path.with_file_name(format!("{}.autosave", file_name(path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_and_copy() {
        let dir = std::env::temp_dir().join(format!("kazakiri-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "one").unwrap();
        // 同じフォルダ、上書き
        let b = make_backup(&f, None, false).unwrap().unwrap();
        assert_eq!(b, dir.join("a.txt.bak"));
        std::fs::write(&f, "two").unwrap();
        make_backup(&f, None, false).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "two");
        // 番号を増やす
        std::fs::write(&f, "three").unwrap();
        let b2 = make_backup(&f, None, true).unwrap().unwrap();
        assert_eq!(b2, dir.join("a.txt.bak2"));
        assert_eq!(std::fs::read_to_string(&b2).unwrap(), "three");
        // 別のフォルダ（なければ作る）
        let other = dir.join("sub").join("bk");
        let b3 = make_backup(&f, Some(&other), false).unwrap().unwrap();
        assert!(b3.starts_with(&other) && b3.extension().is_some_and(|e| e == "bak"));
        assert_eq!(std::fs::read_to_string(&b3).unwrap(), "three");
        // 元がなければ何もしない
        assert!(make_backup(&dir.join("none.txt"), None, false).unwrap().is_none());
        assert_eq!(autosave_path(&f, None), dir.join("a.txt.autosave"));
        assert_ne!(autosave_path(&f, Some(&other)), autosave_path(&dir.join("x").join("a.txt"), Some(&other)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
