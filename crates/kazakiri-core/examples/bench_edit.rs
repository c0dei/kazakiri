//! 編集の速度を測る（断片が増えたときの 1 回あたりの時間）。`cargo run --release --example bench_edit`
use kazakiri_core::{Document, EditKind, Editor};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut data = Vec::new();
    for i in 0..2_000_000u32 {
        data.extend_from_slice(format!("line {i} some log text here\n").as_bytes());
    }
    println!("size {} MB", data.len() >> 20);
    let path = std::env::temp_dir().join("kazakiri_bench_edit.txt");
    std::fs::write(&path, &data).unwrap();
    let doc = Arc::new(Document::open(&path).unwrap());
    while !doc.progress().done {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut ed = Editor::new(doc);
    let mut x: u64 = 88172645463325252;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for round in [1_000usize, 10_000, 50_000] {
        let t = Instant::now();
        for _ in 0..round {
            let len = ed.doc().len();
            let pos = next() % len;
            ed.replace(pos, pos, "x", EditKind::Other, (pos, pos)).unwrap();
        }
        let el = t.elapsed();
        println!("+{round} edits: {:?} ({:.1} µs/edit)  pieces={}", el, el.as_micros() as f64 / round as f64, ed.doc().piece_count());
    }
    let t = Instant::now();
    let mut s = 0u64;
    for _ in 0..20_000 {
        let l = next() % ed.doc().line_count().0;
        s += ed.doc().line_range(l).map_or(0, |r| r.0);
    }
    println!("20k line_range lookups: {:?} (sum {s})", t.elapsed());
    let _ = std::fs::remove_file(&path);
}
