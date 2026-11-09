//! 検索。
//!
//! - 大文字小文字を区別する文字列検索は、ファイルの文字コードに変換したバイト列を
//!   memchr::memmem（SIMD）でそのまま探す。最速。
//! - それ以外（大文字小文字を区別しない・正規表現）は regex クレートを使う。
//!   UTF-8 のファイルはバイト列のまま、それ以外の文字コードはチャンクごとにデコードしてから探す。
//!
//! ファイルは行の境目でチャンクに分け、複数スレッドで並列に探す。
//! 近くにある一致はすぐ返したいので、まず手前の小さな範囲を 1 スレッドで探し、
//! 見つからなければ大きな窓を並列で探す。マッチは 1 行の中に限る。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use memchr::memmem;

use crate::document::Document;
use crate::line_index::{line_start_at_or_after, line_start_at_or_before, next_line_start};

/// 1 スレッドが一度に受け持つ大きさ
const SUB_CHUNK: usize = 8 << 20;
/// デコードしてから探す場合（メモリを抑えるため小さめ）
const SUB_CHUNK_TEXT: usize = 2 << 20;
/// 最初に 1 スレッドで探す範囲
const NEAR: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    Found(u64, u64),
    NotFound,
    Cancelled,
}

#[allow(clippy::large_enum_variant)] // Query は 1 つしか作らないので気にしない
enum Matcher {
    Bytes { fwd: memmem::Finder<'static>, rev: memmem::FinderRev<'static> },
    /// UTF-8 のファイルをバイト列のまま正規表現で探す
    BytesRegex(regex::bytes::Regex),
    /// UTF-8 以外：デコードした文字列に正規表現をかける
    TextRegex(regex::Regex),
}

pub struct Query {
    matcher: Matcher,
}

/// 検索の進み具合とキャンセル
#[derive(Default)]
pub struct SearchControl {
    pub cancel: AtomicBool,
    pub scanned: AtomicU64,
}

impl Query {
    pub fn new(doc: &Document, pattern: &str, opts: SearchOptions) -> Result<Query, String> {
        if pattern.is_empty() {
            return Err("検索語が空です".into());
        }
        let matcher = if !opts.regex && opts.case_sensitive {
            let needle = doc.encode(pattern);
            Matcher::Bytes {
                fwd: memmem::Finder::new(&needle).into_owned(),
                rev: memmem::FinderRev::new(&needle).into_owned(),
            }
        } else {
            let body = if opts.regex { pattern.to_string() } else { regex::escape(pattern) };
            // m: ^ $ を行頭・行末に / R: CRLF の CR を行末として扱う
            let flags = if opts.case_sensitive { "(?mR)" } else { "(?imR)" };
            let src = format!("{flags}{body}");
            let utf8 = doc.encoding().encoding == encoding_rs::UTF_8;
            let build = |s: &str| -> Result<Matcher, String> {
                if utf8 {
                    regex::bytes::RegexBuilder::new(s).size_limit(64 << 20).build().map(Matcher::BytesRegex)
                } else {
                    regex::RegexBuilder::new(s).size_limit(64 << 20).build().map(Matcher::TextRegex)
                }
                .map_err(|e| format!("正規表現の誤り: {e}"))
            };
            build(&src)?
        };
        Ok(Query { matcher })
    }

    fn sub_chunk(&self) -> usize {
        if matches!(self.matcher, Matcher::TextRegex(_)) { SUB_CHUNK_TEXT } else { SUB_CHUNK }
    }

    /// `from` 以降（前方）または `from` より前（後方）で最初の一致を探す
    pub fn find(&self, doc: &Document, from: u64, dir: Direction, ctl: &SearchControl) -> SearchOutcome {
        match dir {
            Direction::Forward => self.find_forward(doc, from as usize, ctl),
            Direction::Backward => self.find_backward(doc, from as usize, ctl),
        }
    }

