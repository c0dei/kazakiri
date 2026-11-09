//! 行インデックス。
//!
//! 数GBのファイルで全行の開始位置を持つと、それだけで数百MBになる。
//! そこでファイルを固定サイズのチャンクに分けて複数スレッドで並列に走査し、
//! `CHECKPOINT_EVERY` 行ごとの開始位置だけを記録する（疎なインデックス）。
//! 目的の行へは、直前のチェックポイントから memchr で数百行ぶん進むだけで届く。
//!
//! 走査はバックグラウンドで行い、先頭から連続して終わったチャンクの分だけ
//! すぐに参照できる。そのため、ファイルを開いた直後から表示を始められる。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;

use crate::source::Source;

/// 何行ごとに開始位置を記録するか
pub const CHECKPOINT_EVERY: u64 = 256;
/// 既定のチャンクサイズ（偶数であること。UTF-16 の改行単位を割らないため）
pub const DEFAULT_CHUNK: u64 = 8 << 20;

/// 改行の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    /// LF または CRLF（表示時に末尾の CR を落とす）
    Lf,
    /// CR のみ（古い Mac 形式）
    Cr,
    /// UTF-16LE の LF（0A 00）
    Utf16Le,
    /// UTF-16BE の LF（00 0A）
    Utf16Be,
}

impl Newline {
    fn needle(self) -> u8 {
        match self {
            Newline::Cr => b'\r',
            _ => b'\n',
        }
    }

    /// `p` の位置に needle が見つかったとき、それが本当に改行なら次の行の開始位置を返す
    #[inline]
    fn accept(self, data: &[u8], p: usize) -> Option<usize> {
        match self {
            Newline::Lf | Newline::Cr => Some(p + 1),
            Newline::Utf16Le => {
                (p.is_multiple_of(2) && data.get(p + 1) == Some(&0)).then_some(p + 2)
            }
            Newline::Utf16Be => (p % 2 == 1 && data[p - 1] == 0).then_some(p + 1),
        }
    }
}

/// `from` 以降で最初の改行の直後（=次の行の開始位置）を返す
#[inline]
pub fn next_line_start(data: &[u8], from: usize, kind: Newline) -> Option<usize> {
    let needle = kind.needle();
    let mut pos = from;
    while pos < data.len() {
        let rel = memchr::memchr(needle, &data[pos..])?;
        let p = pos + rel;
        if let Some(next) = kind.accept(data, p) {
            return Some(next);
        }
        pos = p + 1;
    }
    None
}

/// `pos` 以前（`pos` を含む）で最も後ろにある行の開始位置。`floor` より前は見ない
pub fn line_start_at_or_before(data: &[u8], pos: usize, floor: usize, kind: Newline) -> usize {
    let needle = kind.needle();
    let mut end = pos.min(data.len());
    while end > floor {
        let Some(q) = memchr::memrchr(needle, &data[floor..end]).map(|r| floor + r) else { break };
        if let Some(next) = kind.accept(data, q) {
            if next <= pos {
                return next;
            }
        }
        end = q;
    }
    floor
}

/// `pos` 以降（`pos` を含む）で最初の行の開始位置。なければ末尾
pub fn line_start_at_or_after(data: &[u8], pos: usize, floor: usize, kind: Newline) -> usize {
    if pos <= floor {
        return floor;
    }
    next_line_start(data, pos - 1, kind).unwrap_or(data.len()).max(pos.min(data.len()))
}

/// 1 チャンクの走査結果
#[derive(Debug, Default, Clone)]
struct ChunkInfo {
    start: u64,
    end: u64,
    newlines: u64,
    /// checkpoints[k] = このチャンク内 k*CHECKPOINT_EVERY 番目の改行の直後の位置
    checkpoints: Vec<u64>,
}

#[derive(Debug, Clone)]
struct ReadyChunk {
    /// このチャンクより前にある改行の総数
    newlines_before: u64,
    info: ChunkInfo,
}

#[derive(Debug, Default)]
struct State {
    chunks: Vec<ReadyChunk>,
    newlines_ready: u64,
    bytes_ready: u64,
    done: bool,
}

struct Shared {
    source: Arc<Source>,
    kind: Newline,
    text_start: u64,
    state: RwLock<State>,
    cancel: AtomicBool,
    /// 完了順に関係なく走査を終えたバイト数（進捗表示用）
    scanned: AtomicU64,
}

