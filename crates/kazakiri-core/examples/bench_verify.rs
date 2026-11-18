//! 実ファイルでの総合検証：`cargo run --release --example bench_verify -- ファイル 保存先の一時ファイル`
//! 検索・全件検索・置換・編集・取り消し・保存を順に行い、時間と結果の正しさ（保存後のバイト数）を確認する。

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use kazakiri_core::{Direction, Document, Editor, EditKind, Query, SearchControl, SearchOptions, SearchOutcome};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::Path::new(&args[1]);
    let t = Instant::now();
    let doc = Document::open(path).unwrap();
    while !doc.progress().done {
        std::thread::yield_now();
    }
    let doc = Arc::new(doc);
    let (lines, _) = doc.line_count();
    println!("open+index {:?} lines={lines} size={}", t.elapsed(), doc.len());
    let ctl = SearchControl::default();

    // 末尾付近の行を取り出す
    let t = Instant::now();
    let last = doc.lines(lines - 3, 3);
    println!("last 3 lines {:?}: {:?}", t.elapsed(), last.iter().map(|l| l.text.chars().take(40).collect::<String>()).collect::<Vec<_>>());

    for (name, pat, opts) in [
        ("literal case-sensitive", "ProgressHandler", SearchOptions { case_sensitive: true, regex: false, whole_word: false }),
        ("literal case-insens.", "progresshandler", SearchOptions { case_sensitive: false, regex: false, whole_word: false }),
        ("regex", r"weight \[\d{6,}\]", SearchOptions { case_sensitive: true, regex: true, whole_word: false }),
        ("whole word", "Info", SearchOptions { case_sensitive: true, regex: false, whole_word: true }),
    ] {
        let q = Query::new(&doc, pat, opts).unwrap();
        let t = Instant::now();
        let r = q.find_all(&doc, None, usize::MAX, &ctl);
        println!("find_all {name}: {:?} hits={} truncated={}", t.elapsed(), r.ranges.len(), r.truncated);
    }

    // 後方検索（末尾から 1 件）
    let q = Query::new(&doc, "DownloadRequestEnd", SearchOptions { case_sensitive: true, regex: false, whole_word: false }).unwrap();
    let t = Instant::now();
    let out = q.find(&doc, doc.len(), Direction::Backward, &ctl);
    println!("find backward from end {:?}: {}", t.elapsed(), matches!(out, SearchOutcome::Found { .. }));

    // 置換（全件）→ 取り消し → 保存
    let q = Query::new(&doc, "Info", SearchOptions { case_sensitive: true, regex: false, whole_word: true }).unwrap();
    let t = Instant::now();
    let found = q.find_all(&doc, Some("INFORMATION"), usize::MAX, &ctl);
    println!("replace find {:?} hits={}", t.elapsed(), found.ranges.len());
    let mut ed = Editor::new(doc.clone());
    let t = Instant::now();
    let n = ed.replace_all(&found.ranges, &found.replacements, (0, 0)).unwrap();
    println!("replace_all {:?} n={n} piece_count={}", t.elapsed(), ed.doc().piece_count());
    let expect = doc.len() + n as u64 * ("INFORMATION".len() as u64 - "Info".len() as u64);
    println!("expected size {expect} actual {}", ed.doc().len());
    let t = Instant::now();
    let written = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);
    let tmp = ed.doc().write_temp(std::path::Path::new(&args[2]), &written, &cancel).unwrap();
    println!("save {:?} -> {} bytes ({})", t.elapsed(), std::fs::metadata(&tmp).unwrap().len(), tmp.display());
    let _ = std::fs::remove_file(&tmp);
    let t = Instant::now();
    ed.undo();
    println!("undo {:?} size={}", t.elapsed(), ed.doc().len());

    // 途中への 1 文字挿入の連続（キー入力相当）
    let mut ed = Editor::new(doc.clone());
    let t = Instant::now();
    let mid = doc.len() / 2;
    let mid = doc.line_start(doc.line_of_offset(mid).unwrap()).unwrap();
    for i in 0..2000u64 {
        ed.replace(mid + i, mid + i, "x", EditKind::Typing, (mid + i, mid + i)).unwrap();
    }
    println!("2000 single-char inserts {:?}", t.elapsed());
}
