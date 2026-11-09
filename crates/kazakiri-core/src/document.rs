//! 開いているファイル 1 つぶんの状態。
//!
//! 編集していないあいだは、元ファイル（メモリマップ）と行インデックスをそのまま使う。
//! 編集するとピーステーブル（`pieces`）を作り、以降の読み出しはそれを通す。
//! `Document` は複製が安く（中身は `Arc`）、編集は新しい `Document` を返す。

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

use crate::encoding::{self, Detected};
use crate::line_index::{line_start_at_or_after, line_start_at_or_before, next_line_start, LineIndex, Newline, Progress};
use crate::pieces::{Ctx, Piece, Pieces};
use crate::source::Source;

/// 編集できなかった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// 行インデックスの作成中
    NotReady,
    /// 位置がおかしい（BOM の中、末尾より後ろ、UTF-16 の奇数位置）
    OutOfRange,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NotReady => write!(f, "索引の作成中は編集できません"),
            EditError::OutOfRange => write!(f, "編集できない位置です"),
        }
    }
}

impl std::error::Error for EditError {}

/// ファイルを読み直した結果
pub enum Refresh {
    /// 変わっていない（大きさが同じ）
    Unchanged,
    /// 末尾に追記された。索引は増えた分だけ作り直す
    Appended(Document),
    /// 小さくなった・中身が変わった。最初から開き直す必要がある
    Replaced,
}