pub struct LineIndex {
    shared: Arc<Shared>,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub scanned_bytes: u64,
    pub ready_bytes: u64,
    pub total_bytes: u64,
    pub done: bool,
}

impl LineIndex {
    /// バックグラウンドで索引作成を始める
    pub fn build(source: Arc<Source>, kind: Newline, text_start: u64) -> LineIndex {
        Self::build_with(source, kind, text_start, DEFAULT_CHUNK, default_threads())
    }

    pub fn build_with(
        source: Arc<Source>,
        kind: Newline,
        text_start: u64,
        chunk_size: u64,
        threads: usize,
    ) -> LineIndex {
        assert!(chunk_size >= 2 && chunk_size.is_multiple_of(2));
        let shared = Arc::new(Shared {
            source,
            kind,
            text_start,
            state: RwLock::new(State::default()),
            cancel: AtomicBool::new(false),
            scanned: AtomicU64::new(0),
        });
        let s = shared.clone();
        let start = text_start;
        thread::Builder::new()
            .name("line-index".into())
            .spawn(move || run(s, chunk_size, threads.max(1), start))
            .expect("spawn line-index thread");
        LineIndex { shared }
    }

    /// 索引を作らない（GREP のように行番号を自分で数える用途）。行の位置は分からない
    pub fn none(source: Arc<Source>, kind: Newline, text_start: u64) -> LineIndex {
        let shared = Arc::new(Shared {
            source,
            kind,
            text_start,
            state: RwLock::new(State::default()),
            cancel: AtomicBool::new(true),
            scanned: AtomicU64::new(0),
        });
        LineIndex { shared }
    }

    /// 末尾に追記されたファイル用。`prev` の索引を引き継ぎ、増えた部分だけを走査する。
    /// `prev` の索引作成が終わっていないとき、または小さくなったときは `None`
    pub fn extend(prev: &LineIndex, source: Arc<Source>) -> Option<LineIndex> {
        let old = prev.shared.state.read().unwrap();
        let old_len = prev.shared.source.len();
        if !old.done || source.len() < old_len {
            return None;
        }
        let state = State {
            chunks: old.chunks.clone(),
            newlines_ready: old.newlines_ready,
            bytes_ready: old_len,
            done: false,
        };
        drop(old);
        let shared = Arc::new(Shared {
            source,
            kind: prev.shared.kind,
            text_start: prev.shared.text_start,
            state: RwLock::new(state),
            cancel: AtomicBool::new(false),
            scanned: AtomicU64::new(old_len),
        });
        let s = shared.clone();
        thread::Builder::new()
            .name("line-index".into())
            .spawn(move || run(s, DEFAULT_CHUNK, default_threads(), old_len))
            .expect("spawn line-index thread");
        Some(LineIndex { shared })
    }

    pub fn kind(&self) -> Newline {
        self.shared.kind
    }

    pub fn progress(&self) -> Progress {
        let st = self.shared.state.read().unwrap();
        Progress {
            scanned_bytes: self.shared.scanned.load(Ordering::Relaxed),
            ready_bytes: st.bytes_ready,
            total_bytes: self.shared.source.len(),
            done: st.done,
        }
    }

    pub fn is_done(&self) -> bool {
        self.shared.state.read().unwrap().done
    }

    /// 開始位置が確定している行数
    pub fn known_lines(&self) -> u64 {
        self.shared.state.read().unwrap().newlines_ready + 1
    }

    /// 総行数。索引作成中は、ここまでの平均行長からの推定値
    pub fn total_lines(&self) -> (u64, bool) {
        let st = self.shared.state.read().unwrap();
        let known = st.newlines_ready + 1;
        if st.done {
            return (known, true);
        }
        let total = self.shared.source.len().saturating_sub(self.shared.text_start);
        let ready = st.bytes_ready.saturating_sub(self.shared.text_start);
        if ready == 0 || st.newlines_ready == 0 {
            return (known.max(1), false);
        }
        let est = (st.newlines_ready as f64 * total as f64 / ready as f64) as u64 + 1;
        (est.max(known), false)
    }

