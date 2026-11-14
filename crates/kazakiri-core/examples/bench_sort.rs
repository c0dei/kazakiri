//! CSV 並べ替えの速度を測る（編集の反映まで）。`cargo run --release --example bench_sort [CSV]`
use kazakiri_core::csv::{self, SortOptions};
use kazakiri_core::{Document, EditKind, Editor, SearchControl};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let mut data = String::from("id,name,score,city,memo\r\n");
            let mut x: u64 = 88172645463325252;
            for i in 0..2_000_000u32 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                data.push_str(&format!("{i},User {},{},Tokyo,some memo text {}\r\n", x % 100000, x % 977, i));
            }
            let path = std::env::temp_dir().join("kazakiri_bench_sort.csv");
            std::fs::write(&path, &data).unwrap();
            path
        }
    };
    let doc = Arc::new(Document::open(&path).unwrap());
    while !doc.progress().done {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    println!("size {} KB", doc.len() >> 10);
    let ctl = SearchControl::default();
    for col in [0usize, 1, 4] {
        let t = Instant::now();
        let out = csv::sort_document(&doc, SortOptions { column: col, delim: ',', descending: false, header: true }, &ctl).unwrap();
        println!("sort col {col}: {:?}", t.elapsed());
        let mut ed = Editor::new(doc.clone());
        let (a, b) = (doc.text_start() as u64, doc.len());
        let t = Instant::now();
        ed.replace_bytes(a, b, out, EditKind::Other, (a, a)).unwrap();
        println!("  replace_bytes: {:?}", t.elapsed());
        let t = Instant::now();
        let d = ed.doc();
        println!("  line_count {:?} -> {:?}", d.line_count(), t.elapsed());
        // 編集した後の文書をもう一度並べ替える
        let t = Instant::now();
        let again = csv::sort_document(d, SortOptions { column: 3, delim: ',', descending: true, header: true }, &ctl).unwrap();
        println!("  sort again (edited doc): {:?} ({} KB)", t.elapsed(), again.len() >> 10);
    }
}