    fn find_forward(&self, doc: &Document, from: usize, ctl: &SearchControl) -> SearchOutcome {
        let data = doc.data();
        let (floor, kind) = (doc.text_start(), doc.newline());
        let len = data.len();
        let from = from.max(floor);
        if from >= len {
            return SearchOutcome::NotFound;
        }
        // 近くは 1 スレッドで
        let cs = line_start_at_or_before(data, from, floor, kind);
        let near_end = line_start_at_or_after(data, from + NEAR, floor, kind);
        if let Some(hit) = self.chunk(doc, cs, near_end, from, near_end) {
            return SearchOutcome::Found(hit.0 as u64, hit.1 as u64);
        }
        ctl.scanned.fetch_add((near_end - from) as u64, Ordering::Relaxed);

        let threads = threads();
        let mut pos = near_end;
        while pos < len {
            if ctl.cancel.load(Ordering::Relaxed) {
                return SearchOutcome::Cancelled;
            }
            let win_end = line_start_at_or_after(data, pos + self.sub_chunk() * threads, floor, kind);
            let ranges = split(data, pos, win_end, floor, doc, self.sub_chunk());
            let results = run_parallel(&ranges, |&(a, b)| {
                if ctl.cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let r = self.chunk(doc, a, b, a, b);
                ctl.scanned.fetch_add((b - a) as u64, Ordering::Relaxed);
                r
            });
            if let Some(hit) = results.into_iter().flatten().min_by_key(|h| h.0) {
                return SearchOutcome::Found(hit.0 as u64, hit.1 as u64);
            }
            pos = win_end;
        }
        if ctl.cancel.load(Ordering::Relaxed) { SearchOutcome::Cancelled } else { SearchOutcome::NotFound }
    }

    fn find_backward(&self, doc: &Document, to: usize, ctl: &SearchControl) -> SearchOutcome {
        let data = doc.data();
        let (floor, kind) = (doc.text_start(), doc.newline());
        let to = to.min(data.len());
        if to <= floor {
            return SearchOutcome::NotFound;
        }
        // `to` を含む行の終わりまでを上端にする（一致は `to` より前で始まればよい）
        let top = next_line_start(data, to.saturating_sub(1), kind).unwrap_or(data.len()).max(to);
        let near_start = line_start_at_or_before(data, to.saturating_sub(NEAR), floor, kind);
        if let Some(hit) = self.chunk_last(doc, near_start, top, near_start, to) {
            return SearchOutcome::Found(hit.0 as u64, hit.1 as u64);
        }
        ctl.scanned.fetch_add((to - near_start) as u64, Ordering::Relaxed);

        let threads = threads();
        let mut pos = near_start;
        while pos > floor {
            if ctl.cancel.load(Ordering::Relaxed) {
                return SearchOutcome::Cancelled;
            }
            let win_start = line_start_at_or_before(data, pos.saturating_sub(self.sub_chunk() * threads), floor, kind);
            let ranges = split(data, win_start, pos, floor, doc, self.sub_chunk());
            let results = run_parallel(&ranges, |&(a, b)| {
                if ctl.cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let r = self.chunk_last(doc, a, b, a, b);
                ctl.scanned.fetch_add((b - a) as u64, Ordering::Relaxed);
                r
            });
            if let Some(hit) = results.into_iter().flatten().max_by_key(|h| h.0) {
                return SearchOutcome::Found(hit.0 as u64, hit.1 as u64);
            }
            pos = win_start;
        }
        if ctl.cancel.load(Ordering::Relaxed) { SearchOutcome::Cancelled } else { SearchOutcome::NotFound }
    }

    /// [cs, ce) の中で、開始位置が [lo, hi) にある最初の一致
    fn chunk(&self, doc: &Document, cs: usize, ce: usize, lo: usize, hi: usize) -> Option<(usize, usize)> {
        let data = doc.data();
        let utf16 = doc.encoding().is_utf16();
        match &self.matcher {
            Matcher::Bytes { fwd, .. } => {
                let n = fwd.needle().len();
                let mut pos = lo;
                while pos < hi {
                    let p = pos + fwd.find(&data[pos..ce])?;
                    if p >= hi {
                        return None;
                    }
                    if !utf16 || p.is_multiple_of(2) {
                        return Some((p, p + n));
                    }
                    pos = p + 1;
                }
                None
            }
            Matcher::BytesRegex(re) => {
                let m = re.find_at(&data[cs..ce], lo - cs)?;
                (cs + m.start() < hi).then(|| (cs + m.start(), cs + m.end()))
            }
            Matcher::TextRegex(re) => {
                let text = doc.decode(&data[cs..ce]);
                let dlo = if lo > cs { doc.decode(&data[cs..lo]).len() } else { 0 };
                let dhi = if hi < ce { doc.decode(&data[cs..hi]).len() } else { text.len() };
                let m = re.find_at(&text, floor_char(&text, dlo))?;
                (m.start() < dhi).then(|| map_back(doc, cs, &text, m.start(), m.end()))
            }
        }
    }

