//! kazakiri-bench: コアの性能を測るためのコマンド。
//!
//!   kazakiri-bench gen <出力パス> <サイズMB> [log|csv|sjis]   テスト用ファイルを作る
//!   kazakiri-bench open <パス> [検索語]                        開く・索引・表示・検索の時間を測る

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use kazakiri_core::{Direction, Document, Query, SearchControl, SearchOptions, SearchOutcome};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") if args.len() >= 4 => {
            let mb: u64 = args[3].parse().expect("サイズMB");
            gen(Path::new(&args[2]), mb, args.get(4).map(String::as_str).unwrap_or("log"));
        }
        Some("open") if args.len() >= 3 => open(Path::new(&args[2]), args.get(3).map(String::as_str)),
        _ => {
            eprintln!("usage:\n  kazakiri-bench gen <path> <MB> [log|csv|sjis]\n  kazakiri-bench open <path> [needle]");
            std::process::exit(2);
        }
    }
}

fn gen(path: &Path, mb: u64, kind: &str) {
    let target = mb << 20;
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).expect("create"));
    let mut written = 0u64;
    let mut i = 0u64;
    let mut line = String::new();
    let sjis = Document::from_bytes(vec![0x82, 0xA0]); // 「あ」(SJIS) → SJIS エンコーダとして使う
    while written < target {
        line.clear();
        match kind {
            "csv" | "sjis" => {
                use std::fmt::Write;
                let _ = write!(line, "{i},user{},{}.{:02},\"東京都,渋谷区\",{}\r\n", i % 9973, i % 100000, i % 100, i * 7 % 1000003);
            }
            _ => {
                use std::fmt::Write;
                let level = ["INFO", "WARN", "ERROR", "DEBUG"][(i % 4) as usize];
                let _ = writeln!(
                    line,
                    "2026-11-09 12:{:02}:{:02}.{:03} [{level}] req={i} 処理が完了しました path=/api/v1/items/{}",
                    i / 60 % 60, i % 60, i % 1000, i % 4093
                );
            }
        }
        let bytes = if kind == "sjis" {
            sjis.encode(&line)
        } else {
            line.clone().into_bytes()
        };
        w.write_all(&bytes).unwrap();
        written += bytes.len() as u64;
        i += 1;
    }
    w.flush().unwrap();
    println!("wrote {} lines, {} bytes -> {}", i, written, path.display());
}

