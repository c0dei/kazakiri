//! ピーステーブル（編集用のテキスト表現）。
//!
//! 元ファイル（メモリマップ）は読み取り専用のまま置いておき、
//! 「元ファイルのこの範囲」「追加した文字列のこの範囲」という断片（ピース）の並びで
//! 編集後のテキストを表す。数GBのファイルでも、編集はピースの並びを変えるだけで済む。
//!
//! 各ピースは中に含む改行の数を持つので、行番号 → 位置の変換は
//! 「ピースを二分探索 → 元ファイルの行インデックス（または追加文字列の走査）」で求まる。
//!
//! 追加した文字列は `Arc<[u8]>` で持ち、書き換えない。そのため編集で取り除いたピースを
//! 取っておけば、それを差し戻すだけで元に戻せる（Undo の記録は編集の大きさに比例するだけ）。

use std::borrow::Cow;
use std::sync::Arc;

use crate::line_index::{next_line_start, LineIndex, Newline};

/// 続けて入力した文字をひとつの追加文字列にまとめる上限
const MAX_GROW: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) enum Src {
    /// 元ファイル
    Orig,
    /// 追加した文字列
    Add(Arc<[u8]>),
}

#[derive(Clone, Debug)]
pub(crate) struct Piece {
    src: Src,
    /// 元ファイル、または追加文字列の中での開始位置
    start: u64,
    len: u64,
    /// このピースに含まれる改行の数
    nl: u64,
}

/// 元ファイルとその行インデックス
pub(crate) struct Ctx<'a> {
    pub data: &'a [u8],
    pub index: &'a LineIndex,
    pub kind: Newline,
}

impl Ctx<'_> {
    /// 元ファイル [a, b) にある改行の数（行インデックスから求める）
    fn orig_nl(&self, a: u64, b: u64) -> u64 {
        let lof = |x: u64| self.index.line_of_offset(x).expect("index must be complete");
        lof(b) - lof(a)
    }
}

fn count_nl(bytes: &[u8], kind: Newline) -> u64 {
    let mut n = 0;
    let mut pos = 0;
    while let Some(next) = next_line_start(bytes, pos, kind) {
        n += 1;
        pos = next;
    }
    n
}

