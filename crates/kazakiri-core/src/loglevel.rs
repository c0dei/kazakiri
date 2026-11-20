//! ログレベル（ERROR・WARN など）の判定と、レベルでの絞り込み。色分けと絞り込みで同じ規則を使う。
//!
//! 行の先頭の 512 文字の中で、レベルの語が「単語として」現れた最初のものを、その行のレベルとする（大文字小文字は区別しない）。
//! レベルのない行（スタックトレースなど）は、直前のレベルのある行の続きとして、同じ扱いにする。

use crate::document::Document;
use crate::logtime::{filter_records, parse_timestamp, TimeRange};
use crate::search::{FindAll, SearchControl};

/// 重大度の順（小さいほど軽い）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

const ALL: [Level; 6] = [Level::Trace, Level::Debug, Level::Info, Level::Warn, Level::Error, Level::Fatal];

const WORDS: [(&str, Level); 14] = [
    ("TRACE", Level::Trace),
    ("VERBOSE", Level::Trace),
    ("DEBUG", Level::Debug),
    ("INFO", Level::Info),
    ("INFORMATION", Level::Info),
    ("NOTICE", Level::Info),
    ("WARN", Level::Warn),
    ("WARNING", Level::Warn),
    ("ERROR", Level::Error),
    ("ERR", Level::Error),
    ("SEVERE", Level::Error),
    ("FATAL", Level::Fatal),
    ("CRITICAL", Level::Fatal),
    ("EMERG", Level::Fatal),
];

/// 判定する先頭の文字数
const HEAD_CHARS: usize = 512;

fn word_level(w: &str) -> Option<Level> {
    WORDS.iter().find(|(n, _)| n.eq_ignore_ascii_case(w)).map(|&(_, l)| l)
}

/// レベルの語の位置（バイト）と種類を、出てくる順に返す
pub fn find_levels(text: &str) -> Vec<(usize, usize, Level)> {
    let head = match text.char_indices().nth(HEAD_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    };
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in head.char_indices().chain(std::iter::once((head.len(), ' '))) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(a)) => {
                start = None;
                if let Some(l) = word_level(&head[a..i]) {
                    out.push((a, i, l));
                }
            }
            _ => {}
        }
    }
    out
}

/// その行のレベル（最初に出てくるレベルの語）
pub fn line_level(text: &str) -> Option<Level> {
    find_levels(text).first().map(|x| x.2)
}

/// 絞り込みの指定：残すレベルの集合
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelSpec {
    mask: u8,
}

fn bit(l: Level) -> u8 {
    1 << l as u8
}

impl LevelSpec {
    /// `ERROR,WARN`（複数。区切りは `,` 空白 `|`）、`>=WARN`（その重大度以上）、`>WARN`（より重い）
    pub fn parse(spec: &str) -> Result<LevelSpec, String> {
        let mut mask = 0u8;
        for tok in spec.split(|c: char| c == ',' || c == '|' || c == '、' || c.is_whitespace()).filter(|t| !t.is_empty()) {
            let (op, name) = if let Some(n) = tok.strip_prefix(">=") {
                (1, n)
            } else if let Some(n) = tok.strip_prefix('>') {
                (2, n)
            } else {
                (0, tok)
            };
            let l = word_level(name).ok_or_else(|| format!("レベルが分かりません：{name}（TRACE・DEBUG・INFO・WARN・ERROR・FATAL）"))?;
            for x in ALL {
                let hit = match op {
                    0 => x == l,
                    1 => x >= l,
                    _ => x > l,
                };
                if hit {
                    mask |= bit(x);
                }
            }
        }
        if mask == 0 {
            return Err("レベルを入力してください（例: ERROR,WARN / >=WARN）".into());
        }
        Ok(LevelSpec { mask })
    }

    pub fn contains(&self, l: Level) -> bool {
        self.mask & bit(l) != 0
    }
}

/// 指定のレベルの行（と、その続きの行）を、連続した行のまとまりごとの範囲で返す。戻り値は `filter_time` と同じ
pub fn filter_level(doc: &Document, spec: LevelSpec, limit: usize, ctl: &SearchControl) -> Result<(FindAll, u64), String> {
    filter_records(doc, limit, ctl, |text| line_level(text).map(|l| spec.contains(l)))
}

