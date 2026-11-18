//! ログの時刻での絞り込みと移動。行の先頭付近にある `2026-11-18 12:34:56`（`/` 区切り、`T` 区切り、秒なしも可）を時刻として読む。
//!
//! 時刻のない行（スタックトレースなど）は、直前の時刻のある行の続きとして扱い、同じ扱い（範囲内／外）にする。
//! 時刻の比較は、`yyyymmddhhmmss` の数値で行う（日時の計算はしない）。

use std::sync::atomic::Ordering as AO;

use crate::document::Document;
use crate::search::{run_parallel, threads, FindAll, SearchControl};

/// 行の先頭から、時刻を探す範囲（バイト）
const SCAN_BYTES: usize = 64;

fn digits(b: &[u8], i: usize, n: usize) -> Option<u64> {
    let s = b.get(i..i + n)?;
    s.iter().try_fold(0u64, |v, &c| c.is_ascii_digit().then(|| v * 10 + (c - b'0') as u64))
}

/// `YYYY-MM-DD` の形（区切りは `-` か `/`）を `yyyymmdd` に。続きの位置も返す
fn date_at(b: &[u8]) -> Option<(u64, usize)> {
    let y = digits(b, 0, 4)?;
    let sep = *b.get(4)?;
    if sep != b'-' && sep != b'/' {
        return None;
    }
    let m = digits(b, 5, 2)?;
    if *b.get(7)? != sep {
        return None;
    }
    let d = digits(b, 8, 2)?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y * 10000 + m * 100 + d, 10))
}

/// 時刻の (時, 分, 秒（なければ None））
type Hms = (u64, u64, Option<u64>);

/// `HH:MM` か `HH:MM:SS` を読む。続きの位置も返す
fn time_at(b: &[u8]) -> Option<(Hms, usize)> {
    let h = digits(b, 0, 2)?;
    if *b.get(2)? != b':' {
        return None;
    }
    let m = digits(b, 3, 2)?;
    if h >= 24 || m >= 60 {
        return None;
    }
    if b.get(5) == Some(&b':') {
        if let Some(s) = digits(b, 6, 2) {
            return (s <= 60).then_some(((h, m, Some(s.min(59))), 8));
        }
    }
    Some(((h, m, None), 5))
}

fn key(date: u64, h: u64, m: u64, s: u64) -> u64 {
    date * 1_000_000 + h * 10_000 + m * 100 + s
}

/// 行の中の時刻（先頭の 64 バイトまでに現れる最初のもの）を `yyyymmddhhmmss` の数値で返す
pub fn parse_timestamp(line: &str) -> Option<u64> {
    let b = &line.as_bytes()[..line.len().min(SCAN_BYTES)];
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && b[i - 1].is_ascii_digit()) {
            continue;
        }
        let Some((date, n)) = date_at(&b[i..]) else { continue };
        let rest = &b[i + n..];
        if !matches!(rest.first(), Some(b' ' | b'T')) {
            continue;
        }
        if let Some(((h, m, s), _)) = time_at(&rest[1..]) {
            return Some(key(date, h, m, s.unwrap_or(0)));
        }
    }
    None
}

/// 時刻の範囲（両端を含む）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub from: u64,
    pub to: u64,
}

/// 範囲の片側（日付と時刻のどちらかが欠けていてもよい）
struct Partial {
    date: Option<u64>,
    time: Option<Hms>,
}

fn parse_partial(s: &str) -> Result<Partial, String> {
    let s = s.trim();
    let bad = || format!("時刻を読めません: 「{s}」（例: 2026-11-18 12:30:00、2026-11-18、12:30）");
    let b = s.as_bytes();
    if let Some((d, n)) = date_at(b) {
        let rest = s[n..].trim_start_matches([' ', 'T']);
        if rest.is_empty() {
            return Ok(Partial { date: Some(d), time: None });
        }
        let (t, used) = time_at(rest.as_bytes()).ok_or_else(bad)?;
        return if used == rest.len() { Ok(Partial { date: Some(d), time: Some(t) }) } else { Err(bad()) };
    }
    match time_at(b) {
        Some((t, used)) if used == b.len() => Ok(Partial { date: None, time: Some(t) }),
        _ => Err(bad()),
    }
}

