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

#[derive(Clone, Debug)]
pub(crate) struct Pieces {
    pieces: Vec<Piece>,
    /// 各ピースの開始位置（編集後のテキストでの位置）
    starts: Vec<u64>,
    /// 各ピースより前にある改行の数
    nls: Vec<u64>,
    len: u64,
    total_nl: u64,
}

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
        let mut p = Pieces { pieces: Vec::new(), starts: Vec::new(), nls: Vec::new(), len: 0, total_nl: 0 };
        if len > 0 {
            p.pieces.push(Piece { src: Src::Orig, start: 0, len, nl: ctx.orig_nl(0, len) });
        }
        p.rebuild();
        p
    }

    fn rebuild(&mut self) {
        self.starts.clear();
        self.nls.clear();
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
        let b = b.min(self.len);
        let ja = self.split(ctx, a);
        let jb = if b > a { self.split(ctx, b) } else { ja };
        let removed: Vec<Piece> = self.pieces.splice(ja..jb, new).collect();
        self.rebuild();
        removed
    }

    /// [a, b) を `bytes` で置き換え、取り除いたピース列を返す
    pub fn replace(&mut self, ctx: &Ctx, a: u64, b: u64, bytes: &[u8]) -> Vec<Piece> {
        let removed = if b > a { self.splice(ctx, a, b, Vec::new()) } else { Vec::new() };
        self.insert(ctx, a, bytes);
        removed
    }

    pub fn insert(&mut self, ctx: &Ctx, pos: u64, bytes: &[u8]) {
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
                let mut pos = 0;
                for _ in 0..=local {
                    pos = next_line_start(bytes, pos, ctx.kind)?;
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