    /// [cs, ce) の中で、開始位置が [lo, hi) にある最後の一致
    fn chunk_last(&self, doc: &Document, cs: usize, ce: usize, lo: usize, hi: usize) -> Option<(usize, usize)> {
        let data = doc.data();
        let utf16 = doc.encoding().is_utf16();
        match &self.matcher {
            Matcher::Bytes { rev, .. } => {
                let n = rev.needle().len();
                let mut end = (hi + n - 1).min(ce);
                while end > lo {
                    let p = lo + rev.rfind(&data[lo..end])?;
                    if !utf16 || p.is_multiple_of(2) {
                        return Some((p, p + n));
                    }
                    end = p + n - 1;
                }
                None
            }
            Matcher::BytesRegex(re) => {
                let hay = &data[cs..ce];
                let mut last = None;
                let mut at = lo - cs;
                while at <= hay.len() {
                    let Some(m) = re.find_at(hay, at) else { break };
                    if cs + m.start() >= hi {
                        break;
                    }
                    last = Some((cs + m.start(), cs + m.end()));
                    at = if m.end() > m.start() { m.end() } else { m.start() + 1 };
                }
                last
            }
            Matcher::TextRegex(re) => {
                let text = doc.decode(&data[cs..ce]);
                let dlo = if lo > cs { doc.decode(&data[cs..lo]).len() } else { 0 };
                let dhi = if hi < ce { doc.decode(&data[cs..hi]).len() } else { text.len() };
                let mut last = None;
                let mut at = floor_char(&text, dlo);
                while at <= text.len() {
                    let Some(m) = re.find_at(&text, at) else { break };
                    if m.start() >= dhi {
                        break;
                    }
                    last = Some((m.start(), m.end()));
                    at = if m.end() > m.start() { m.end() } else { ceil_char(&text, m.start() + 1) };
                }
                last.map(|(s, e)| map_back(doc, cs, &text, s, e))
            }
        }
    }
}