fn open(path: &Path, needle: Option<&str>) {
    let t0 = Instant::now();
    let doc = Document::open(path).expect("open");
    let t_open = t0.elapsed();

    // 先頭画面（50 行）が出るまで
    let first = doc.lines(0, 50);
    let t_first = t0.elapsed();

    while !doc.progress().done {
        std::thread::sleep(Duration::from_millis(1));
    }
    let t_index = t0.elapsed();
    let (lines, _) = doc.line_count();

    println!("file      : {} ({:.2} GB)", path.display(), doc.len() as f64 / (1u64 << 30) as f64);
    println!("encoding  : {} / {}", doc.encoding().name(), doc.newline_name());
    println!("lines     : {lines}");
    println!("open      : {:?}", t_open);
    println!("1st screen: {:?}  ({} lines)", t_first, first.len());
    println!(
        "index     : {:?}  ({:.2} GB/s)",
        t_index,
        doc.len() as f64 / (1u64 << 30) as f64 / t_index.as_secs_f64()
    );
    if let Some(l) = first.first() {
        println!("line 1    : {}", l.text.chars().take(100).collect::<String>());
    }

    // ランダムな行へのジャンプ（スクロールバーのドラッグ相当）
    let t = Instant::now();
    let jumps = 10_000u64;
    let mut x = 0x9E3779B97F4A7C15u64;
    for _ in 0..jumps {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let l = doc.lines(x % lines, 60);
        std::hint::black_box(l);
    }
    println!("jump+60l  : {:?} / screen", t.elapsed() / jumps as u32);

    if let Some(needle) = needle {
        let gbps = |el: Duration| doc.len() as f64 / (1u64 << 30) as f64 / el.as_secs_f64();
        let modes = [
            ("literal", SearchOptions { case_sensitive: true, regex: false }),
            ("ignore-case", SearchOptions { case_sensitive: false, regex: false }),
            ("regex", SearchOptions { case_sensitive: true, regex: true }),
        ];
        for (name, opts) in modes {
            let q = match Query::new(&doc, needle, opts) {
                Ok(q) => q,
                Err(e) => {
                    println!("{name:<12}: {e}");
                    continue;
                }
            };
            let ctl = SearchControl::default();
            // 全件を前から順に（一致が多いと 1 件ごとの呼び出しコストが効く）
            let t = Instant::now();
            let (mut count, mut pos, mut last) = (0u64, 0u64, None);
            while let SearchOutcome::Found(s, e) = q.find(&doc, pos, Direction::Forward, &ctl) {
                count += 1;
                last = Some(s);
                pos = e.max(s + 1);
            }
            let el = t.elapsed();
            println!("{name:<12}: {count} hits, all in {el:?} ({:.2} GB/s)", gbps(el));
            // 末尾から最後の一致を 1 件（後方検索）
            let t = Instant::now();
            let back = q.find(&doc, doc.len(), Direction::Backward, &ctl);
            let ok = matches!(back, SearchOutcome::Found(s, _) if Some(s) == last) || (last.is_none() && back == SearchOutcome::NotFound);
            println!("{:<12}  backward last hit in {:?} (matches forward: {ok})", "", t.elapsed());
        }
    }
    // 編集：ランダムな位置に 1000 回挿入し、そのあとの表示・検索の速さ
    {
        use kazakiri_core::{EditKind, Editor};
        let mut ed = Editor::new(std::sync::Arc::new(doc));
        let t = Instant::now();
        let mut x = 0x2545F4914F6CDD1Du64;
        for i in 0..1000u64 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let line = x % lines;
            let pos = ed.doc().line_start(line).unwrap();
            ed.break_group();
            ed.replace(pos, pos, &format!("EDIT{i} "), EditKind::Other, (pos, pos)).unwrap();
        }
        println!("edit x1000: {:?} total ({} pieces)", t.elapsed(), ed.doc().piece_count());
        let t = Instant::now();
        for _ in 0..1000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            std::hint::black_box(ed.doc().lines(x % lines, 60));
        }
        println!("edited jump+60l: {:?} / screen", t.elapsed() / 1000);
        let t = Instant::now();
        let q = Query::new(ed.doc(), "EDIT999 ", SearchOptions { case_sensitive: true, regex: false }).unwrap();
        let r = q.find(ed.doc(), 0, Direction::Forward, &SearchControl::default());
        println!("edited find: {:?} in {:?}", matches!(r, SearchOutcome::Found(..)), t.elapsed());
        let t = Instant::now();
        while ed.undo().is_some() {}
        println!("undo all  : {:?}, modified={}", t.elapsed(), ed.is_modified());

        // すべて置換：[INFO] → [info]（一致が多いとき）
        let t = Instant::now();
        let q = Query::new(ed.doc(), "[INFO]", SearchOptions { case_sensitive: true, regex: false }).unwrap();
        let found = q.find_all(ed.doc(), Some("[info]"), 50_000_000, &SearchControl::default());
        let t_find = t.elapsed();
        let n = ed.replace_all(&found.ranges, &found.replacements, (0, 0)).unwrap();
        drop(found);
        println!("after replace RSS: {}", peak_rss());
        println!("replace all: {n} hits, find {t_find:?} + apply {:?} ({} pieces)", t.elapsed() - t_find, ed.doc().piece_count());
        let t = Instant::now();
        std::hint::black_box(ed.doc().lines(lines / 2, 60));
        println!("after replace jump+60l: {:?}", t.elapsed());
        let t = Instant::now();
        ed.undo();
        println!("undo replace: {:?}", t.elapsed());
        println!("peak RSS (replace): {}", peak_rss());
    }
    println!("peak RSS  : {}", peak_rss());
}

fn peak_rss() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        let get = |k: &str| s.lines().find(|l| l.starts_with(k)).unwrap_or("").to_string();
        return format!("{} | {}", get("VmHWM").trim(), get("RssAnon").trim());
    }
    "n/a".into()
}
