//! 時刻での絞り込み・移動の計測：`cargo run --release --example bench_time -- ファイル "2026-11-09 19:30 ~ 19:31"`

use std::time::Instant;

use kazakiri_core::logtime::{filter_time, find_time_line, TimeRange};
use kazakiri_core::{Document, SearchControl};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let t = Instant::now();
    let doc = Document::open(std::path::Path::new(&args[1])).unwrap();
    while !doc.progress().done {
        std::thread::yield_now();
    }
    println!("open + index: {:?}  lines={}", t.elapsed(), doc.line_count().0);
    let range = TimeRange::parse(&args[2]).unwrap();
    let t = Instant::now();
    let (found, lines) = filter_time(&doc, range, 1_000_000, &SearchControl::default()).unwrap();
    println!("filter_time: {:?}  blocks={} lines={lines}", t.elapsed(), found.ranges.len());
    let t = Instant::now();
    let l = find_time_line(&doc, range.from);
    println!("find_time_line: {:?}  line={l:?}", t.elapsed());
}