/// 1 行あたり表示用にデコードする上限（巨大な 1 行 JSON 対策）
pub const MAX_LINE_DISPLAY_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct Document {
    path: Option<PathBuf>,
    source: Arc<Source>,
    detected: Detected,
    index: Arc<LineIndex>,
    forced: Option<&'static Encoding>,
    /// 編集後のテキスト。`None` なら元ファイルのまま
    text: Option<Arc<Pieces>>,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub number: u64,
    pub start: u64,
    /// 改行を含まない行末
    pub end: u64,
    pub text: String,
    /// 表示上限で切り詰めたか
    pub truncated: bool,
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Document> {
        Self::open_with(path, None)
    }

    /// 文字コードを指定して開く（`None` なら自動判定）
    pub fn open_with(path: &Path, forced: Option<&'static Encoding>) -> io::Result<Document> {
        let source = Source::open(path)?;
        let mut doc = Self::from_source(source, forced);
        doc.path = Some(path.to_path_buf());
        Ok(doc)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Document {
        Self::from_source(Source::from_vec(bytes), None)
    }

    pub fn from_bytes_with(bytes: Vec<u8>, forced: Option<&'static Encoding>) -> Document {
        Self::from_source(Source::from_vec(bytes), forced)
    }

    fn from_source(source: Source, forced: Option<&'static Encoding>) -> Document {
        let source = Arc::new(source);
        let detected = match forced {
            None => encoding::detect(source.bytes()),
            Some(enc) => encoding::forced(enc, source.bytes()),
        };
        let kind = newline_kind(&detected, source.bytes());
        let index = Arc::new(LineIndex::build(source.clone(), kind, detected.bom_len as u64));
        Document { path: None, source, detected, index, forced, text: None }
    }

    /// ディスク上のファイルを確認し、追記だけなら索引を引き継いで読み直す
    pub fn refresh(&self) -> io::Result<Refresh> {
        let Some(path) = &self.path else { return Ok(Refresh::Unchanged) };
        // 編集中は読み直さない
        if self.text.is_some() {
            return Ok(Refresh::Unchanged);
        }
        let new_len = std::fs::metadata(path)?.len();
        let old_len = self.source.len();
        if new_len == old_len {
            return Ok(Refresh::Unchanged);
        }
        if new_len < old_len || old_len == 0 {
            return Ok(Refresh::Replaced);
        }
        let source = Source::open(path)?;
        // 先頭と、元の末尾付近が同じなら「追記」とみなす
        let old = self.source.bytes();
        let new = source.bytes();
        let head = old.len().min(4096);
        let tail = old.len().saturating_sub(4096);
        if new.len() < old.len() || old[..head] != new[..head] || old[tail..] != new[tail..old.len()] {
            return Ok(Refresh::Replaced);
        }
        let source = Arc::new(source);
        let Some(index) = LineIndex::extend(&self.index, source.clone()) else { return Ok(Refresh::Replaced) };
        Ok(Refresh::Appended(Document {
            path: self.path.clone(),
            source,
            detected: self.detected,
            index: Arc::new(index),
            forced: self.forced,
            text: None,
        }))
    }

    /// 文字コードを指定して開いたか（`None` なら自動判定）
    pub fn forced_encoding(&self) -> Option<&'static Encoding> {
        self.forced
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { data: self.source.bytes(), index: &self.index, kind: self.index.kind() }
    }

    /// 編集していなければ元ファイルのバイト列
    pub(crate) fn raw(&self) -> Option<&[u8]> {
        self.text.is_none().then(|| self.source.bytes())
    }

    /// 本文の開始位置（BOM の直後）
    pub fn text_start(&self) -> usize {
        self.detected.bom_len.min(self.source.bytes().len())
    }

    /// バイト列をこのファイルの文字コードで文字列にする
    pub fn decode<'a>(&self, bytes: &'a [u8]) -> std::borrow::Cow<'a, str> {
        encoding::decode(&self.detected, bytes)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn len(&self) -> u64 {
        match &self.text {
            Some(t) => t.len(),
            None => self.source.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// バイト列 [start, end)。編集していない範囲はコピーせずに返す
    pub fn read(&self, start: u64, end: u64) -> Cow<'_, [u8]> {
        match &self.text {
            Some(t) => t.read(&self.ctx(), start, end),
            None => {
                let data = self.source.bytes();
                let e = (end as usize).min(data.len());
                Cow::Borrowed(&data[(start as usize).min(e)..e])
            }
        }
    }

    pub fn encoding(&self) -> &Detected {
        &self.detected
    }

    pub fn newline(&self) -> Newline {
        self.index.kind()
    }

    /// 改行コードの表示名（先頭行から判断）
    pub fn newline_name(&self) -> &'static str {
        match self.index.kind() {
            Newline::Cr => "CR",
            Newline::Lf => match self.line_range(0) {
                Some((_, end)) if self.read(end, end + 1).first() == Some(&b'\r') => "CRLF",
                _ => "LF",
            },
            Newline::Utf16Le | Newline::Utf16Be => {
                let unit = if self.index.kind() == Newline::Utf16Le { [0x0D, 0] } else { [0, 0x0D] };
                match self.line_range(0) {
                    Some((_, end)) if *self.read(end, end + 2) == unit[..] => "CRLF",
                    _ => "LF",
                }
            }
        }
    }

    pub fn progress(&self) -> Progress {
        self.index.progress()
    }

    /// (行数, 確定しているか)。索引作成中は推定値
    pub fn line_count(&self) -> (u64, bool) {
        match &self.text {
            Some(t) => (t.total_lines(), true),
            None => self.index.total_lines(),
        }
    }

    pub fn line_of_offset(&self, offset: u64) -> Option<u64> {
        match &self.text {
            Some(t) => Some(t.line_of_offset(&self.ctx(), offset)),
            None => self.index.line_of_offset(offset),
        }
    }

    /// 行の開始位置
    pub fn line_start(&self, line: u64) -> Option<u64> {
        match &self.text {
            Some(t) => t.line_start(&self.ctx(), line, self.text_start() as u64),
            None => self.index.line_start(line),
        }
    }

    /// 行の [開始, 終了)。終了は改行コードを含まない
    pub fn line_range(&self, line: u64) -> Option<(u64, u64)> {
        let start = self.line_start(line)?;
        let end = match &self.text {
            None => self.line_end(start as usize) as u64,
            Some(_) => match self.line_start(line + 1) {
                None => self.len(),
                Some(next) => next - self.terminator_len(start, next),
            },
        };
        Some((start, end))
    }

    /// `next`（行の開始位置）の直前にある改行コードの長さ。行の先頭 `start` より前は見ない
    fn terminator_len(&self, start: u64, next: u64) -> u64 {
        let tail = self.read(next.saturating_sub(4).max(start), next);
        let t = &tail[..];
        let n = t.len();
        match self.index.kind() {
            Newline::Cr => 1,
            Newline::Lf => {
                if n >= 2 && t[n - 2] == b'\r' { 2 } else { 1 }
            }
            Newline::Utf16Le => {
                if n >= 4 && t[n - 4..n - 2] == [0x0D, 0] { 4 } else { 2 }
            }
            Newline::Utf16Be => {
                if n >= 4 && t[n - 4..n - 2] == [0, 0x0D] { 4 } else { 2 }
            }
        }
    }

    /// `pos` 以前で最も後ろの行頭（検索のチャンク分け用）
    pub(crate) fn line_start_at_or_before(&self, pos: u64) -> u64 {
        let floor = self.text_start() as u64;
        match self.raw() {
            Some(data) => line_start_at_or_before(data, pos as usize, floor as usize, self.newline()) as u64,
            None => {
                let pos = pos.min(self.len());
                let n = self.line_of_offset(pos).unwrap_or(0);
                self.line_start(n).unwrap_or(floor)
            }
        }
    }

    /// `pos` 以降で最初の行頭（なければ末尾）
    pub(crate) fn line_start_at_or_after(&self, pos: u64) -> u64 {
        let floor = self.text_start() as u64;
        match self.raw() {
            Some(data) => line_start_at_or_after(data, pos as usize, floor as usize, self.newline()) as u64,
            None => {
                let len = self.len();
                if pos <= floor {
                    return floor;
                }
                if pos >= len {
                    return len;
                }
                let n = self.line_of_offset(pos).unwrap_or(0);
                match self.line_start(n) {
                    Some(s) if s == pos => pos,
                    _ => self.line_start(n + 1).unwrap_or(len),
                }
            }
        }
    }

    fn line_end(&self, start: usize) -> usize {
        let data = self.source.bytes();
        let kind = self.index.kind();
        match next_line_start(data, start, kind) {
            None => data.len(),
            Some(next) => match kind {
                Newline::Cr => next - 1,
                Newline::Lf => {
                    let lf = next - 1;
                    if lf > start && data[lf - 1] == b'\r' { lf - 1 } else { lf }
                }
                Newline::Utf16Le => {
                    let lf = next - 2;
                    if lf >= start + 2 && data[lf - 2] == 0x0D && data[lf - 1] == 0 { lf - 2 } else { lf }
                }
                Newline::Utf16Be => {
                    let lf = next - 2;
                    if lf >= start + 2 && data[lf - 2] == 0 && data[lf - 1] == 0x0D { lf - 2 } else { lf }
                }
            },
        }
    }

    /// `first` 行目から最大 `count` 行を表示用に取り出す。
    /// 開始位置がまだ分からない行（索引作成中）は返さない。
    pub fn lines(&self, first: u64, count: usize) -> Vec<Line> {
        if self.text.is_some() {
            return self.lines_edited(first, count);
        }
        let mut out = Vec::with_capacity(count);
        let Some(mut start) = self.index.line_start(first) else { return out };
        let data = self.source.bytes();
        let len = data.len() as u64;
        let kind = self.index.kind();
        for number in first..first + count as u64 {
            let end = self.line_end(start as usize) as u64;
            let mut take = (end - start) as usize;
            let truncated = take > MAX_LINE_DISPLAY_BYTES;
            if truncated {
                take = MAX_LINE_DISPLAY_BYTES;
                if self.detected.is_utf16() {
                    take &= !1;
                }
            }
            let bytes = &data[start as usize..start as usize + take];
            let text = encoding::decode(&self.detected, bytes).into_owned();
            out.push(Line { number, start, end, text, truncated });
            match next_line_start(data, start as usize, kind) {
                Some(n) if (n as u64) <= len => start = n as u64,
                _ => break,
            }
        }
        out
    }

    fn lines_edited(&self, first: u64, count: usize) -> Vec<Line> {
        let mut out = Vec::with_capacity(count);
        for number in first..first + count as u64 {
            let Some((start, end)) = self.line_range(number) else { break };
            let mut take = end - start;
            let truncated = take > MAX_LINE_DISPLAY_BYTES as u64;
            if truncated {
                take = MAX_LINE_DISPLAY_BYTES as u64;
                if self.detected.is_utf16() {
                    take &= !1;
                }
            }
            let text = self.decode(&self.read(start, start + take)).into_owned();
            out.push(Line { number, start, end, text, truncated });
        }
        out
    }

    // ---- カーソル移動 ----

    /// `bytes` の先頭の文字が何バイトか（文字コードごとの規則で求める）
    fn char_len(&self, bytes: &[u8]) -> usize {
        let Some(&b) = bytes.first() else { return 0 };
        let enc = self.detected.encoding;
        let n = if enc == encoding_rs::UTF_8 {
            match b {
                0xF0..=0xF7 => 4,
                0xE0..=0xEF => 3,
                0xC0..=0xDF => 2,
                _ => 1,
            }
        } else if self.detected.is_utf16() {
            let unit = if enc == UTF_16LE {
                u16::from_le_bytes([b, *bytes.get(1).unwrap_or(&0)])
            } else {
                u16::from_be_bytes([b, *bytes.get(1).unwrap_or(&0)])
            };
            if (0xD800..0xDC00).contains(&unit) { 4 } else { 2 }
        } else if enc == encoding_rs::SHIFT_JIS {
            if matches!(b, 0x81..=0x9F | 0xE0..=0xFC) { 2 } else { 1 }
        } else if enc == encoding_rs::EUC_JP {
            match b {
                0x8F => 3,
                0x8E | 0xA1..=0xFE => 2,
                _ => 1,
            }
        } else {
            1
        };
        n.min(bytes.len()).max(1)
    }

    /// 次の文字の位置。行末なら次の行の先頭（CRLF はひとまとまり）
    pub fn next_char(&self, pos: u64) -> u64 {
        let len = self.len();
        if pos >= len {
            return len;
        }
        let Some(line) = self.line_of_offset(pos) else { return pos };
        let Some((_, end)) = self.line_range(line) else { return pos };
        if pos >= end {
            return self.line_start(line + 1).unwrap_or(len);
        }
        let head = self.read(pos, (pos + 4).min(end));
        pos + self.char_len(&head) as u64
    }

    /// 前の文字の位置。行頭なら前の行の行末
    pub fn prev_char(&self, pos: u64) -> u64 {
        let floor = self.text_start() as u64;
        if pos <= floor {
            return floor;
        }
        let Some(line) = self.line_of_offset(pos) else { return pos };
        let Some((start, end)) = self.line_range(line) else { return pos };
        if pos <= start {
            return self.line_range(line - 1).map_or(floor, |(_, e)| e);
        }
        let pos = pos.min(end);
        // UTF-8 / UTF-16 は少し前から、それ以外は行頭から数える
        let from = if self.detected.encoding == encoding_rs::UTF_8 || self.detected.is_utf16() {
            pos.saturating_sub(8).max(start)
        } else {
            start
        };
        let from = if self.detected.is_utf16() { from + (from - start) % 2 } else { from };
        let buf = self.read(from, pos);
        let mut i = 0usize;
        let mut last = 0usize;
        // UTF-8 は途中から始めても、続きのバイト（10xxxxxx）を飛ばせば文字の頭が分かる
        if self.detected.encoding == encoding_rs::UTF_8 {
            while i < buf.len() && buf[i] & 0xC0 == 0x80 {
                i += 1;
            }
            last = i;
        }
        while i < buf.len() {
            last = i;
            i += self.char_len(&buf[i..]);
        }
        from + last as u64
    }

    /// 文字列をこのファイルの文字コードで表せるか
    pub fn can_encode(&self, s: &str) -> bool {
        let enc = self.detected.encoding;
        if enc == encoding_rs::UTF_8 || self.detected.is_utf16() {
            return true;
        }
        !enc.encode(s).2
    }

    // ---- 編集 ----

    /// 編集しているか（保存していない変更があるかどうかは `Editor` が管理する）
    pub fn is_edited(&self) -> bool {
        self.text.is_some()
    }

    /// いま編集できるか（行インデックスの完成が必要）
    pub fn can_edit(&self) -> bool {
        self.text.is_some() || self.index.is_done()
    }

    /// ピースの数（断片化の目安）
    pub fn piece_count(&self) -> usize {
        self.text.as_ref().map_or(1, |t| t.piece_count())
    }

    fn check_pos(&self, pos: u64) -> Result<(), EditError> {
        if pos < self.text_start() as u64 || pos > self.len() || (self.detected.is_utf16() && pos % 2 == 1) {
            Err(EditError::OutOfRange)
        } else {
            Ok(())
        }
    }

    fn editable_pieces(&self) -> Result<Pieces, EditError> {
        match &self.text {
            Some(t) => Ok((**t).clone()),
            None if self.index.is_done() => Ok(Pieces::new(&self.ctx())),
            None => Err(EditError::NotReady),
        }
    }

    fn check_range(&self, a: u64, b: u64) -> Result<(), EditError> {
        self.check_pos(a)?;
        self.check_pos(b)?;
        if a > b { Err(EditError::OutOfRange) } else { Ok(()) }
    }

    /// [a, b) を `bytes`（このファイルの文字コードのバイト列）で置き換えた新しい Document
    pub fn replace(&self, a: u64, b: u64, bytes: &[u8]) -> Result<Document, EditError> {
        self.replace_tracked(a, b, bytes).map(|(d, _)| d)
    }

    /// `replace` と同じだが、取り除いたピース列も返す（Undo 用）
    pub(crate) fn replace_tracked(&self, a: u64, b: u64, bytes: &[u8]) -> Result<(Document, Vec<Piece>), EditError> {
        self.check_range(a, b)?;
        let mut t = self.editable_pieces()?;
        let removed = t.replace(&self.ctx(), a, b, bytes);
        let mut doc = self.clone();
        doc.text = Some(Arc::new(t));
        Ok((doc, removed))
    }

    /// [a, b) をピース列で置き換える（Undo / Redo 用）
    pub(crate) fn splice(&self, a: u64, b: u64, pieces: Vec<Piece>) -> Result<(Document, Vec<Piece>), EditError> {
        self.check_range(a, b)?;
        let mut t = self.editable_pieces()?;
        let removed = t.splice(&self.ctx(), a, b, pieces);
        let mut doc = self.clone();
        doc.text = Some(Arc::new(t));
        Ok((doc, removed))
    }

    pub fn insert(&self, pos: u64, bytes: &[u8]) -> Result<Document, EditError> {
        self.replace(pos, pos, bytes)
    }

    pub fn delete(&self, a: u64, b: u64) -> Result<Document, EditError> {
        self.replace(a, b, &[])
    }

    // ---- 保存 ----

    /// 内容を書き出す。`written` に書いたバイト数を加え、`cancel` が立ったら中断する
    pub fn write_to(&self, w: &mut dyn Write, written: &AtomicU64, cancel: &AtomicBool) -> io::Result<()> {
        const BLOCK: usize = 4 << 20;
        let ctx = self.ctx();
        let whole = [self.source.bytes()];
        let segments: Box<dyn Iterator<Item = &[u8]>> = match &self.text {
            Some(t) => Box::new(t.segments(&ctx)),
            None => Box::new(whole.into_iter()),
        };
        for seg in segments {
            for block in seg.chunks(BLOCK) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "保存を中止しました"));
                }
                w.write_all(block)?;
                written.fetch_add(block.len() as u64, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// `target` と同じフォルダに一時ファイルとして書き出し、そのパスを返す。
    /// 書き出しが終わったら、この Document（と複製）をすべて閉じてから
    /// `commit_save` で置き換える（Windows ではマップ中のファイルは置き換えられないため）
    pub fn write_temp(&self, target: &Path, written: &AtomicU64, cancel: &AtomicBool) -> io::Result<PathBuf> {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let temp = target.with_file_name(format!(".{name}.kazakiri-save"));
        let result = (|| {
            let file = std::fs::File::create(&temp)?;
            let mut w = io::BufWriter::with_capacity(1 << 20, file);
            self.write_to(&mut w, written, cancel)?;
            let file = w.into_inner().map_err(|e| e.into_error())?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        Ok(temp)
    }

    /// 文字列を前方検索し、見つかったバイト範囲を返す（大文字小文字は区別）。
    /// 繰り返し検索するときは `search::Query` を使う
    pub fn find(&self, pattern: &str, from: u64) -> Option<(u64, u64)> {
        use crate::search::{Direction, Query, SearchControl, SearchOptions, SearchOutcome};
        let q = Query::new(self, pattern, SearchOptions { case_sensitive: true, regex: false }).ok()?;
        match q.find(self, from, Direction::Forward, &SearchControl::default()) {
            SearchOutcome::Found(s, e) => Some((s, e)),
            _ => None,
        }
    }

    /// 文字列をこのファイルの文字コードのバイト列にする
    pub fn encode(&self, s: &str) -> Vec<u8> {
        let enc = self.detected.encoding;
        if enc == UTF_16LE {
            s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
        } else if enc == UTF_16BE {
            s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
        } else {
            enc.encode(s).0.into_owned()
        }
    }
}

/// `write_temp` で書き出した一時ファイルで `target` を置き換える
pub fn commit_save(temp: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(temp, target)
}

fn newline_kind(det: &Detected, bytes: &[u8]) -> Newline {
    if det.encoding == UTF_16LE {
        return Newline::Utf16Le;
    }
    if det.encoding == UTF_16BE {
        return Newline::Utf16Be;
    }
    let sample = &bytes[det.bom_len.min(bytes.len())..bytes.len().min(encoding::SAMPLE_BYTES)];
    if memchr::memchr(b'\n', sample).is_none() && memchr::memchr(b'\r', sample).is_some() {
        Newline::Cr
    } else {
        Newline::Lf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(doc: &Document) {
        while !doc.progress().done {
            std::thread::yield_now();
        }
    }

    #[test]
    fn crlf_lines_and_find() {
        let doc = Document::from_bytes(b"alpha\r\nbeta\r\n\r\ngamma".to_vec());
        ready(&doc);
        assert_eq!(doc.line_count(), (4, true));
        assert_eq!(doc.newline_name(), "CRLF");
        let lines: Vec<_> = doc.lines(0, 10).into_iter().map(|l| l.text).collect();
        assert_eq!(lines, ["alpha", "beta", "", "gamma"]);
        let (s, _) = doc.find("gamma", 0).unwrap();
        assert_eq!(doc.line_of_offset(s), Some(3));
    }

    #[test]
    fn sjis_document() {
        let text = "一行目です\r\n二行目：エラー発生\r\n三行目\r\n".repeat(10);
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
        let doc = Document::from_bytes(bytes.into_owned());
        ready(&doc);
        assert_eq!(doc.encoding().encoding, encoding_rs::SHIFT_JIS);
        assert_eq!(doc.lines(1, 1)[0].text, "二行目：エラー発生");
        let (s, _) = doc.find("エラー", 0).unwrap();
        assert_eq!(doc.line_of_offset(s), Some(1));
    }

    #[test]
    fn utf16le_document() {
        let mut v = vec![0xFF, 0xFE];
        v.extend("あいう\r\nえお\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        let doc = Document::from_bytes(v);
        ready(&doc);
        assert_eq!(doc.newline_name(), "CRLF");
        let lines: Vec<_> = doc.lines(0, 10).into_iter().map(|l| l.text).collect();
        assert_eq!(lines, ["あいう", "えお", ""]);
        let (s, _) = doc.find("えお", 0).unwrap();
        assert_eq!(doc.line_of_offset(s), Some(1));
    }

    #[test]
    fn refresh_appended_file() {
        let dir = std::env::temp_dir().join(format!("kazakiri-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("grow.log");
        std::fs::write(&path, "a\nb\n").unwrap();
        let doc = Document::open(&path).unwrap();
        ready(&doc);
        assert!(matches!(doc.refresh().unwrap(), Refresh::Unchanged));
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"c\nd").unwrap();
        }
        let Refresh::Appended(doc2) = doc.refresh().unwrap() else { panic!("not appended") };
        ready(&doc2);
        assert_eq!(doc2.line_count(), (4, true));
        assert_eq!(doc2.lines(2, 5).into_iter().map(|l| l.text).collect::<Vec<_>>(), ["c", "d"]);
        drop(doc);
        drop(doc2);
        std::fs::write(&path, "x\n").unwrap();
        let doc3 = Document::open(&path).unwrap();
        ready(&doc3);
        // ログのローテーション相当：いまのファイルを別名にして、新しいファイルを作る
        // （開いている＝マップ中のファイルへの上書きの rename は Windows では拒否されるので、この順にする）
        std::fs::rename(&path, dir.join("grow.log.1")).unwrap();
        std::fs::write(&path, "y\nzzzz\n").unwrap();
        assert!(matches!(doc3.refresh().unwrap(), Refresh::Replaced));
        drop(doc3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caret_moves_by_character() {
        let cases: Vec<(Vec<u8>, Option<&'static Encoding>)> = vec![
            ("aあ😀b\r\nc".as_bytes().to_vec(), None),
            (encoding_rs::SHIFT_JIS.encode("aあｱb\r\nc").0.into_owned(), Some(encoding_rs::SHIFT_JIS)),
            (encoding_rs::EUC_JP.encode("aあｱb\r\nc").0.into_owned(), Some(encoding_rs::EUC_JP)),
            ("aあ😀b\r\nc".encode_utf16().flat_map(|u| u.to_le_bytes()).collect(), Some(encoding_rs::UTF_16LE)),
        ];
        for (bytes, enc) in cases {
            let doc = Document::from_bytes_with(bytes.clone(), enc);
            ready(&doc);
            // 先頭から右へ
            let mut stops = vec![0];
            let mut p = 0;
            while p < doc.len() {
                p = doc.next_char(p);
                stops.push(p);
            }
            // 文字数：a, あ, 😀/ｱ, b, 改行, c → 6 回で末尾
            assert_eq!(stops.len(), 7, "{enc:?} {stops:?}");
            // 左へ戻ると同じ位置を通る
            let mut back = vec![doc.len()];
            let mut p = doc.len();
            while p > 0 {
                p = doc.prev_char(p);
                back.push(p);
            }
            back.reverse();
            assert_eq!(back, stops, "{enc:?}");
        }
    }

    #[test]
    fn encodability() {
        let doc = Document::from_bytes_with(b"abc".to_vec(), Some(encoding_rs::SHIFT_JIS));
        assert!(doc.can_encode("日本語ｱ"));
        assert!(!doc.can_encode("😀"));
        let doc = Document::from_bytes(b"abc".to_vec());
        assert!(doc.can_encode("😀"));
    }

    #[test]
    fn forced_encoding() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("テスト\r\n");
        let doc = Document::from_bytes_with(bytes.into_owned(), Some(encoding_rs::UTF_8));
        ready(&doc);
        assert_eq!(doc.encoding().encoding, encoding_rs::UTF_8);
        assert_eq!(doc.forced_encoding(), Some(encoding_rs::UTF_8));
        let doc = Document::from_bytes_with(b"\xEF\xBB\xBFabc".to_vec(), Some(encoding_rs::UTF_8));
        assert_eq!(doc.encoding().bom_len, 3);
        let doc = Document::from_bytes_with(b"\xEF\xBB\xBFabc".to_vec(), Some(encoding_rs::SHIFT_JIS));
        assert_eq!(doc.encoding().bom_len, 0);
    }
}
