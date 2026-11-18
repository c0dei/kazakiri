//! 性能の回帰テスト。大きなファイル（既定 128MB の合成ログ）で、主な操作が時間の予算に収まるかを確かめる。
//!
//! 通常の `cargo test` では動かさない（`#[ignore]`）。CI などでは次のように動かす：
//!   cargo test --release -p kazakiri-core --test perf_guard -- --ignored --nocapture
//!
//! 予算は、手元の実測（128MB で、ほとんどが数 ms〜数十 ms）の数十倍の余裕を持たせてある。
//! 計算量が悪化した（行索引が直列になった、検索が行ごとの処理になった、など）ときだけ落ちるようにして、
//! CI の機械の遅さや揺れでは落ちないようにするため。環境変数：
//!   KAZAKIRI_PERF_MB    ファイルの大きさ（MB、既定 128）
//!   KAZAKIRI_PERF_SLACK 予算にかける倍率（既定 1。遅い機械では 2〜3）

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kazakiri_core::logtime::{filter_time, TimeRange};
use kazakiri_core::{Document, EditKind, Editor, Query, SearchControl, SearchOptions};

struct Guard {
    slack: f64,
    failures: Vec<String>,
}

impl Guard {
    /// 計測した時間を表示し、予算（秒）を超えたら失敗として記録する
    fn check(&mut self, name: &str, took: Duration, budget_secs: f64) {
        let budget = budget_secs * self.slack;
        let ok = took.as_secs_f64() <= budget;
        println!("{} {name:<34} {:>9.1} ms (予算 {:>7.0} ms)", if ok { "ok  " } else { "SLOW" }, took.as_secs_f64() * 1e3, budget * 1e3);
        if !ok {
            self.failures.push(format!("{name}: {:.0} ms > {:.0} ms", took.as_secs_f64() * 1e3, budget * 1e3));
        }
    }
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// 合成ログを書く（毎秒 1 行ほど進む時刻、レベル、本文。ところどころにスタックトレース風の続きの行）
fn write_log(path: &std::path::Path, mb: u64) -> u64 {
    let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path).unwrap());
    let target = mb << 20;
    let (mut written, mut i) = (0u64, 0u64);
    let mut line = String::new();
    while written < target {
        use std::fmt::Write as _;
        line.clear();
        let level = ["INFO", "WARN", "ERROR", "DEBUG"][(i % 4) as usize];
        let _ = writeln!(
            line,
            "2026-11-09 {:02}:{:02}:{:02}.{:03} [{level}] req={i} 処理が完了しました path=/api/v1/items/{}",
            i / 3600 % 24,
            i / 60 % 60,
            i % 60,
            i % 1000,
            i % 4093
        );
        if i % 5000 == 7 {
            line.push_str("java.lang.IllegalStateException: boom\n    at app.Main.run(Main.java:42)\n");
        }
        w.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
        i += 1;
    }
    w.flush().unwrap();
    i
}