/// すべて置換の置換後のバイト列
#[derive(Clone, Copy)]
pub(crate) enum Repls<'a> {
    /// 全件同じ
    One(&'a [u8]),
    /// 一致ごと
    Each(&'a [Vec<u8>]),
}

/// ピース列の中の読み出し位置
struct Cursor {
    i: usize,
    off: u64,
}

impl Cursor {
    fn skip(&mut self, list: &[Piece], mut n: u64) {
        while n > 0 && self.i < list.len() {
            let rest = list[self.i].len - self.off;
            if n < rest {
                self.off += n;
                return;
            }
            n -= rest;
            self.i += 1;
            self.off = 0;
        }
    }
}

/// ピースの一部 [start, start + len) の改行数
fn sub_nl(ctx: &Ctx, p: &Piece, start: u64, len: u64) -> u64 {
    if start == p.start && len == p.len {
        return p.nl;
    }
    match &p.src {
        // 短ければ直接数え、長ければ行インデックスを使う
        Src::Orig if len > 64 * 1024 => ctx.orig_nl(start, start + len),
        Src::Orig => count_nl(&ctx.data[start as usize..(start + len) as usize], ctx.kind),
        Src::Add(chunk) => count_nl(&chunk[start as usize..(start + len) as usize], ctx.kind),
    }
}

/// `list` の `cur` から `n` バイトぶんを切り出して `out` に足す
fn cut(ctx: &Ctx, list: &[Piece], cur: &mut Cursor, mut n: u64, out: &mut Vec<Piece>) {
    while n > 0 && cur.i < list.len() {
        let p = &list[cur.i];
        let take = n.min(p.len - cur.off);
        let start = p.start + cur.off;
        out.push(Piece { src: p.src.clone(), start, len: take, nl: sub_nl(ctx, p, start, take) });
        n -= take;
        cur.skip(list, take);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Pieces {
    pieces: Vec<Piece>,
    /// 各ピースの開始位置（編集後のテキストでの位置）
    starts: Vec<u64>,
    /// 各ピースより前にある改行の数
    nls: Vec<u64>,
    len: u64,
    total_nl: u64,
    /// 追加した文字列の中の行頭を、前に調べた場所から続けて探すための覚え書き
    scan: ScanCache,
}

/// （ピースの番号, ピースの中の何番目の改行か, その次の行頭の位置）の覚え書き。
/// 順に行を読んでいくとき、毎回ピースの先頭から数え直す（行数の 2 乗の時間）のを避ける。
/// 同時に何本かのスレッドが違う場所を読んでもよいよう、いくつか覚える
#[derive(Default)]
struct ScanCache(std::sync::Mutex<Vec<(usize, u64, usize)>>);

impl Clone for ScanCache {
    fn clone(&self) -> Self {
        ScanCache::default()
    }
}

impl std::fmt::Debug for ScanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScanCache")
    }
}

const SCAN_SLOTS: usize = 16;

impl Piece {
    fn bytes<'a>(&'a self, ctx: &Ctx<'a>) -> &'a [u8] {
        let (s, e) = (self.start as usize, (self.start + self.len) as usize);
        match &self.src {
            Src::Orig => &ctx.data[s..e],
            Src::Add(chunk) => &chunk[s..e],
        }
    }
}

impl Pieces {
    /// 元ファイル全体をひとつのピースにする（行インデックスが完成している必要がある）
    pub fn new(ctx: &Ctx) -> Pieces {
        let len = ctx.data.len() as u64;
        let mut p = Pieces { pieces: Vec::new(), starts: Vec::new(), nls: Vec::new(), len: 0, total_nl: 0, scan: ScanCache::default() };
        if len > 0 {
            p.pieces.push(Piece { src: Src::Orig, start: 0, len, nl: ctx.orig_nl(0, len) });
        }
        p.rebuild();
        p
    }

    /// 行頭の覚え書きを捨てる（ピースを書き換えるとき）
    fn forget(&mut self) {
        if let Ok(c) = self.scan.0.get_mut() {
            c.clear();
        }
    }

    fn rebuild(&mut self) {
        self.forget();
        // 大きく縮んだら（すべて置換の取り消しなど）確保していた領域を返す
        if self.pieces.capacity() > 1024 && self.pieces.capacity() > self.pieces.len() * 4 {
            self.pieces.shrink_to_fit();
            self.starts = Vec::new();
            self.nls = Vec::new();
        }
        self.starts.clear();
        self.nls.clear();
        // 倍々に伸ばすと一時的に大きくなるので、必要な分だけ確保する
        self.starts.reserve_exact(self.pieces.len());
        self.nls.reserve_exact(self.pieces.len());
        let (mut pos, mut nl) = (0, 0);
        for p in &self.pieces {
            self.starts.push(pos);
            self.nls.push(nl);
            pos += p.len;
            nl += p.nl;
        }
        self.len = pos;
        self.total_nl = nl;
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn total_lines(&self) -> u64 {
        self.total_nl + 1
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// `pos` を含むピースの番号（`pos == len` なら最後のピース）
    fn piece_at(&self, pos: u64) -> usize {
        self.starts.partition_point(|&s| s <= pos).saturating_sub(1)
    }

    pub fn read<'a>(&'a self, ctx: &Ctx<'a>, a: u64, b: u64) -> Cow<'a, [u8]> {
        let b = b.min(self.len);
        if a >= b {
            return Cow::Borrowed(&[]);
        }
        let i = self.piece_at(a);
        let first = &self.pieces[i];
        let off = a - self.starts[i];
        if b <= self.starts[i] + first.len {
            return Cow::Borrowed(&first.bytes(ctx)[off as usize..(off + b - a) as usize]);
        }
        let mut out = Vec::with_capacity((b - a) as usize);
        let mut j = i;
        while j < self.pieces.len() && self.starts[j] < b {
            let p = &self.pieces[j];
            let s = a.max(self.starts[j]) - self.starts[j];
            let e = b.min(self.starts[j] + p.len) - self.starts[j];
            out.extend_from_slice(&p.bytes(ctx)[s as usize..e as usize]);
            j += 1;
        }
        Cow::Owned(out)
    }

    /// 全体を順に取り出す（保存用）。各要素は元ファイルか追加文字列の一部
    pub fn segments<'a>(&'a self, ctx: &'a Ctx<'a>) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.pieces.iter().map(move |p| p.bytes(ctx))
    }

    /// `pos` にピースの境目を作り、`pos` から始まるピースの番号を返す
    fn split(&mut self, ctx: &Ctx, pos: u64) -> usize {
        self.forget();
        if pos >= self.len {
            return self.pieces.len();
        }
        let i = self.piece_at(pos);
        let k = pos - self.starts[i];
        if k == 0 {
            return i;
        }
        let p = self.pieces[i].clone();
        let left_nl = match &p.src {
            Src::Orig => ctx.orig_nl(p.start, p.start + k),
            Src::Add(chunk) => count_nl(&chunk[p.start as usize..(p.start + k) as usize], ctx.kind),
        };
        self.pieces[i] = Piece { src: p.src.clone(), start: p.start, len: k, nl: left_nl };
        self.pieces.insert(i + 1, Piece { src: p.src, start: p.start + k, len: p.len - k, nl: p.nl - left_nl });
        self.rebuild();
        i + 1
    }

    /// [a, b) を `new` のピース列で置き換え、取り除いたピース列を返す
    pub fn splice(&mut self, ctx: &Ctx, a: u64, b: u64, new: Vec<Piece>) -> Vec<Piece> {
        self.forget();
        let b = b.min(self.len);
        let ja = self.split(ctx, a);
        let jb = if b > a { self.split(ctx, b) } else { ja };
        let removed: Vec<Piece> = self.pieces.splice(ja..jb, new).collect();
        self.rebuild();
        removed
    }

    /// 複数の範囲をまとめて置き換える（すべて置換用）。
    /// `ranges` は重ならず位置の順。置換後のバイト列はひとつの追加文字列にまとめる。
    /// 取り除いたピース列（`ranges` の最初の開始〜最後の終了の範囲ぶん）を返す
    pub fn replace_many(&mut self, ctx: &Ctx, ranges: &[(u64, u64)], repls: Repls) -> Vec<Piece> {
        self.forget();
        let Some(&(first, _)) = ranges.first() else { return Vec::new() };
        let last = ranges.last().unwrap().1.min(self.len);
        let removed = self.splice(ctx, first, last, Vec::new());

        // 全件共通なら 1 つの追加文字列をすべてのピースで共有する
        let (chunk, one_nl): (Arc<[u8]>, u64) = match repls {
            Repls::One(b) => (b.into(), count_nl(b, ctx.kind)),
            Repls::Each(list) => {
                let total: usize = list.iter().map(|r| r.len()).sum();
                let mut all = Vec::with_capacity(total);
                for r in list {
                    all.extend_from_slice(r);
                }
                (all.into(), 0)
            }
        };

        // 新しいピースは本体の配列に直接積む（大きな一時配列を作らない）
        let at = self.split(ctx, first);
        let tail = self.pieces.split_off(at);
        self.pieces.reserve(ranges.len() * 2 + 1 + tail.len());
        let new = &mut self.pieces;

        // 取り除いたピース列から、一致と一致のあいだを切り出す（前から順に進むだけ）
        let mut cur = Cursor { i: 0, off: 0 };
        let mut pos = first; // いまの位置（取り除く前の座標）
        let mut roff = 0u64; // 追加文字列の中の位置
        for (k, &(a, b)) in ranges.iter().enumerate() {
            if a > pos {
                cut(ctx, &removed, &mut cur, a - pos, new);
            }
            cur.skip(&removed, b - a.max(pos));
            pos = b;
            match repls {
                Repls::One(b) if !b.is_empty() => {
                    new.push(Piece { src: Src::Add(chunk.clone()), start: 0, len: b.len() as u64, nl: one_nl });
                }
                Repls::Each(list) => {
                    let n = list.get(k).map_or(0, |r| r.len() as u64);
                    if n > 0 {
                        let nl = count_nl(&chunk[roff as usize..(roff + n) as usize], ctx.kind);
                        new.push(Piece { src: Src::Add(chunk.clone()), start: roff, len: n, nl });
                        roff += n;
                    }
                }
                _ => {}
            }
        }
        self.pieces.extend(tail);
        self.rebuild();
        removed
    }

    /// [a, b) を `bytes` で置き換え、取り除いたピース列を返す
    pub fn replace(&mut self, ctx: &Ctx, a: u64, b: u64, bytes: &[u8]) -> Vec<Piece> {
        self.forget();
        let removed = if b > a { self.splice(ctx, a, b, Vec::new()) } else { Vec::new() };
        self.insert(ctx, a, bytes);
        removed
    }

    pub fn insert(&mut self, ctx: &Ctx, pos: u64, bytes: &[u8]) {
        self.forget();
        if bytes.is_empty() {
            return;
        }
        let pos = pos.min(self.len);
        let j = self.split(ctx, pos);
        let nl = count_nl(bytes, ctx.kind);
        // 直前のピースが「さっき入力した文字列」の末尾なら、つなげてひとつにする
        if j > 0 {
            let prev = &self.pieces[j - 1];
            if let Src::Add(chunk) = &prev.src {
                if prev.start + prev.len == chunk.len() as u64 && chunk.len() + bytes.len() <= MAX_GROW {
                    let mut grown = Vec::with_capacity(chunk.len() + bytes.len());
                    grown.extend_from_slice(chunk);
                    grown.extend_from_slice(bytes);
                    let (start, len, pnl) = (prev.start, prev.len, prev.nl);
                    self.pieces[j - 1] = Piece { src: Src::Add(grown.into()), start, len: len + bytes.len() as u64, nl: pnl + nl };
                    self.rebuild();
                    return;
                }
            }
        }
        let chunk: Arc<[u8]> = bytes.into();
        self.pieces.insert(j, Piece { src: Src::Add(chunk), start: 0, len: bytes.len() as u64, nl });
        self.rebuild();
    }

    /// ピース列の合計の長さ
    pub fn total_len(pieces: &[Piece]) -> u64 {
        pieces.iter().map(|p| p.len).sum()
    }

    /// 行 `n` の開始位置（行 0 は `text_start`）
    pub fn line_start(&self, ctx: &Ctx, n: u64, text_start: u64) -> Option<u64> {
        if n == 0 {
            return Some(text_start.min(self.len));
        }
        let g = n - 1; // この行の直前にある改行の通し番号
        if g >= self.total_nl {
            return None;
        }
        // g 番目の改行を含むピース（nls[k] + nl[k] > g となる最初の k）
        let (mut lo, mut hi) = (0, self.pieces.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.nls[mid] + self.pieces[mid].nl <= g {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let i = lo;
        let p = &self.pieces[i];
        let local = g - self.nls[i];
        let within = match &p.src {
            Src::Orig => {
                let before = ctx.index.line_of_offset(p.start)?;
                ctx.index.line_start(before + local + 1)? - p.start
            }
            Src::Add(chunk) => {
                let bytes = &chunk[p.start as usize..(p.start + p.len) as usize];
                // 同じピースで、これより前の改行まで調べてあれば、そこから続ける
                let mut cache = self.scan.0.try_lock().ok();
                let slot = cache.as_ref().and_then(|c| c.iter().enumerate().filter(|(_, e)| e.0 == i && e.1 <= local).max_by_key(|(_, e)| e.1).map(|(k, e)| (k, e.1, e.2)));
                let (mut at, mut pos) = slot.map_or((0, 0), |(_, l, p)| (l + 1, p));
                if let Some((_, l, p)) = slot {
                    if l == local {
                        return Some(self.starts[i] + p as u64);
                    }
                    at = l + 1;
                    pos = p;
                }
                while at <= local {
                    pos = next_line_start(bytes, pos, ctx.kind)?;
                    at += 1;
                }
                if let Some(c) = cache.as_mut() {
                    match slot {
                        Some((k, ..)) => c[k] = (i, local, pos),
                        None if c.len() < SCAN_SLOTS => c.push((i, local, pos)),
                        None => c[(local as usize) % SCAN_SLOTS] = (i, local, pos),
                    }
                }
                pos as u64
            }
        };
        Some(self.starts[i] + within)
    }

    /// 位置 `pos` を含む行の番号
    pub fn line_of_offset(&self, ctx: &Ctx, pos: u64) -> u64 {
        if self.pieces.is_empty() {
            return 0;
        }
        let pos = pos.min(self.len);
        let i = self.piece_at(pos);
        let p = &self.pieces[i];
        let off = pos - self.starts[i];
        let inner = match &p.src {
            Src::Orig => ctx.orig_nl(p.start, p.start + off),
            Src::Add(chunk) => count_nl(&chunk[p.start as usize..(p.start + off) as usize], ctx.kind),
        };
        self.nls[i] + inner
    }
}
