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
    /// 正規表現モードか（置換後の文字列で $1 などを展開するか）
    regex_mode: bool,
}

/// 全件検索の結果
#[derive(Debug, Default)]
pub struct FindAll {
    /// 一致の [開始, 終了)。位置の順
    pub ranges: Vec<(u64, u64)>,
    /// 置換後のバイト列（置換するときだけ）。
    /// 正規表現の展開があれば一致ごと（`ranges` と同じ数）、なければ全件共通の 1 つだけ
    pub replacements: Vec<Vec<u8>>,
    /// 上限に達して打ち切ったか
    pub truncated: bool,
    pub cancelled: bool,
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
        Ok(Query { matcher, regex_mode: opts.regex })
    }

    /// 置換後の文字列を、一致ごとのバイト列にする準備。
    /// 正規表現モードでは `$1` `${name}` `$$` を展開する。それ以外はそのまま
    fn template(&self, doc: &Document, replacement: &str) -> Template {
        if self.regex_mode && replacement.contains('$') {
            Template::Expand(replacement.to_string())
        } else {
            Template::Fixed(doc.encode(replacement))
        }
    }

    /// すべての一致を探す（`replacement` を渡すと置換後のバイト列も作る）。
    /// 一致が `limit` 件を超えたら打ち切る
    pub fn find_all(&self, doc: &Document, replacement: Option<&str>, limit: usize, ctl: &SearchControl) -> FindAll {
        self.find_all_in(doc, replacement, limit, ctl, None)
    }

    /// `find_all` を、バイト範囲 `within`（なければ全体）の中だけに限る。
    /// 範囲をまたぐ一致は含めない。範囲の端は、行頭・行末とは限らない
    pub fn find_all_in(&self, doc: &Document, replacement: Option<&str>, limit: usize, ctl: &SearchControl, within: Option<(u64, u64)>) -> FindAll {
        let tmpl = replacement.map(|r| self.template(doc, r));
        let (floor, len) = match within {
            Some((a, b)) => ((a as usize).max(doc.text_start()), (b as usize).min(doc.len() as usize)),
            None => (doc.text_start(), doc.len() as usize),
        };
        let ranges = split(doc, floor, len, self.sub_chunk());
        let total = std::sync::atomic::AtomicUsize::new(0);
        let threads = threads();
        let mut out = FindAll::default();
        if let Some(Template::Fixed(b)) = &tmpl {
            out.replacements.push(b.clone());
        }
        for window in ranges.chunks(threads) {
            if ctl.cancel.load(Ordering::Relaxed) {
                out.cancelled = true;
                break;
            }
            let parts = run_parallel(window, |&(a, b)| {
                if ctl.cancel.load(Ordering::Relaxed) || total.load(Ordering::Relaxed) > limit {
                    return None;
                }
                let r = self.chunk_all(doc, a, b, tmpl.as_ref());
                total.fetch_add(r.len(), Ordering::Relaxed);
                ctl.scanned.fetch_add((b - a) as u64, Ordering::Relaxed);
                Some(r)
            });
            for part in parts.into_iter().flatten() {
                for (s, e, rep) in part {
                    out.ranges.push((s as u64, e as u64));
                    if let Some(rep) = rep {
                        out.replacements.push(rep);
                    }
                }
            }
            if out.ranges.len() > limit {
                out.ranges.truncate(limit);
                if out.replacements.len() > 1 {
                    out.replacements.truncate(limit);
                }
                out.truncated = true;
                break;
            }
        }
        if ctl.cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
        }
        out
    }

    /// [s, e) がいまも一致しているなら、その置換後のバイト列（1 件ずつ置換するとき用）
    pub fn replacement_at(&self, doc: &Document, s: u64, e: u64, replacement: &str) -> Option<Vec<u8>> {
        let tmpl = self.template(doc, replacement);
        let ls = doc.line_start_at_or_before(s) as usize;
        let le = (doc.line_start_at_or_after(e.max(s + 1)) as usize).max(e as usize);
        let (_, _, r) = self.chunk_all(doc, ls, le, Some(&tmpl)).into_iter().find(|&(ms, me, _)| ms as u64 == s && me as u64 == e)?;
        match (r, tmpl) {
            (Some(r), _) => Some(r),
            (None, Template::Fixed(b)) => Some(b),
            (None, Template::Expand(_)) => None,
        }
    }

    /// [cs, ce) の中のすべての一致（と置換後のバイト列）
    fn chunk_all(&self, doc: &Document, cs: usize, ce: usize, tmpl: Option<&Template>) -> Vec<(usize, usize, Option<Vec<u8>>)> {
        let buf = doc.read(cs as u64, ce as u64);
        let utf16 = doc.encoding().is_utf16();
        let mut out = Vec::new();
        // 全件共通の置換は一致ごとに複製しない（呼び出し側が 1 つだけ持つ）
        let fixed = |_: Option<&Template>| None;
        match &self.matcher {
            Matcher::Bytes { fwd, .. } => {
                let n = fwd.needle().len();
                let mut pos = 0;
                while let Some(i) = fwd.find(&buf[pos..]) {
                    let p = pos + i;
                    if utf16 && (cs + p) % 2 == 1 {
                        pos = p + 1;
                        continue;
                    }
                    out.push((cs + p, cs + p + n, fixed(tmpl)));
                    pos = p + n.max(1);
                }
            }
            Matcher::BytesRegex(re) => match tmpl {
                Some(Template::Expand(t)) => {
                    for caps in re.captures_iter(&buf) {
                        let m = caps.get(0).unwrap();
                        let mut dst = Vec::new();
                        caps.expand(t.as_bytes(), &mut dst);
                        out.push((cs + m.start(), cs + m.end(), Some(dst)));
                    }
                }
                _ => {
                    for m in re.find_iter(&buf) {
                        out.push((cs + m.start(), cs + m.end(), fixed(tmpl)));
                    }
                }
            },
            Matcher::TextRegex(re) => {
                let text = doc.decode(&buf);
                // 位置を元のバイト位置に戻す。前の一致からの差分だけエンコードするので全体で O(n)
                let (mut dec, mut raw) = (0usize, cs);
                let mut advance = |to: usize| {
                    raw += encoded_len(doc, &text[dec..to]);
                    dec = to;
                    raw
                };
                let expand = matches!(tmpl, Some(Template::Expand(_)));
                if expand {
                    let Some(Template::Expand(t)) = tmpl else { unreachable!() };
                    for caps in re.captures_iter(&text) {
                        let m = caps.get(0).unwrap();
                        let s = advance(m.start());
                        let e = advance(m.end());
                        let mut dst = String::new();
                        caps.expand(t, &mut dst);
                        out.push((s, e, Some(doc.encode(&dst))));
                    }
                } else {
                    for m in re.find_iter(&text) {
                        let s = advance(m.start());
                        let e = advance(m.end());
                        out.push((s, e, fixed(tmpl)));
                    }
                }
            }
        }
        out
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
        let floor = doc.text_start();
        let len = doc.len() as usize;
        let from = from.max(floor);
        if from >= len {
            return SearchOutcome::NotFound;
        }
        // 近くは 1 スレッドで
        let cs = doc.line_start_at_or_before(from as u64) as usize;
        let near_end = doc.line_start_at_or_after((from + NEAR) as u64) as usize;
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
            let win_end = doc.line_start_at_or_after((pos + self.sub_chunk() * threads) as u64) as usize;
            let ranges = split(doc, pos, win_end, self.sub_chunk());
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
        let floor = doc.text_start();
        let to = to.min(doc.len() as usize);
        if to <= floor {
            return SearchOutcome::NotFound;
        }
        // `to` を含む行の終わりまでを上端にする（一致は `to` より前で始まればよい）
        let top = (doc.line_start_at_or_after(to as u64) as usize).max(to);
        let near_start = doc.line_start_at_or_before(to.saturating_sub(NEAR) as u64) as usize;
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
            let win_start = doc.line_start_at_or_before(pos.saturating_sub(self.sub_chunk() * threads) as u64) as usize;
            let ranges = split(doc, win_start, pos, self.sub_chunk());
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
        // 編集していない範囲はコピーせずに読める
        let buf = doc.read(cs as u64, ce as u64);
        let utf16 = doc.encoding().is_utf16();
        match &self.matcher {
            Matcher::Bytes { fwd, .. } => {
                let n = fwd.needle().len();
                let mut pos = lo;
                while pos < hi {
                    let p = pos + fwd.find(&buf[pos - cs..])?;
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
                let m = re.find_at(&buf, lo - cs)?;
                (cs + m.start() < hi).then(|| (cs + m.start(), cs + m.end()))
            }
            Matcher::TextRegex(re) => {
                let text = doc.decode(&buf);
                let dlo = if lo > cs { doc.decode(&buf[..lo - cs]).len() } else { 0 };
                let dhi = if hi < ce { doc.decode(&buf[..hi - cs]).len() } else { text.len() };
                let m = re.find_at(&text, floor_char(&text, dlo))?;
                (m.start() < dhi).then(|| map_back(doc, cs, &text, m.start(), m.end()))
            }
        }
    }

    /// [cs, ce) の中で、開始位置が [lo, hi) にある最後の一致
    fn chunk_last(&self, doc: &Document, cs: usize, ce: usize, lo: usize, hi: usize) -> Option<(usize, usize)> {
        let buf = doc.read(cs as u64, ce as u64);
        let utf16 = doc.encoding().is_utf16();
        match &self.matcher {
            Matcher::Bytes { rev, .. } => {
                let n = rev.needle().len();
                let mut end = (hi + n - 1).min(ce);
                while end > lo {
                    let p = lo + rev.rfind(&buf[lo - cs..end - cs])?;
                    if !utf16 || p.is_multiple_of(2) {
                        return Some((p, p + n));
                    }
                    end = p + n - 1;
                }
                None
            }
            Matcher::BytesRegex(re) => {
                let hay = &buf[..];
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
                let text = doc.decode(&buf);
                let dlo = if lo > cs { doc.decode(&buf[..lo - cs]).len() } else { 0 };
                let dhi = if hi < ce { doc.decode(&buf[..hi - cs]).len() } else { text.len() };
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

/// 置換後の文字列
enum Template {
    /// すべての一致で同じバイト列
    Fixed(Vec<u8>),
    /// 正規表現のキャプチャを展開する
    Expand(String),
}

/// デコードした文字列 `s` を元の文字コードに戻したときのバイト数。
/// 位置を元のバイト位置に戻すために、一致のたびに呼ぶので、エンコードし直さずに数えられる場合は数える
/// （ASCII だけなら文字列の長さ、Shift_JIS なら 1 文字 1〜2 バイト、UTF-16 なら 2 バイト単位）
fn encoded_len(doc: &Document, s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    let enc = doc.encoding().encoding;
    if enc == encoding_rs::SHIFT_JIS && !s.contains('\u{FFFD}') {
        // ASCII と半角カナは 1 バイト、それ以外（全角・記号）は 2 バイト
        return s.chars().map(|c| if (c as u32) < 0x80 || ('\u{FF61}'..='\u{FF9F}').contains(&c) { 1 } else { 2 }).sum();
    }
    if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE {
        return s.encode_utf16().count() * 2;
    }
    doc.encode(s).len()
}

/// デコード後の位置を、元ファイルのバイト位置に戻す（再エンコードした長さで求める）
fn map_back(doc: &Document, cs: usize, text: &str, s: usize, e: usize) -> (usize, usize) {
    let start = cs + encoded_len(doc, &text[..s]);
    (start, start + encoded_len(doc, &text[s..e]))
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
pub(crate) fn split(doc: &Document, a: usize, b: usize, size: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = a;
    while s < b {
        let e = (doc.line_start_at_or_after((s + size) as u64) as usize).min(b).max(s + 1);
        out.push((s, e));
        s = e;
    }
    out
}

pub(crate) fn run_parallel<T, F>(ranges: &[(usize, usize)], f: F) -> Vec<Option<T>>
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

pub(crate) fn threads() -> usize {
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
        while let SearchOutcome::Found(s, e) = q.find(d, pos, dir, &ctl) {
            out.push((s, e));
            pos = match dir {
                Direction::Forward => e.max(s + 1),
                Direction::Backward => s,
            };
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
    fn find_all_and_replacements() {
        let text: String = (0..30000).map(|i| format!("id={i} name=user{} ok\n", i % 7)).collect();
        let d = doc(text.clone().into_bytes());
        // 文字列
        let q = Query::new(&d, "user3", CS).unwrap();
        let r = q.find_all(&d, Some("U3"), usize::MAX, &SearchControl::default());
        let want: Vec<_> = naive(&text, "user3").into_iter().map(|i| (i as u64, i as u64 + 5)).collect();
        assert_eq!(r.ranges, want);
        assert_eq!(r.replacements, [b"U3".to_vec()]);
        // 正規表現のキャプチャ
        let q = Query::new(&d, r"id=(\d+) name=(\w+)", RE).unwrap();
        let r = q.find_all(&d, Some("$2:$1"), usize::MAX, &SearchControl::default());
        assert_eq!(r.ranges.len(), 30000);
        assert_eq!(r.replacements[12], b"user5:12");
        // 上限で打ち切り
        let r = q.find_all(&d, None, 100, &SearchControl::default());
        assert!(r.truncated && r.ranges.len() == 100);
        // 1 件ずつの置換
        let (s, e) = (r.ranges[3].0, r.ranges[3].1);
        assert_eq!(q.replacement_at(&d, s, e, "[$1]").unwrap(), b"[3]");
        assert!(q.replacement_at(&d, s + 1, e, "x").is_none());
    }

    #[test]
    fn encoded_len_matches_real_encoding() {
        // Shift_JIS：ASCII・半角カナ・全角・記号・機種依存文字
        let sj = "abc ｱｲｳ 日本語のテキスト、①㈱ <>\\ ~ ｶﾀｶﾅ\r\n";
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(sj);
        let d = doc(bytes.into_owned());
        for text in [sj, "abc", "", "全角だけ", "ｱ", "a日b"] {
            assert_eq!(encoded_len(&d, text), d.encode(text).len(), "{text:?}");
        }
        // UTF-16
        let mut u16b = vec![0xFF, 0xFE];
        u16b.extend("あいう\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        let d = doc(u16b);
        for text in ["あ", "a日b🙂", ""] {
            assert_eq!(encoded_len(&d, text), d.encode(text).len(), "{text:?}");
        }
        // EUC-JP は従来どおり（エンコードして数える）
        let (bytes, _, _) = encoding_rs::EUC_JP.encode("日本語 abc ｱｲ\n");
        let d = doc(bytes.into_owned());
        assert_eq!(encoded_len(&d, "日本語 ｱ"), d.encode("日本語 ｱ").len());
    }

    #[test]
    fn find_all_within_a_range() {
        let text = "foo 1\nfoo 2\nfoo 3\nfoo 4\nfoo 5\n";
        let d = doc(text.as_bytes().to_vec());
        let q = Query::new(&d, "foo", CS).unwrap();
        // 2 行目の頭から 4 行目の頭まで（= 2・3 行目）
        let r = q.find_all_in(&d, Some("BAR"), usize::MAX, &SearchControl::default(), Some((6, 18)));
        assert_eq!(r.ranges, vec![(6, 9), (12, 15)]);
        // 範囲の途中で切れる一致は含めない
        let r = q.find_all_in(&d, None, usize::MAX, &SearchControl::default(), Some((7, 14)));
        assert!(r.ranges.is_empty(), "{:?}", r.ranges);
        // 範囲が全体なら find_all と同じ
        let all = q.find_all(&d, None, usize::MAX, &SearchControl::default());
        let same = q.find_all_in(&d, None, usize::MAX, &SearchControl::default(), Some((0, d.len())));
        assert_eq!(all.ranges, same.ranges);
        // 正規表現（文字コードを戻す経路）でも位置が正しい
        let (sj, _, _) = encoding_rs::SHIFT_JIS.encode("あ foo\nい foo\nう foo\n");
        let d = doc(sj.into_owned());
        let q = Query::new(&d, "f(o+)", RE).unwrap();
        let (s, e) = (d.line_range(1).unwrap().0, d.line_range(2).unwrap().0);
        let r = q.find_all_in(&d, Some("[$1]"), usize::MAX, &SearchControl::default(), Some((s, e)));
        assert_eq!(r.ranges.len(), 1);
        assert_eq!(r.replacements[0], b"[oo]");
        assert!(r.ranges[0].0 >= s && r.ranges[0].1 <= e);
    }

    #[test]
    fn find_all_sjis_regex_positions() {
        let text = "名前=太郎 年齢=20\r\n名前=花子 年齢=31\r\n".repeat(200);
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
        let bytes = bytes.into_owned();
        let d = doc(bytes.clone());
        let q = Query::new(&d, "名前=(\\S+) 年齢=(\\d+)", RE).unwrap();
        let r = q.find_all(&d, Some("$2歳の$1"), usize::MAX, &SearchControl::default());
        assert_eq!(r.ranges.len(), 400);
        let (s, e) = r.ranges[1];
        assert_eq!(&bytes[s as usize..e as usize], &*encoding_rs::SHIFT_JIS.encode("名前=花子 年齢=31").0);
        assert_eq!(r.replacements[1], &*encoding_rs::SHIFT_JIS.encode("31歳の花子").0);
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
