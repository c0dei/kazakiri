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

/// 列フィルタの条件
#[derive(Debug)]
pub enum Cond {
    /// 等しい
    Eq(String),
    /// 等しくない
    Ne(String),
    /// 含む
    Contains(String),
    /// 正規表現に一致
    Regex(regex::Regex),
    /// 数値として比較（数値でない値は一致しない）
    Num(std::cmp::Ordering, bool, f64),
    Empty,
    NotEmpty,
}

/// 「3 = ERROR」のような、1 つの列に対する条件
#[derive(Debug)]
pub struct Filter {
    /// 0 から数えた列
    pub column: usize,
    pub cond: Cond,
    /// 文字列の比較で大文字と小文字を区別しない
    pub ignore_case: bool,
}

impl Filter {
    /// 書式（列は 1 から数える）:
    /// `3 = 値`、`3 != 値`、`3 : 含む語`、`3 ~ 正規表現`、`3 >= 100`（`> < >= <=`）、`3 empty`、`3 !empty`
    pub fn parse(text: &str, ignore_case: bool) -> Result<Filter, String> {
        let t = text.trim();
        let split = t.find(|c: char| !c.is_ascii_digit()).ok_or("「列 条件 値」の形で書いてください（例: 3 = ERROR）")?;
        let column: usize = t[..split].parse().map_err(|_| "列の番号が読めません（例: 3 = ERROR）".to_string())?;
        if column == 0 {
            return Err("列は 1 から数えます".into());
        }
        let rest = t[split..].trim_start();
        let (op, value) = ["!empty", "empty", ">=", "<=", "!=", "=", ":", "~", ">", "<"]
            .iter()
            .find_map(|op| rest.strip_prefix(op).map(|v| (*op, v.trim())))
            .ok_or("条件は = != : ~ > < >= <= empty !empty のどれかです")?;
        let cond = match op {
            "empty" => Cond::Empty,
            "!empty" => Cond::NotEmpty,
            "=" => Cond::Eq(value.to_string()),
            "!=" => Cond::Ne(value.to_string()),
            ":" => Cond::Contains(value.to_string()),
            "~" => {
                let re = regex::RegexBuilder::new(value).case_insensitive(ignore_case).build().map_err(|e| format!("正規表現が正しくありません: {e}"))?;
                Cond::Regex(re)
            }
            _ => {
                let n: f64 = value.replace(',', "").parse().map_err(|_| format!("数値が読めません: {value}"))?;
                match op {
                    ">" => Cond::Num(Ordering::Greater, false, n),
                    ">=" => Cond::Num(Ordering::Greater, true, n),
                    "<" => Cond::Num(Ordering::Less, false, n),
                    _ => Cond::Num(Ordering::Less, true, n),
                }
            }
        };
        if matches!(cond, Cond::Eq(_) | Cond::Ne(_) | Cond::Contains(_)) && value.is_empty() {
            return Err("比較する値が空です（空の列を探すなら 「3 empty」）".into());
        }
        Ok(Filter { column: column - 1, cond, ignore_case })
    }

    fn matches_field(&self, field: &str) -> bool {
        let v = unquote(field);
        let v = v.trim();
        let eq = |a: &str, b: &str| if self.ignore_case { a.to_lowercase() == b.to_lowercase() } else { a == b };
        match &self.cond {
            Cond::Eq(x) => eq(v, x),
            Cond::Ne(x) => !eq(v, x),
            Cond::Contains(x) => {
                if self.ignore_case {
                    v.to_lowercase().contains(&x.to_lowercase())
                } else {
                    v.contains(x.as_str())
                }
            }
            Cond::Regex(re) => re.is_match(v),
            Cond::Num(ord, or_equal, n) => match v.replace(',', "").parse::<f64>() {
                Ok(x) if x.is_finite() => {
                    let c = x.partial_cmp(n).unwrap_or(Ordering::Equal);
                    c == *ord || (*or_equal && c == Ordering::Equal)
                }
                _ => false,
            },
            Cond::Empty => v.is_empty(),
            Cond::NotEmpty => !v.is_empty(),
        }
    }

    /// 1 行（改行を含まない）が条件に合うか。その列がない行は、空の列として扱う
    pub fn matches_line(&self, line: &str, delim: char) -> bool {
        let fields = split_fields(line, delim);
        let field = fields.get(self.column).map_or("", |r| &line[r.clone()]);
        self.matches_field(field)
    }
}