fn wait_index(doc: &Document) {
    while !doc.progress().done {
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "大きなファイルを作るので、CI や性能の確認のときだけ（--ignored）"]
fn large_file_operations_stay_within_budget() {
    let mb = env_f64("KAZAKIRI_PERF_MB", 128.0) as u64;
    let mut g = Guard { slack: env_f64("KAZAKIRI_PERF_SLACK", 1.0), failures: Vec::new() };
    let dir = std::env::temp_dir().join(format!("kazakiri-perf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.log");
    let t = Instant::now();
    let lines_written = write_log(&path, mb);
    println!("（{mb}MB・{lines_written} 行を作成: {:?}）", t.elapsed());
    let ctl = SearchControl::default();

    // 開く・先頭画面・全行索引
    let t = Instant::now();
    let doc = Document::open(&path).unwrap();
    let first = doc.lines(0, 50);
    g.check("open + 先頭 50 行", t.elapsed(), 0.3);
    assert_eq!(first.len(), 50);
    let t = Instant::now();
    wait_index(&doc);
    g.check("全行索引", t.elapsed(), 0.5);
    let doc = Arc::new(doc);
    let (lines, exact) = doc.line_count();
    assert!(exact && lines >= lines_written, "行数 {lines}");

    // 任意の行へのジャンプ（スクロールバーのドラッグ相当）
    let t = Instant::now();
    let mut x = 0x9E3779B97F4A7C15u64;
    for _ in 0..5_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        std::hint::black_box(doc.lines(x % lines, 60));
    }
    g.check("ランダムな行へ 5,000 回ジャンプ", t.elapsed(), 0.5);

    // 検索
    for (name, pat, opts, budget) in [
        ("全件検索：文字列", "req=1234", SearchOptions { case_sensitive: true, regex: false, whole_word: false }, 0.5),
        ("全件検索：大文字小文字を無視", "ILLEGALSTATE", SearchOptions { case_sensitive: false, regex: false, whole_word: false }, 0.5),
        ("全件検索：正規表現", r"items/40\d\d$", SearchOptions { case_sensitive: true, regex: true, whole_word: false }, 0.5),
        ("全件検索：単語単位", "WARN", SearchOptions { case_sensitive: true, regex: false, whole_word: true }, 0.5),
    ] {
        let q = Query::new(&doc, pat, opts).unwrap();
        let t = Instant::now();
        let r = q.find_all(&doc, None, usize::MAX, &ctl);
        g.check(name, t.elapsed(), budget);
        assert!(!r.ranges.is_empty(), "{name}: 一致が 0 件");
    }

    // 時刻での絞り込み
    let range = TimeRange::parse("2026-11-09 05:00 ~ 05:30").unwrap();
    let t = Instant::now();
    let (found, n) = filter_time(&doc, range, 1_000_000, &ctl).unwrap();
    g.check("時刻の範囲で絞り込み", t.elapsed(), 1.0);
    assert!(n > 0 && !found.ranges.is_empty());

    // すべて置換 → 取り消し → 保存
    let q = Query::new(&doc, "INFO", SearchOptions { case_sensitive: true, regex: false, whole_word: true }).unwrap();
    let t = Instant::now();
    let found = q.find_all(&doc, Some("INFORMATION"), usize::MAX, &ctl);
    let mut ed = Editor::new(doc.clone());
    let count = ed.replace_all(&found.ranges, &found.replacements, (0, 0)).unwrap();
    g.check("すべて置換（検索 + 適用）", t.elapsed(), 1.0);
    assert_eq!(ed.doc().len(), doc.len() + count as u64 * 7);
    let t = Instant::now();
    let tmp = ed.doc().write_temp(&dir.join("saved.log"), &AtomicU64::new(0), &AtomicBool::new(false)).unwrap();
    g.check("置換後の保存", t.elapsed(), 3.0);
    assert_eq!(std::fs::metadata(&tmp).unwrap().len(), ed.doc().len());
    let _ = std::fs::remove_file(&tmp);
    let t = Instant::now();
    ed.undo();
    g.check("取り消し", t.elapsed(), 0.5);
    assert_eq!(ed.doc().len(), doc.len());

    // キー入力相当：途中への 1 文字挿入の連続
    let mut ed = Editor::new(doc.clone());
    let mid = doc.line_start(lines / 2).unwrap();
    let t = Instant::now();
    for i in 0..2_000u64 {
        ed.replace(mid + i, mid + i, "x", EditKind::Typing, (mid + i, mid + i)).unwrap();
    }
    g.check("1 文字挿入 2,000 回", t.elapsed(), 0.3);
    let t = Instant::now();
    std::hint::black_box(ed.doc().lines(lines / 2, 60));
    g.check("編集後の 60 行の取り出し", t.elapsed(), 0.05);

    // 巨大な 1 行（minify した JSON 相当）の窓の取り出しと編集
    let mut one = String::from("[");
    for i in 0..120_000 {
        one.push_str(&format!("{{\"id\":{i},\"name\":\"user{i}\"}},"));
    }
    one.push_str("0]\n");
    let long = Arc::new(Document::from_bytes(one.into_bytes()));
    wait_index(&long);
    let (ls, le) = long.line_range(0).unwrap();
    let t = Instant::now();
    for k in 0..2_000u64 {
        std::hint::black_box(long.line_window(0, (le - ls) * k / 2_000, 8_000));
    }
    g.check("長い行：窓の取り出し 2,000 回", t.elapsed(), 0.5);
    let mut ed = Editor::new(long.clone());
    let t = Instant::now();
    for i in 0..500u64 {
        let p = ls + (le - ls) / 2 + i;
        ed.replace(p, p, "x", EditKind::Typing, (p, p)).unwrap();
        std::hint::black_box(ed.doc().line_window(0, (p - ls).saturating_sub(100), 8_000));
    }
    g.check("長い行：入力 500 回 + 窓の取り出し", t.elapsed(), 0.5);

    let _ = std::fs::remove_dir_all(&dir);
    assert!(g.failures.is_empty(), "性能の予算を超えました:\n  {}", g.failures.join("\n  "));
}