impl TimeRange {
    /// 「開始 ～ 終了」を読む（区切りは `～` `〜` `~` `..` ` to `）。
    /// 日付だけなら、その日の 0 時から（終了なら 23:59:59 まで）、秒がなければ 00 秒から（終了なら 59 秒まで）。
    /// 片方を空にすると、そちら側は無制限。時刻だけを書いた側は、もう一方の日付を使う。
    /// 区切りがなければ、その日・その分など、書いた精度の全体を範囲にする
    pub fn parse(spec: &str) -> Result<TimeRange, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("時刻の範囲を入力してください（例: 2026-11-18 12:00 ～ 2026-11-18 13:30）".into());
        }
        let parts = ["～", "〜", "~", "..", " to "].iter().find_map(|sep| spec.split_once(sep));
        let (pa, pb) = match parts {
            None => (Some(parse_partial(spec)?), Some(parse_partial(spec)?)),
            Some((a, b)) => ((!a.trim().is_empty()).then(|| parse_partial(a)).transpose()?, (!b.trim().is_empty()).then(|| parse_partial(b)).transpose()?),
        };
        if pa.is_none() && pb.is_none() {
            return Err("開始と終了の少なくとも片方を入力してください".into());
        }
        // 日付のない側は、もう一方の日付を借りる
        let date_of = |p: &Option<Partial>| p.as_ref().and_then(|p| p.date);
        let shared = date_of(&pa).or(date_of(&pb));
        let resolve = |p: Option<Partial>, end: bool| -> Result<Option<u64>, String> {
            let Some(p) = p else { return Ok(None) };
            let date = p.date.or(shared).ok_or("日付がありません（片方に日付を書いてください）")?;
            let (h, m, s) = match p.time {
                Some((h, m, s)) => (h, m, s.unwrap_or(if end { 59 } else { 0 })),
                None => (if end { 23 } else { 0 }, if end { 59 } else { 0 }, if end { 59 } else { 0 }),
            };
            Ok(Some(key(date, h, m, s)))
        };
        let from = resolve(pa, false)?.unwrap_or(0);
        let to = resolve(pb, true)?.unwrap_or(u64::MAX);
        if from > to {
            return Err("開始が終了より後です".into());
        }
        Ok(TimeRange { from, to })
    }

    pub fn contains(&self, t: u64) -> bool {
        (self.from..=self.to).contains(&t)
    }
}

/// 範囲に入る行のまとまり（連続した行）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    /// 先頭と最後の行番号（最後を含む）
    l0: u64,
    l1: u64,
    /// 先頭行の頭から、最後の行の末尾（改行を含まない）まで
    s: u64,
    e: u64,
}

#[derive(Default)]
struct Chunk {
    /// 時刻のある行より前の行（前のチャンクの続き）
    lead: Option<Block>,
    blocks: Vec<Block>,
    /// 最後の行の扱い（時刻のある行がなければ None）
    last: Option<bool>,
}

fn push_block(out: &mut Vec<Block>, b: Block) {
    match out.last_mut() {
        Some(p) if p.l1 + 1 == b.l0 => {
            p.l1 = b.l1;
            p.e = b.e;
        }
        _ => out.push(b),
    }
}

