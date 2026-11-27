//! 開いているファイルに、ほかのプロセスが何をできるか（Windows の挙動の確認と回帰テスト）。
//! ログを書き続けるアプリやローテーションを妨げないことを確かめる。
//!
//! - メモリに読み込んで開いたファイル（上限以下）：追記・上書き・切り詰め・削除・改名のすべてができる
//! - メモリマップで開いたファイル（上限より大きい）：切り詰めだけが、Windows に拒否される

#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kazakiri_core::source::DEFAULT_MEMORY_LIMIT;
use kazakiri_core::{DiskState, Document, EditKind, Editor, Refresh};

/// マップされたファイルを切り詰めようとしたときの Windows のエラー（ERROR_USER_MAPPED_FILE）
const ERROR_USER_MAPPED_FILE: i32 = 1224;

/// 常にマップ／既定の上限（小さなファイルはメモリ）
const MAPPED: u64 = 0;
const IN_MEMORY: u64 = DEFAULT_MEMORY_LIMIT;

/// 行索引ができあがるまで待つ（追記の読み込みは、索引を引き継ぐので、できあがっていないと `Replaced` になる）
fn wait(doc: &Document) {
    while !doc.progress().done {
        std::thread::yield_now();
    }
}

fn open(path: &Path, limit: u64) -> Document {
    let doc = Document::open_with_limit(path, None, limit).unwrap();
    wait(&doc);
    doc
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kazakiri-share-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn texts(doc: &Document) -> Vec<String> {
    doc.lines(0, 100).into_iter().map(|l| l.text).collect()
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new().append(true).open(path).expect("追記で開けない").write_all(bytes).expect("追記できない");
}

#[test]
fn small_files_are_loaded_into_memory_by_default() {
    let path = tmp("default.log");
    std::fs::write(&path, "a\n").unwrap();
    let doc = Document::open(&path).unwrap();
    assert!(!doc.is_mapped());
    assert!(open(&path, MAPPED).is_mapped());
}

#[test]
fn other_process_can_append_while_open() {
    for (name, limit) in [("append-mem.log", IN_MEMORY), ("append-map.log", MAPPED)] {
        let path = tmp(name);
        std::fs::write(&path, "a\nb\n").unwrap();
        let doc = open(&path, limit);
        assert_eq!(doc.disk_state(), DiskState::Same);
        append(&path, b"c\n");
        assert_eq!(doc.disk_state(), DiskState::Changed);
        let Refresh::Appended(new) = doc.refresh().unwrap() else { panic!("追記と判定されない: {name}") };
        wait(&new);
        assert_eq!(texts(&new), ["a", "b", "c", ""]);
        assert_eq!(new.is_mapped(), limit == MAPPED);
    }
}

#[test]
fn other_process_can_overwrite_in_place_and_we_notice() {
    for (name, limit) in [("inplace-mem.log", IN_MEMORY), ("inplace-map.log", MAPPED)] {
        let path = tmp(name);
        std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
        let doc = open(&path, limit);
        // 大きさが同じ書き換えも、更新時刻で見つかる
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut f = OpenOptions::new().write(true).open(&path).expect("書き込みで開けない");
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(b"YYYY").expect("上書きできない");
        drop(f);
        assert_eq!(doc.disk_state(), DiskState::Changed);
        assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced), "{name}");
    }
}

#[test]
fn copytruncate_rotation_works_while_a_small_file_is_open() {
    let path = tmp("rotate.log");
    std::fs::write(&path, "old 1\nold 2\n").unwrap();
    let doc = open(&path, IN_MEMORY);
    // ログを書き続けるアプリ（追記のハンドルを持ったまま）
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    // copytruncate：内容を別名に写してから、元のファイルを空にする
    std::fs::copy(&path, tmp("rotate.log.1")).unwrap();
    OpenOptions::new().write(true).open(&path).unwrap().set_len(0).expect("切り詰められない");
    // 開いている側は、読み込んだ内容を持ち続ける
    assert_eq!(texts(&doc), ["old 1", "old 2", ""]);
    assert_eq!(doc.disk_state(), DiskState::Changed);
    assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced));
    let doc = open(&path, IN_MEMORY);
    assert!(doc.is_empty());
    // 切り詰めたあとの書き込みを、開き直して読み、続きの追記として読み込める
    writer.write_all(b"new 1\n").unwrap();
    assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced), "空の状態からは開き直し");
    let doc = open(&path, IN_MEMORY);
    writer.write_all(b"new 2\n").unwrap();
    let Refresh::Appended(doc) = doc.refresh().unwrap() else { panic!("追記と判定されない") };
    wait(&doc);
    assert_eq!(texts(&doc), ["new 1", "new 2", ""]);
}

#[test]
fn truncating_by_recreating_the_file_works_while_a_small_file_is_open() {
    // `>` でのリダイレクトと同じ：作り直しのモードで開く
    let path = tmp("redirect.log");
    std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
    let doc = open(&path, IN_MEMORY);
    std::fs::write(&path, "z\n").expect("作り直せない");
    assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced));
    assert_eq!(texts(&open(&path, IN_MEMORY)), ["z", ""]);
}

#[test]
fn rewritten_longer_file_is_not_taken_for_an_append() {
    // 切り詰めたあと、確認する前に元より長くなった場合
    let path = tmp("longer.log");
    std::fs::write(&path, "aaaa\n").unwrap();
    let doc = open(&path, IN_MEMORY);
    std::fs::write(&path, "bbbbbbbb\ncccccccc\n").unwrap();
    assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced));
}

#[test]
fn edits_survive_truncation_of_a_small_file() {
    let path = tmp("edited.log");
    std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
    let mut editor = Editor::new(Arc::new(open(&path, IN_MEMORY)));
    editor.replace(0, 4, "XXXX", EditKind::Other, (0, 0)).unwrap();
    std::fs::write(&path, "").expect("切り詰められない");
    let doc = editor.doc();
    assert_eq!(texts(doc), ["XXXX", "bbbb", ""]);
    assert_eq!(doc.disk_state(), DiskState::Changed);
    // 編集中は読み直さない
    assert!(matches!(doc.refresh().unwrap(), Refresh::Unchanged));
}

#[test]
fn growing_past_the_limit_switches_to_a_mapping() {
    let path = tmp("grow.log");
    std::fs::write(&path, "1234\n").unwrap();
    let doc = open(&path, 8);
    assert!(!doc.is_mapped());
    append(&path, b"5678\n");
    let Refresh::Appended(new) = doc.refresh().unwrap() else { panic!("追記と判定されない") };
    wait(&new);
    assert!(new.is_mapped());
    assert_eq!(texts(&new), ["1234", "5678", ""]);
}

#[test]
fn only_truncation_is_refused_for_a_mapped_file() {
    let path = tmp("trunc.log");
    std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
    let _doc = open(&path, MAPPED);
    let err = OpenOptions::new().write(true).open(&path).unwrap().set_len(0).expect_err("切り詰められてしまった");
    assert_eq!(err.raw_os_error(), Some(ERROR_USER_MAPPED_FILE));
    std::fs::rename(&path, tmp("trunc.log.1")).expect("改名できない");
    std::fs::remove_file(tmp("trunc.log.1")).expect("削除できない");
}
