//! 開いているファイル 1 つぶんの状態（いまは閲覧専用）。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

use crate::encoding::{self, Detected};
use crate::line_index::{next_line_start, LineIndex, Newline, Progress};
use crate::source::Source;

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

pub struct Document {
    path: Option<PathBuf>,
    source: Arc<Source>,
    detected: Detected,
    index: LineIndex,
    forced: Option<&'static Encoding>,
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
        let index = LineIndex::build(source.clone(), kind, detected.bom_len as u64);
        Document { path: None, source, detected, index, forced }
    }

    /// ディスク上のファイルを確認し、追記だけなら索引を引き継いで読み直す
    pub fn refresh(&self) -> io::Result<Refresh> {
        let Some(path) = &self.path else { return Ok(Refresh::Unchanged) };
        let new_len = std::fs::metadata(path)?.len();
        let old_len = self.len();
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
            index,
            forced: self.forced,
        }))
    }

    /// 文字コードを指定して開いたか（`None` なら自動判定）
    pub fn forced_encoding(&self) -> Option<&'static Encoding> {
        self.forced
    }

    /// ファイル全体のバイト列
    pub fn data(&self) -> &[u8] {
        self.source.bytes()
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
        // ログのローテーション相当：別ファイルを作って置き換える
        let rotated = dir.join("grow.log.new");
        std::fs::write(&rotated, "y\nzzzz\n").unwrap();
        std::fs::rename(&rotated, &path).unwrap();
        assert!(matches!(doc3.refresh().unwrap(), Refresh::Replaced));
        drop(doc3);
        let _ = std::fs::remove_dir_all(&dir);
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