/// デコード後の位置を、元ファイルのバイト位置に戻す（再エンコードした長さで求める）
fn map_back(doc: &Document, cs: usize, text: &str, s: usize, e: usize) -> (usize, usize) {
    let start = cs + doc.encode(&text[..s]).len();
    (start, start + doc.encode(&text[s..e]).len())
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// [a, b) を行の境目でおおよそ `size` ずつに分ける
fn split(data: &[u8], a: usize, b: usize, floor: usize, doc: &Document, size: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = a;
    while s < b {
        let e = line_start_at_or_after(data, s + size, floor, doc.newline()).min(b).max(s + 1);
        out.push((s, e));
        s = e;
    }
    out
}

fn run_parallel<T, F>(ranges: &[(usize, usize)], f: F) -> Vec<Option<T>>
where
    T: Send,
    F: Fn(&(usize, usize)) -> Option<T> + Sync,
{
    if ranges.len() <= 1 {
        return ranges.iter().map(&f).collect();
    }
    thread::scope(|s| {
        let handles: Vec<_> = ranges.iter().map(|r| s.spawn(|| f(r))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or(None)).collect()
    })
}

fn threads() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bytes: Vec<u8>) -> Document {
        let d = Document::from_bytes(bytes);
        while !d.progress().done {
            thread::yield_now();
        }
        d
    }

    fn all(d: &Document, pat: &str, opts: SearchOptions, dir: Direction) -> Vec<(u64, u64)> {
        let q = Query::new(d, pat, opts).unwrap();
        let ctl = SearchControl::default();
        let mut out = Vec::new();
        let mut pos = if dir == Direction::Forward { 0 } else { d.len() };
        loop {
            match q.find(d, pos, dir, &ctl) {
                SearchOutcome::Found(s, e) => {
                    out.push((s, e));
                    pos = match dir {
                        Direction::Forward => e.max(s + 1),
                        Direction::Backward => s,
                    };
                }
                _ => break,
            }
        }
        out
    }

    fn naive(text: &str, pat: &str) -> Vec<usize> {
        text.match_indices(pat).map(|(i, _)| i).collect()
    }

    const CS: SearchOptions = SearchOptions { case_sensitive: true, regex: false };
    const CI: SearchOptions = SearchOptions { case_sensitive: false, regex: false };
    const RE: SearchOptions = SearchOptions { case_sensitive: true, regex: true };

    #[test]
    fn forward_and_backward_literal() {
        let text: String = (0..20000).map(|i| format!("row {i} {}\n", if i % 97 == 0 { "ERROR here" } else { "ok" })).collect();
        let d = doc(text.clone().into_bytes());
        let want: Vec<_> = naive(&text, "ERROR").into_iter().map(|i| (i as u64, i as u64 + 5)).collect();
        assert_eq!(all(&d, "ERROR", CS, Direction::Forward), want);
        let mut rev = want.clone();
        rev.reverse();
        assert_eq!(all(&d, "ERROR", CS, Direction::Backward), rev);
        // 大文字小文字を区別しない
        assert_eq!(all(&d, "error", CI, Direction::Forward), want);
        assert_eq!(all(&d, "error", CS, Direction::Forward), vec![]);
    }

    #[test]
    fn regex_line_anchors_crlf() {
        let d = doc(b"abc\r\nxabc\r\nabc\r\n".to_vec());
        assert_eq!(all(&d, "^abc$", RE, Direction::Forward), vec![(0, 3), (11, 14)]);
        assert_eq!(all(&d, "^abc$", RE, Direction::Backward), vec![(11, 14), (0, 3)]);
        assert!(Query::new(&d, "(", RE).is_err());
    }

    #[test]
    fn sjis_regex_and_case_insensitive() {
        let text = "一行目 OK\r\n二行目 エラー発生 Error=5\r\n三行目\r\n".repeat(50);
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
        let bytes = bytes.into_owned();
        let d = doc(bytes.clone());
        assert_eq!(d.encoding().encoding, encoding_rs::SHIFT_JIS);
        let lit = all(&d, "エラー", CS, Direction::Forward);
        assert_eq!(lit.len(), 50);
        let re = all(&d, "エラー.生", RE, Direction::Forward);
        assert_eq!(re.len(), 50);
        for (&(s1, _), &(s2, e2)) in lit.iter().zip(&re) {
            assert_eq!(s1, s2);
            assert_eq!(&bytes[s2 as usize..e2 as usize], &*encoding_rs::SHIFT_JIS.encode("エラー発生").0);
        }
        let ci = all(&d, "error=\\d", SearchOptions { case_sensitive: false, regex: true }, Direction::Backward);
        assert_eq!(ci.len(), 50);
        assert_eq!(&bytes[ci[0].0 as usize..ci[0].1 as usize], b"Error=5");
    }

    #[test]
    fn utf16_alignment() {
        let mut v = vec![0xFF, 0xFE];
        v.extend("aaa\r\nあaa\r\n".repeat(30).encode_utf16().flat_map(|u| u.to_le_bytes()));
        let d = doc(v);
        assert_eq!(all(&d, "aa", CS, Direction::Forward).len(), 60);
        assert_eq!(all(&d, "あa", CI, Direction::Backward).len(), 30);
    }

    #[test]
    fn big_parallel_windows() {
        // SUB_CHUNK を何度もまたぐ大きさ
        let line = "0123456789abcdefghijklmnopqrstuvwxyz\n";
        let mut text = line.repeat((SUB_CHUNK * 3) / line.len());
        let mid = text.len() / 2;
        let mid = mid - mid % line.len();
        text.replace_range(mid..mid + 6, "NEEDLE");
        text.push_str("NEEDLE at end\n");
        let d = doc(text.into_bytes());
        for opts in [CS, CI, RE] {
            let f = all(&d, "NEEDLE", opts, Direction::Forward);
            assert_eq!(f.len(), 2, "{opts:?}");
            assert_eq!(f[0].0, mid as u64);
            let b = all(&d, "NEEDLE", opts, Direction::Backward);
            assert_eq!(b, f.iter().rev().cloned().collect::<Vec<_>>());
        }
    }
}
