//! 開いている（メモリマップ中の）ファイルに、ほかのプロセスが何をできるか。
//! Windows の挙動の確認用（ログを書き続けるアプリやローテーションを妨げないかの検証）。

#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use kazakiri_core::{DiskState, Document, Refresh};

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kazakiri-share-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn other_process_can_append_while_open() {
    let path = tmp("append.log");
    std::fs::write(&path, "a\nb\n").unwrap();
    let doc = Document::open(&path).unwrap();
    assert_eq!(doc.disk_state(), DiskState::Same);
    OpenOptions::new().append(true).open(&path).expect("追記で開けない").write_all(b"c\n").expect("追記できない");
    assert_eq!(doc.disk_state(), DiskState::Changed);
    assert!(matches!(doc.refresh().unwrap(), Refresh::Appended(_)));
}

#[test]
fn other_process_can_overwrite_in_place_and_we_notice() {
    let path = tmp("inplace.log");
    std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
    let doc = Document::open(&path).unwrap();
    let mut f = OpenOptions::new().write(true).open(&path).expect("書き込みで開けない");
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(b"XXXX").expect("上書きできない");
    drop(f);
    // 大きさが同じ書き換えも、更新時刻で見つかる
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut f = OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all(b"YYYY").unwrap();
    drop(f);
    assert_eq!(doc.disk_state(), DiskState::Changed);
    assert!(matches!(doc.refresh().unwrap(), Refresh::Replaced));
}

#[test]
fn truncate_and_delete_while_open_are_reported() {
    let path = tmp("trunc.log");
    std::fs::write(&path, "aaaa\nbbbb\n").unwrap();
    let doc = Document::open(&path).unwrap();
    let trunc = OpenOptions::new().write(true).truncate(true).open(&path);
    println!("truncate while mapped: {:?}", trunc.as_ref().err().map(|e| e.to_string()));
    let del = std::fs::remove_file(&path);
    println!("delete while mapped: {:?}", del.as_ref().err().map(|e| e.to_string()));
    let ren = std::fs::rename(&path, tmp("trunc.log.1"));
    println!("rename while mapped: {:?}", ren.as_ref().err().map(|e| e.to_string()));
    drop(trunc);
    let _ = doc.disk_state();
}