/// 列フィルタに合う行を探す。戻り値の範囲は各行の [開始, 終了)（改行を含まない）。
/// `header` なら 1 行目は対象にしない
pub fn filter_lines(doc: &Document, delim: char, header: bool, f: &Filter, limit: usize, ctl: &SearchControl) -> Result<crate::search::FindAll, String> {
    use std::sync::atomic::Ordering as AO;
    let (count, exact) = doc.line_count();
    if !exact {
        return Err("索引の作成中は列フィルタを使えません".into());
    }
    const CHUNK: u64 = 20_000;
    let chunks: Vec<(usize, usize)> = (0..count.div_ceil(CHUNK)).map(|i| ((i * CHUNK) as usize, ((i + 1) * CHUNK).min(count) as usize)).collect();
    let mut out = crate::search::FindAll::default();
    let total = std::sync::atomic::AtomicUsize::new(0);
    for window in chunks.chunks(crate::search::threads()) {
        if ctl.cancel.load(AO::Relaxed) {
            out.cancelled = true;
            break;
        }
        let parts = crate::search::run_parallel(window, |&(a, b)| {
            if ctl.cancel.load(AO::Relaxed) || total.load(AO::Relaxed) > limit {
                return None;
            }
            let mut hits = Vec::new();
            for line in doc.lines(a as u64, b - a) {
                if header && line.number == 0 {
                    continue;
                }
                if f.matches_line(&line.text, delim) {
                    hits.push((line.start, line.end));
                }
            }
            total.fetch_add(hits.len(), AO::Relaxed);
            ctl.scanned.fetch_add((b - a) as u64, AO::Relaxed);
            Some(hits)
        });
        for part in parts.into_iter().flatten() {
            out.ranges.extend(part);
        }
        if out.ranges.len() > limit {
            out.ranges.truncate(limit);
            out.truncated = true;
            break;
        }
    }
    Ok(out)
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
    fn filter_parse_and_match() {
        let f = Filter::parse("2 = bob", true).unwrap();
        assert!(f.matches_line("1,BOB,3", ','));
        assert!(!f.matches_line("1,bobby,3", ','));
        let f = Filter::parse("3 >= 1,000", false).unwrap();
        assert!(f.matches_line("a,b,\"1,500\"", ','));
        assert!(!f.matches_line("a,b,999", ','));
        assert!(!f.matches_line("a,b,x", ','));
        let f = Filter::parse("2 : err", true).unwrap();
        assert!(f.matches_line("1,An ERROR here,3", ','));
        let f = Filter::parse("3 empty", false).unwrap();
        assert!(f.matches_line("a,b", ',') && f.matches_line("a,b,", ',') && !f.matches_line("a,b,c", ','));
        let f = Filter::parse("1 ~ ^\\d+$", false).unwrap();
        assert!(f.matches_line("123,x", ',') && !f.matches_line("12a,x", ','));
        let f = Filter::parse("2 != x", false).unwrap();
        assert!(f.matches_line("1,y", ',') && !f.matches_line("1,x", ','));
        for bad in ["", "abc", "0 = x", "2", "2 ?? x", "2 > abc", "2 = ", "2 ~ ("] {
            assert!(Filter::parse(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn filter_lines_over_document() {
        let text: String = std::iter::once("id,name,score\r\n".to_string()).chain((1..=50_000).map(|i| format!("{i},n{},{}\r\n", i % 7, i % 100))).collect();
        let d = ready(text.as_bytes());
        let f = Filter::parse("3 >= 98", false).unwrap();
        let r = filter_lines(&d, ',', true, &f, usize::MAX, &SearchControl::default()).unwrap();
        assert_eq!(r.ranges.len(), 50_000 / 100 * 2);
        let (s, e) = r.ranges[0];
        assert_eq!(&text.as_bytes()[s as usize..e as usize], b"98,n0,98");
        // 見出しを含めると「id」の行も対象になりうる（ここでは数値でないので一致しない）
        let f = Filter::parse("2 = n3", false).unwrap();
        let r = filter_lines(&d, ',', true, &f, 100, &SearchControl::default()).unwrap();
        assert!(r.truncated && r.ranges.len() == 100);
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
