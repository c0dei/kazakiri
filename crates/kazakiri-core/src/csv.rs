//! CSV / TSV：フィールドの分け方、区切り文字の判定、表示幅、列での並べ替え。
//!
//! 引用符（"..."）の中の区切り文字は区切りとみなさない。引用符の中の改行（複数行にまたがる
//! フィールド）は扱わず、1 行を 1 レコードとして扱う。

use std::cmp::Ordering;
use std::ops::Range;

use crate::document::Document;
use crate::search::SearchControl;

/// 並べ替えできる最大行数（キーをメモリに持つため）
pub const SORT_LIMIT: u64 = 5_000_000;

/// 1 行をフィールドに分ける。戻り値は各フィールドの、`line` の中のバイト範囲（引用符を含む）
pub fn split_fields(line: &str, delim: char) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut prev_quote = false;
    for (i, c) in line.char_indices() {
        if c == '"' {
            // フィールドの先頭の " で引用開始。引用中の "" は " そのもの
            if !quoted && i == start {
                quoted = true;
            } else if quoted {
                prev_quote = !prev_quote;
            }
            continue;
        }
        if quoted && prev_quote {
            // 閉じの " のあと
            quoted = false;
            prev_quote = false;
        }
        if c == delim && !quoted {
            out.push(start..i);
            start = i + c.len_utf8();
        }
    }
    out.push(start..line.len());
    out
}

/// 引用符を外したフィールドの中身
pub fn unquote(field: &str) -> String {
    let t = field.trim_matches(|c| c == ' ');
    match t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => field.to_string(),
    }
}

/// 先頭の数行から区切り文字を推定する（タブ、カンマ、セミコロン、縦棒）
pub fn detect_delimiter(lines: &[&str]) -> char {
    let mut best = (',', 0usize);
    for d in ['\t', ',', ';', '|'] {
        let counts: Vec<usize> = lines.iter().filter(|l| !l.is_empty()).map(|l| split_fields(l, d).len()).collect();
        if counts.is_empty() {
            continue;
        }
        // 各行で同じ数に分かれ、しかも 2 列以上になるものを選ぶ
        let first = counts[0];
        let consistent = counts.iter().filter(|&&c| c == first).count();
        let score = if first >= 2 { consistent * first } else { 0 };
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

/// 等幅フォントでの表示幅（半角 1、全角 2）
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

pub fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF |
        0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 |
        0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortOptions {
    pub column: usize,
    pub delim: char,
    pub descending: bool,
    /// 1 行目を見出しとして先頭に残す
    pub header: bool,
}

/// 並べ替えのキー：数値として読めれば数値で、それ以外は文字列で比べる（数値が先）
#[derive(Debug)]
enum Key {
    Num(f64),
    Text(String),
}

fn key_of(field: &str) -> Key {
    let t = unquote(field);
    let n = t.trim().replace(',', "");
    match n.parse::<f64>() {
        Ok(v) if !n.is_empty() && v.is_finite() => Key::Num(v),
        _ => Key::Text(t),
    }
}

fn cmp_key(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Num(x), Key::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Key::Num(_), Key::Text(_)) => Ordering::Less,
        (Key::Text(_), Key::Num(_)) => Ordering::Greater,
        (Key::Text(x), Key::Text(y)) => x.cmp(y),
    }
}