/// 範囲に入る行（と、その続きの行）を、連続した行のまとまりごとの範囲 [先頭行の頭, 最後の行の末尾) で返す。
/// 戻り値の 2 つ目は、範囲に入った行の数。`limit` はまとまりの数の上限。行索引ができあがっていないときは Err
pub fn filter_time(doc: &Document, range: TimeRange, limit: usize, ctl: &SearchControl) -> Result<(FindAll, u64), String> {
    let (count, exact) = doc.line_count();
    if !exact {
        return Err("索引の作成中は時刻での絞り込みを使えません".into());
    }
    const CHUNK: u64 = 20_000;
    let chunks: Vec<(usize, usize)> = (0..count.div_ceil(CHUNK)).map(|i| ((i * CHUNK) as usize, ((i + 1) * CHUNK).min(count) as usize)).collect();
    let mut out = FindAll::default();
    let mut blocks: Vec<Block> = Vec::new();
    let mut prev = false;
    for window in chunks.chunks(threads()) {
        if ctl.cancel.load(AO::Relaxed) {
            out.cancelled = true;
            break;
        }
        let parts = run_parallel(window, |&(a, b)| {
            if ctl.cancel.load(AO::Relaxed) {
                return None;
            }
            let mut c = Chunk::default();
            let mut cur: Option<bool> = None;
            for line in doc.lines(a as u64, b - a) {
                if let Some(t) = parse_timestamp(&line.text) {
                    cur = Some(range.contains(t));
                }
                let blk = Block { l0: line.number, l1: line.number, s: line.start, e: line.end };
                match cur {
                    None => match &mut c.lead {
                        Some(l) => {
                            l.l1 = blk.l1;
                            l.e = blk.e;
                        }
                        None => c.lead = Some(blk),
                    },
                    Some(true) => push_block(&mut c.blocks, blk),
                    Some(false) => {}
                }
            }
            c.last = cur;
            ctl.scanned.fetch_add((b - a) as u64, AO::Relaxed);
            Some(c)
        });
        for c in parts.into_iter().flatten() {
            if let (Some(l), true) = (c.lead, prev) {
                push_block(&mut blocks, l);
            }
            for b in c.blocks {
                push_block(&mut blocks, b);
            }
            prev = c.last.unwrap_or(prev);
        }
        if blocks.len() > limit {
            blocks.truncate(limit);
            out.truncated = true;
            break;
        }
    }
    let lines = blocks.iter().map(|b| b.l1 - b.l0 + 1).sum();
    out.ranges = blocks.iter().map(|b| (b.s, b.e)).collect();
    Ok((out, lines))
}

/// `line` 以降で最初に時刻のある行の時刻（最大 64 行先まで見る）
fn time_from(doc: &Document, line: u64) -> Option<u64> {
    doc.lines(line, 64).iter().find_map(|l| parse_timestamp(&l.text))
}