    /// 行 `line`（0 始まり）の開始バイト位置
    pub fn line_start(&self, line: u64) -> Option<u64> {
        if line == 0 {
            return Some(self.shared.text_start);
        }
        let g = line - 1; // この行の直前にある改行の通し番号
        let st = self.shared.state.read().unwrap();
        if g >= st.newlines_ready {
            return None;
        }
        let ci = st.chunks.partition_point(|c| c.newlines_before <= g) - 1;
        let c = &st.chunks[ci];
        let local = g - c.newlines_before;
        let k = (local / CHECKPOINT_EVERY) as usize;
        let mut pos = c.info.checkpoints[k] as usize;
        let data = self.shared.source.bytes();
        for _ in 0..(local % CHECKPOINT_EVERY) {
            pos = next_line_start(data, pos, self.shared.kind)?;
        }
        Some(pos as u64)
    }

    /// バイト位置 `offset` を含む行の番号
    pub fn line_of_offset(&self, offset: u64) -> Option<u64> {
        let text_start = self.shared.text_start;
        if offset <= text_start {
            return Some(0);
        }
        let st = self.shared.state.read().unwrap();
        if offset >= st.bytes_ready && !(st.done && offset <= self.shared.source.len()) {
            return None;
        }
        let ci = st.chunks.partition_point(|c| c.info.start <= offset).saturating_sub(1);
        let c = st.chunks.get(ci)?;
        let cps = &c.info.checkpoints;
        let k = cps.partition_point(|&p| p <= offset);
        let (mut count, from) = if k == 0 {
            (c.newlines_before, c.info.start.max(text_start))
        } else {
            let k = k - 1;
            (c.newlines_before + k as u64 * CHECKPOINT_EVERY + 1, cps[k])
        };
        let data = self.shared.source.bytes();
        let mut pos = from as usize;
        while let Some(next) = next_line_start(data, pos, self.shared.kind) {
            if next as u64 > offset {
                break;
            }
            count += 1;
            pos = next;
        }
        Some(count)
    }
}

impl Drop for LineIndex {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

fn default_threads() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8)
}

/// `scan_from` から末尾までを走査し、既存の状態の後ろに追加していく
fn run(shared: Arc<Shared>, chunk_size: u64, threads: usize, scan_from: u64) {
    let len = shared.source.len();
    let start = scan_from.min(len);
    let n_chunks = (len - start).div_ceil(chunk_size) as usize;
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<(usize, ChunkInfo)>();

    let workers: Vec<_> = (0..threads.min(n_chunks.max(1)))
        .map(|_| {
            let shared = shared.clone();
            let next = next.clone();
            let tx = tx.clone();
            thread::spawn(move || loop {
                if shared.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n_chunks {
                    return;
                }
                let cs = start + i as u64 * chunk_size;
                let ce = (cs + chunk_size).min(len);
                let info = scan_chunk(shared.source.bytes(), cs, ce, shared.kind);
                shared.scanned.fetch_add(ce - cs, Ordering::Relaxed);
                if tx.send((i, info)).is_err() {
                    return;
                }
            })
        })
        .collect();
    drop(tx);

    // 届いた順はばらばらなので、先頭から連続した分だけ公開する
    let mut pending = BTreeMap::new();
    let mut expect = 0usize;
    for (i, info) in rx {
        pending.insert(i, info);
        let mut st = shared.state.write().unwrap();
        while let Some(info) = pending.remove(&expect) {
            let before = st.newlines_ready;
            st.newlines_ready += info.newlines;
            st.bytes_ready = info.end;
            st.chunks.push(ReadyChunk { newlines_before: before, info });
            expect += 1;
        }
    }
    for w in workers {
        let _ = w.join();
    }
    if !shared.cancel.load(Ordering::Relaxed) {
        let mut st = shared.state.write().unwrap();
        st.bytes_ready = len;
        st.done = true;
    }
}

