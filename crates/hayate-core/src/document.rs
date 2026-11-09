//! 開いているファイル 1 つぶんの状態（いまは閲覧専用）。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use encoding_rs::{UTF_16BE, UTF_16LE};

use crate::encoding::{self, Detected};
use crate::line_index::{next_line_start, LineIndex, Newline, Progress};
use crate::source::Source;

/// 1 行あたり表示用にデコードする上限（巨大な 1 行 JSON 対策）
pub const MAX_LINE_DISPLAY_BYTES: usize = 64 * 1024;

pub struct Document {
    path: Option<PathBuf>,
    source: Arc<Source>,
    detected: Detected,
    index: LineIndex,
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
        let source = Source::open(path)?;
        let mut doc = Self::from_source(source);
        doc.path = Some(path.to_path_buf());
        Ok(doc)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Document {
        Self::from_source(Source::from_vec(bytes))
    }

    fn from_source(source: Source) -> Document {
        let source = Arc::new(source);
        let detected = encoding::detect(source.bytes());
        let kind = newline_kind(&detected, source.bytes());
        let index = LineIndex::build(source.clone(), kind, detected.bom_len as u64);
        Document { path: None, source, detected, index }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn len(&self) -> u64 {
        self.source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// ファイルの生バイト列の一部
    pub fn slice(&self, start: u64, end: u64) -> &[u8] {
        let data = self.source.bytes();
        let e = (end as usize).min(data.len());
        &data[(start as usize).min(e)..e]
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
                Some((_, end)) if self.source.bytes().get(end as usize) == Some(&b'\r') => "CRLF",
                _ => "LF",
            },
            Newline::Utf16Le | Newline::Utf16Be => {
                let unit = if self.index.kind() == Newline::Utf16Le { [0x0D, 0] } else { [0, 0x0D] };
                match self.line_range(0) {
                    Some((_, end)) if self.source.bytes().get(end as usize..end as usize + 2) == Some(&unit[..]) => "CRLF",
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
        self.index.total_lines()
    }

    pub fn line_of_offset(&self, offset: u64) -> Option<u64> {
        self.index.line_of_offset(offset)
    }

    /// 行の [開始, 終了)。終了は改行コードを含まない
    pub fn line_range(&self, line: u64) -> Option<(u64, u64)> {
        let start = self.index.line_start(line)?;
        Some((start, self.line_end(start as usize) as u64))
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

    /// 文字列を前方検索し、見つかったバイト範囲を返す（大文字小文字は区別）
    pub fn find(&self, pattern: &str, from: u64) -> Option<(u64, u64)> {
        self.searcher(pattern).find(from)
    }

    /// 同じ語を繰り返し検索するときはこちら（検索器を使い回す）
    pub fn searcher(&self, pattern: &str) -> Searcher<'_> {
        let needle = self.encode(pattern);
        Searcher { doc: self, finder: memchr::memmem::Finder::new(&needle).into_owned() }
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
}

pub struct Searcher<'a> {
    doc: &'a Document,
    finder: memchr::memmem::Finder<'static>,
}

impl Searcher<'_> {
    pub fn find(&self, from: u64) -> Option<(u64, u64)> {
        let n = self.finder.needle().len();
        if n == 0 {
            return None;
        }
        let data = self.doc.source.bytes();
        let utf16 = self.doc.detected.is_utf16();
        let mut pos = (from as usize).max(self.doc.detected.bom_len);
        while pos <= data.len() {
            let p = pos + self.finder.find(&data[pos..])?;
            if !utf16 || p.is_multiple_of(2) {
                return Some((p as u64, (p + n) as u64));
            }
            pos = p + 1;
        }
        None
    }
}