/// 時刻が `target`（`yyyymmddhhmmss`）以上になる最初の行。ログが時刻順に並んでいる前提の二分探索なので、数GB でも一瞬
pub fn find_time_line(doc: &Document, target: u64) -> Option<u64> {
    let (count, _) = doc.line_count();
    let (mut lo, mut hi) = (0u64, count);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match time_from(doc, mid) {
            Some(t) if t >= target => hi = mid,
            _ => lo = mid + 1,
        }
    }
    // 続きの行（時刻のない行）の途中なら、次の時刻のある行まで進める
    while lo < count {
        let l = doc.lines(lo, 1);
        match l.first() {
            Some(x) if parse_timestamp(&x.text).is_none() => lo += 1,
            Some(_) => return Some(lo),
            None => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        let d = Document::from_bytes(text.as_bytes().to_vec());
        while !d.progress().done {
            std::thread::yield_now();
        }
        d
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_timestamp("2026-11-18 12:34:56 INFO x"), Some(20261118123456));
        assert_eq!(parse_timestamp("[2026/11/18T01:02] WARN"), Some(20261118010200));
        assert_eq!(parse_timestamp("ts=2026-11-18 23:59:60 x"), Some(20261118235959));
        assert_eq!(parse_timestamp("    at Foo.bar(Foo.java:12)"), None);
        assert_eq!(parse_timestamp("2026-13-18 12:00:00"), None);
        assert_eq!(parse_timestamp("12026-11-18 12:00:00x"), None);
        assert_eq!(parse_timestamp("2026-11-18 25:00:00"), None);
    }

    #[test]
    fn parses_ranges() {
        let r = TimeRange::parse("2026-11-18 12:00 ～ 2026-11-18 13:30").unwrap();
        assert_eq!((r.from, r.to), (20261118120000, 20261118133059));
        // 日付だけ：その日全体
        let r = TimeRange::parse("2026-11-18").unwrap();
        assert_eq!((r.from, r.to), (20261118000000, 20261118235959));
        // 時刻だけの側は、もう一方の日付を借りる
        let r = TimeRange::parse("2026-11-18 12:00:00 .. 13:00:00").unwrap();
        assert_eq!((r.from, r.to), (20261118120000, 20261118130000));
        // 片側だけ
        let r = TimeRange::parse("2026-11-18 12:00 ~").unwrap();
        assert_eq!((r.from, r.to), (20261118120000, u64::MAX));
        let r = TimeRange::parse("~ 2026-11-18 12:00").unwrap();
        assert_eq!((r.from, r.to), (0, 20261118120059));
        assert!(TimeRange::parse("12:00 ~ 13:00").is_err());
        assert!(TimeRange::parse("2026-11-18 13:00 ~ 2026-11-18 12:00").is_err());
        assert!(TimeRange::parse("きのう").is_err());
        assert!(TimeRange::parse("").is_err());
    }

    fn sample() -> String {
        let mut s = String::new();
        for m in 0..10 {
            s.push_str(&format!("2026-11-18 12:{m:02}:00 INFO line {m}\n"));
            if m == 5 {
                s.push_str("Exception: boom\n    at a.b(c)\n");
            }
        }
        s
    }

    #[test]
    fn filter_keeps_continuation_lines() {
        let d = doc(&sample());
        let r = TimeRange::parse("2026-11-18 12:04 ~ 2026-11-18 12:06").unwrap();
        let (found, lines) = filter_time(&d, r, 1000, &SearchControl::default()).unwrap();
        // 12:04, 12:05（＋続きの 2 行）, 12:06 が 1 つのまとまり
        assert_eq!(found.ranges.len(), 1);
        assert_eq!(lines, 5);
        let (s, e) = found.ranges[0];
        let text = d.decode(&d.read(s, e)).into_owned();
        assert!(text.starts_with("2026-11-18 12:04:00") && text.ends_with("12:06:00 INFO line 6"), "{text}");
        assert!(text.contains("at a.b(c)"));
        // 範囲に入らなければ空
        let r = TimeRange::parse("2027-01-01").unwrap();
        assert!(filter_time(&d, r, 1000, &SearchControl::default()).unwrap().0.ranges.is_empty());
        // 離れた 2 か所は、別のまとまり
        let r = TimeRange { from: 20261118120000, to: 20261118120059 };
        let (f, n) = filter_time(&d, r, 1000, &SearchControl::default()).unwrap();
        assert_eq!((f.ranges.len(), n), (1, 1));
    }

    #[test]
    fn filter_across_chunks() {
        // チャンクの境目（20,000 行）をまたぐ範囲と、境目で続きの行が始まる場合
        let mut s = String::new();
        for i in 0..60_000u64 {
            s.push_str(&format!("2026-11-18 {:02}:{:02}:{:02} L{i}\n", i / 3600 % 24, i / 60 % 60, i % 60));
            if i == 19_999 {
                s.push_str("  continued\n");
            }
        }
        let d = doc(&s);
        let r = TimeRange::parse("2026-11-18 05:00:00 ~ 2026-11-18 05:59:59").unwrap();
        let (f, n) = filter_time(&d, r, 1000, &SearchControl::default()).unwrap();
        // 19,999 行目の時刻は 05:33:19 なので、続きの行も入る（3600 + 1 行）
        assert_eq!((f.ranges.len(), n), (1, 3601));
        let r = TimeRange::parse("2026-11-18 05:33:19 ~ 2026-11-18 05:33:20").unwrap();
        let (_, n) = filter_time(&d, r, 1000, &SearchControl::default()).unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn finds_time_line_by_bisection() {
        let d = doc(&sample());
        assert_eq!(find_time_line(&d, 20261118120300), Some(3));
        assert_eq!(find_time_line(&d, 20261118120500), Some(5));
        // 12:06 の行は、続きの 2 行のあと（行番号 8）
        assert_eq!(find_time_line(&d, 20261118120600), Some(8));
        assert_eq!(find_time_line(&d, 20270101000000), None);
        assert_eq!(find_time_line(&d, 0), Some(0));
    }
}