fn scan_chunk(data: &[u8], cs: u64, ce: u64, kind: Newline) -> ChunkInfo {
    let (cs_u, ce_u) = (cs as usize, ce as usize);
    let mut checkpoints = Vec::new();
    let mut n = 0u64;
    for rel in memchr::memchr_iter(kind.needle(), &data[cs_u..ce_u]) {
        if let Some(next) = kind.accept(data, cs_u + rel) {
            if n.is_multiple_of(CHECKPOINT_EVERY) {
                checkpoints.push(next as u64);
            }
            n += 1;
        }
    }
    ChunkInfo { start: cs, end: ce, newlines: n, checkpoints }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(ix: &LineIndex) {
        while !ix.is_done() {
            thread::yield_now();
        }
    }

    fn naive_starts(data: &[u8], kind: Newline, text_start: usize) -> Vec<usize> {
        let mut v = vec![text_start];
        let mut pos = text_start;
        while let Some(n) = next_line_start(data, pos, kind) {
            v.push(n);
            pos = n;
        }
        v
    }

    fn check(data: Vec<u8>, kind: Newline, text_start: u64) {
        let expected = naive_starts(&data, kind, text_start as usize);
        for chunk in [2u64, 6, 64, 1 << 20] {
            for threads in [1, 3] {
                let src = Arc::new(Source::from_vec(data.clone()));
                let ix = LineIndex::build_with(src, kind, text_start, chunk, threads);
                wait(&ix);
                assert_eq!(ix.total_lines(), (expected.len() as u64, true));
                for (i, &s) in expected.iter().enumerate() {
                    assert_eq!(ix.line_start(i as u64), Some(s as u64), "line {i} chunk {chunk}");
                }
                assert_eq!(ix.line_start(expected.len() as u64), None);
                // 各バイト位置 → 行番号
                for off in text_start as usize..=data.len() {
                    let want = expected.partition_point(|&s| s <= off) as u64 - 1;
                    assert_eq!(ix.line_of_offset(off as u64), Some(want), "offset {off}");
                }
            }
        }
    }

    fn lines_text(n: usize, sep: &str) -> String {
        (0..n).map(|i| format!("line {i} {}", "x".repeat(i % 7))).collect::<Vec<_>>().join(sep)
    }

    #[test]
    fn lf_crlf_cr() {
        check(lines_text(1000, "\n").into_bytes(), Newline::Lf, 0);
        check((lines_text(700, "\r\n") + "\r\n").into_bytes(), Newline::Lf, 0);
        check(lines_text(600, "\r").into_bytes(), Newline::Cr, 0);
        check(b"\n\n\n".to_vec(), Newline::Lf, 0);
        check(b"no newline".to_vec(), Newline::Lf, 0);
        check(Vec::new(), Newline::Lf, 0);
    }

    #[test]
    fn extend_appended() {
        for kind_text in [("\n", Newline::Lf), ("\r\n", Newline::Lf), ("\r", Newline::Cr)] {
            let (sep, kind) = kind_text;
            let first = format!("{}{sep}half", lines_text(900, sep));
            let src = Arc::new(Source::from_vec(first.clone().into_bytes()));
            let ix = LineIndex::build_with(src, kind, 0, 64, 3);
            wait(&ix);
            // 追記（途中の行の続きから）
            let full = format!("{first} line continues{sep}{}", lines_text(700, sep));
            let src2 = Arc::new(Source::from_vec(full.clone().into_bytes()));
            let ix2 = LineIndex::extend(&ix, src2).unwrap();
            wait(&ix2);
            let expected = naive_starts(full.as_bytes(), kind, 0);
            assert_eq!(ix2.total_lines(), (expected.len() as u64, true));
            for (i, &st) in expected.iter().enumerate() {
                assert_eq!(ix2.line_start(i as u64), Some(st as u64), "{sep:?} line {i}");
            }
            for off in (0..=full.len()).step_by(7) {
                let want = expected.partition_point(|&s| s <= off) as u64 - 1;
                assert_eq!(ix2.line_of_offset(off as u64), Some(want));
            }
            // 小さくなったら引き継げない
            let short = Arc::new(Source::from_vec(b"x".to_vec()));
            assert!(LineIndex::extend(&ix2, short).is_none());
        }
    }

    #[test]
    fn with_bom_offset() {
        let mut v = b"\xEF\xBB\xBF".to_vec();
        v.extend(lines_text(500, "\n").into_bytes());
        check(v, Newline::Lf, 3);
    }

    #[test]
    fn utf16() {
        // U+0A0A（ਊ に近いグルムキー文字）や U+000A を含む上位バイトで誤検出しないか
        let s = format!("{}\u{0A0A}\n\u{0A00}x\n", lines_text(400, "\r\n"));
        let mut le = vec![0xFF, 0xFE];
        le.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let mut be = vec![0xFE, 0xFF];
        be.extend(s.encode_utf16().flat_map(|u| u.to_be_bytes()));
        let n_lines = s.matches('\n').count() + 1;
        assert_eq!(naive_starts(&le, Newline::Utf16Le, 2).len(), n_lines);
        assert_eq!(naive_starts(&be, Newline::Utf16Be, 2).len(), n_lines);
        check(le, Newline::Utf16Le, 2);
        check(be, Newline::Utf16Be, 2);
    }
}