/// 列で並べ替えた内容（ファイルの文字コードのバイト列）を作る。
/// 本文全体（BOM の後ろから末尾まで）をこれで置き換えれば並べ替えになる
pub fn sort_document(doc: &Document, opts: SortOptions, ctl: &SearchControl) -> Result<Vec<u8>, String> {
    let (count, exact) = doc.line_count();
    if !exact {
        return Err("索引の作成中は並べ替えできません".into());
    }
    if count > SORT_LIMIT {
        return Err(format!("行が多すぎて並べ替えできません（{} 万行まで）", SORT_LIMIT / 10_000));
    }
    // 最後の行が空（ファイルが改行で終わる）なら並べ替えの対象にしない
    let last_empty = doc.line_range(count - 1).is_some_and(|(s, e)| s == e);
    let body_end = if last_empty { count - 1 } else { count };
    let first = if opts.header { 1.min(body_end) } else { 0 };

    let mut rows: Vec<(Key, u64, u64)> = Vec::with_capacity((body_end - first) as usize);
    for i in first..body_end {
        if i % 65536 == 0 && ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("並べ替えを中止しました".into());
        }
        let (s, e) = doc.line_range(i).ok_or("行の位置が分かりません")?;
        let bytes = doc.read(s, e);
        let text = doc.decode(&bytes);
        let fields = split_fields(&text, opts.delim);
        let key = fields.get(opts.column).map_or(Key::Text(String::new()), |r| key_of(&text[r.clone()]));
        rows.push((key, s, e));
        ctl.scanned.fetch_add(e - s, std::sync::atomic::Ordering::Relaxed);
    }
    rows.sort_by(|a, b| {
        let o = cmp_key(&a.0, &b.0);
        if opts.descending { o.reverse() } else { o }
    });

    let nl = doc.encode(match doc.newline_name() {
        "CRLF" => "\r\n",
        "CR" => "\r",
        _ => "\n",
    });
    let mut out = Vec::with_capacity(doc.len() as usize);
    let push_line = |out: &mut Vec<u8>, s: u64, e: u64| {
        out.extend_from_slice(&doc.read(s, e));
        out.extend_from_slice(&nl);
    };
    if first == 1 {
        let (s, e) = doc.line_range(0).unwrap();
        push_line(&mut out, s, e);
    }
    for (_, s, e) in &rows {
        push_line(&mut out, *s, *e);
    }
    // 元が改行で終わっていなければ、最後の改行は付けない
    if !last_empty && !out.is_empty() {
        out.truncate(out.len() - nl.len());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_quotes() {
        let l = r#"1,"Tokyo, Japan","say ""hi""",,end"#;
        let f: Vec<&str> = split_fields(l, ',').into_iter().map(|r| &l[r]).collect();
        assert_eq!(f, ["1", r#""Tokyo, Japan""#, r#""say ""hi""""#, "", "end"]);
        assert_eq!(unquote(f[1]), "Tokyo, Japan");
        assert_eq!(unquote(f[2]), r#"say "hi""#);
        let t = "a\tb\t\tc";
        assert_eq!(split_fields(t, '\t').len(), 4);
        // 途中の " は引用の開始ではない
        let l = r#"ab"c,d"#;
        assert_eq!(split_fields(l, ',').len(), 2);
    }

    #[test]
    fn delimiter_detection() {
        assert_eq!(detect_delimiter(&["a,b,c", "1,2,3"]), ',');
        assert_eq!(detect_delimiter(&["a\tb\tc", "1\t2,5\t3"]), '\t');
        assert_eq!(detect_delimiter(&["a;b;c", "1;2,5;3"]), ';');
    }

    #[test]
    fn widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("東京ｱ"), 5);
    }

    fn ready(bytes: &[u8]) -> Document {
        let d = Document::from_bytes(bytes.to_vec());
        while !d.progress().done {
            std::thread::yield_now();
        }
        d
    }

    #[test]
    fn sort_by_column() {
        let d = ready(b"id,name,score\r\n3,bob,10\r\n1,alice,9.5\r\n2,\"carol, jr\",100\r\n");
        let o = SortOptions { column: 2, delim: ',', descending: false, header: true };
        let out = sort_document(&d, o, &SearchControl::default()).unwrap();
        assert_eq!(out, b"id,name,score\r\n1,alice,9.5\r\n3,bob,10\r\n2,\"carol, jr\",100\r\n");
        let o = SortOptions { column: 1, descending: true, ..o };
        let out = sort_document(&d, o, &SearchControl::default()).unwrap();
        assert_eq!(out, b"id,name,score\r\n2,\"carol, jr\",100\r\n3,bob,10\r\n1,alice,9.5\r\n");
        // 改行で終わらないファイル、見出しなし
        let d = ready(b"b\na\nc");
        let o = SortOptions { column: 0, delim: ',', descending: false, header: false };
        assert_eq!(sort_document(&d, o, &SearchControl::default()).unwrap(), b"a\nb\nc");
    }
}