/// レベルと時刻の範囲の両方に合う記録を返す。記録の先頭は「時刻のある行」で、その行にレベルがなければ合わないとする
pub fn filter_level_time(doc: &Document, spec: LevelSpec, range: TimeRange, limit: usize, ctl: &SearchControl) -> Result<(FindAll, u64), String> {
    filter_records(doc, limit, ctl, |text| parse_timestamp(text).map(|t| range.contains(t) && line_level(text).is_some_and(|l| spec.contains(l))))
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
    fn finds_words_only() {
        let r = find_levels("2026 [ERROR] x WARN y ERRORS debug");
        let kinds: Vec<_> = r.iter().map(|x| x.2).collect();
        assert_eq!(kinds, [Level::Error, Level::Warn, Level::Debug]);
        assert_eq!((r[0].0, r[0].1), (6, 11));
        assert_eq!(line_level("INFO: error handling"), Some(Level::Info));
        assert_eq!(line_level("    at Foo.bar(Foo.java:12)"), None);
        assert_eq!(line_level("INFORMED"), None);
    }

    #[test]
    fn parses_specs() {
        let s = LevelSpec::parse("ERROR, warn").unwrap();
        assert!(s.contains(Level::Error) && s.contains(Level::Warn) && !s.contains(Level::Fatal) && !s.contains(Level::Info));
        let s = LevelSpec::parse(">=WARN").unwrap();
        assert!(s.contains(Level::Warn) && s.contains(Level::Error) && s.contains(Level::Fatal) && !s.contains(Level::Info));
        let s = LevelSpec::parse(">WARN").unwrap();
        assert!(!s.contains(Level::Warn) && s.contains(Level::Error));
        assert!(LevelSpec::parse("").is_err());
        assert!(LevelSpec::parse("LOUD").is_err());
    }

    #[test]
    fn filter_keeps_continuation_lines() {
        let d = doc("INFO a\nERROR b\n  at x\n  at y\nINFO c\nWARN d\nDEBUG e\n");
        let (f, n) = filter_level(&d, LevelSpec::parse("ERROR").unwrap(), 100, &SearchControl::default()).unwrap();
        assert_eq!((f.ranges.len(), n), (1, 3));
        let (s, e) = f.ranges[0];
        assert_eq!(d.decode(&d.read(s, e)).into_owned(), "ERROR b\n  at x\n  at y");
        let (f, n) = filter_level(&d, LevelSpec::parse(">=WARN").unwrap(), 100, &SearchControl::default()).unwrap();
        assert_eq!((f.ranges.len(), n), (2, 4));
    }

    #[test]
    fn filter_across_chunks() {
        // チャンクの境目（20,000 行）で続きの行が始まる場合
        let mut s = String::new();
        for i in 0..60_000u64 {
            s.push_str(if i % 1000 == 0 { "ERROR e\n" } else { "INFO i\n" });
            if i == 19_999 {
                s.push_str("  continued\n");
            }
        }
        let d = doc(&s);
        let (_, n) = filter_level(&d, LevelSpec::parse("ERROR").unwrap(), 1000, &SearchControl::default()).unwrap();
        assert_eq!(n, 60);
        let (_, n) = filter_level(&d, LevelSpec::parse("INFO").unwrap(), 1000, &SearchControl::default()).unwrap();
        // INFO の行 59,940 + 境目の続きの行 1 + 末尾の改行のあとの空行 1
        assert_eq!(n, 60_000 - 60 + 2);
    }

    #[test]
    fn filter_level_and_time() {
        let d = doc("2026-11-18 11:59:00 ERROR early
2026-11-18 12:10:00 INFO ok
2026-11-18 12:20:00 ERROR boom
  at a.b(c)
2026-11-18 12:30:00 no level here
2026-11-18 13:10:00 ERROR late
");
        let r = TimeRange::parse("2026-11-18 12:00 ~ 13:00").unwrap();
        let (f, n) = filter_level_time(&d, LevelSpec::parse("ERROR").unwrap(), r, 100, &SearchControl::default()).unwrap();
        assert_eq!((f.ranges.len(), n), (1, 2));
        let (s, e) = f.ranges[0];
        assert_eq!(d.decode(&d.read(s, e)).into_owned(), "2026-11-18 12:20:00 ERROR boom
  at a.b(c)");
    }
}
