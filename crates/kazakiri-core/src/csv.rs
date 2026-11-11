//! CSV / TSV：フィールドの分け方、区切り文字の判定、表示幅、列での並べ替え。
//!
//! 引用符（"..."）の中の区切り文字は区切りとみなさない。引用符の中の改行（複数行にまたがる
//! フィールド）は扱わず、1 行を 1 レコードとして扱う。

use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Path, PathBuf};

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

/// 1 行を読み終えた時点で、引用符の中にいるか（＝次の行が同じレコードの続きか）。
/// `start_quoted` は、この行の頭が前の行から続く引用符の中か。
/// 引用が始まるのはフィールドの先頭の `"` だけ（`split_fields` と同じ決まり）
pub fn ends_in_quote(line: &str, delim: char, start_quoted: bool) -> bool {
    let mut quoted = start_quoted;
    let mut field_start = !start_quoted;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    it.next();
                } else {
                    quoted = false;
                }
            }
        } else if c == '"' && field_start {
            quoted = true;
            field_start = false;
        } else {
            field_start = c == delim;
        }
    }
    quoted
}

/// 引用符の中に改行を含むレコードがあれば、その最初の続きの行（0 から数える）。
/// 行を単位に並べ替えたり絞り込んだりすると、そのレコードが壊れる
pub fn first_continuation_line(doc: &Document, delim: char, ctl: &SearchControl) -> Option<u64> {
    let (count, _) = doc.line_count();
    let mut quoted = false;
    for i in 0..count {
        if i % 65536 == 0 && ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        if quoted {
            return Some(i);
        }
        let (s, e) = doc.line_range(i)?;
        quoted = ends_in_quote(&doc.decode(&doc.read(s, e)), delim, false);
    }
    None
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

/// 外部ソートで 1 回にメモリへ載せる行数
pub const RUN_ROWS: usize = 2_000_000;
/// 外部ソートのキーの長さの上限（これより長いキーは先頭だけで比べる）
const KEY_BYTES_MAX: usize = 8 << 10;

fn write_key(w: &mut impl std::io::Write, k: &Key) -> std::io::Result<()> {
    match k {
        Key::Num(v) => {
            w.write_all(&[0])?;
            w.write_all(&v.to_le_bytes())
        }
        Key::Text(t) => {
            let mut end = t.len().min(KEY_BYTES_MAX);
            while !t.is_char_boundary(end) {
                end -= 1;
            }
            w.write_all(&[1])?;
            w.write_all(&(end as u32).to_le_bytes())?;
            w.write_all(&t.as_bytes()[..end])
        }
    }
}

/// 並べ替えた部分（run）を 1 件ずつ読む
struct RunReader {
    r: std::io::BufReader<std::fs::File>,
}

impl RunReader {
    fn next(&mut self) -> std::io::Result<Option<(Key, u64, u64)>> {
        use std::io::Read;
        let mut tag = [0u8; 1];
        if self.r.read(&mut tag)? == 0 {
            return Ok(None);
        }
        let key = if tag[0] == 0 {
            let mut b = [0u8; 8];
            self.r.read_exact(&mut b)?;
            Key::Num(f64::from_le_bytes(b))
        } else {
            let mut l = [0u8; 4];
            self.r.read_exact(&mut l)?;
            let mut t = vec![0u8; u32::from_le_bytes(l) as usize];
            self.r.read_exact(&mut t)?;
            Key::Text(String::from_utf8_lossy(&t).into_owned())
        };
        let mut b = [0u8; 16];
        self.r.read_exact(&mut b)?;
        Ok(Some((key, u64::from_le_bytes(b[..8].try_into().unwrap()), u64::from_le_bytes(b[8..].try_into().unwrap()))))
    }
}

/// マージの待ち行列の要素。`BinaryHeap` は最大が先に出るので、順序を逆にして最小を先に出す
struct HeapItem {
    key: Key,
    run: usize,
    start: u64,
    end: u64,
    descending: bool,
}

impl PartialEq for HeapItem {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for HeapItem {}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for HeapItem {
    fn cmp(&self, o: &Self) -> Ordering {
        let k = cmp_key(&self.key, &o.key);
        let k = if self.descending { k.reverse() } else { k };
        // 同じキーなら元の順（run の番号が小さい方）を先に（安定にするため）
        k.then(self.run.cmp(&o.run)).reverse()
    }
}

/// 行数が多くても並べ替えられる版：`out` に、並べ替えたファイルを書き出す（元は変えない）。
/// `run_rows` 行ずつ並べて一時ファイルに書き、最後にマージする。メモリは `run_rows` に比例し、行数には比例しない。
/// `done` に書いたバイト数（読む分と書く分を合わせて、全体の 2 倍が最大）を足す
pub fn sort_to_file(doc: &Document, opts: SortOptions, out: &Path, run_rows: usize, done: &std::sync::atomic::AtomicU64, ctl: &SearchControl) -> Result<(), String> {
    use std::io::{BufWriter, Write};
    use std::sync::atomic::Ordering as AO;
    let (count, exact) = doc.line_count();
    if !exact {
        return Err("索引の作成中は並べ替えできません".into());
    }
    if let Some(n) = first_continuation_line(doc, opts.delim, ctl) {
        return Err(format!("{} 行目が前の行から続くレコード（引用符の中の改行）なので、並べ替えるとレコードが壊れます。並べ替えは行いません", n + 1));
    }
    let last_empty = doc.line_range(count - 1).is_some_and(|(s, e)| s == e);
    let body_end = if last_empty { count - 1 } else { count };
    let first = if opts.header { 1.min(body_end) } else { 0 };
    let run_rows = run_rows.max(1);
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sorted".into());
    let run_path = |i: usize| out.with_file_name(format!(".{name}.kazakiri-sort-run{i}"));
    let mut runs: Vec<PathBuf> = Vec::new();
    let cleanup = |runs: &[PathBuf]| {
        for p in runs {
            let _ = std::fs::remove_file(p);
        }
    };
    let result = (|| -> Result<(), String> {
        // 1. 部分ごとに並べて、一時ファイルへ
        let mut i = first;
        while i < body_end {
            let j = (i + run_rows as u64).min(body_end);
            let mut rows: Vec<(Key, u64, u64)> = Vec::with_capacity((j - i) as usize);
            for n in i..j {
                if n % 65536 == 0 && ctl.cancel.load(AO::Relaxed) {
                    return Err("並べ替えを中止しました".into());
                }
                let (s, e) = doc.line_range(n).ok_or("行の位置が分かりません")?;
                let bytes = doc.read(s, e);
                let text = doc.decode(&bytes);
                let fields = split_fields(&text, opts.delim);
                let key = fields.get(opts.column).map_or(Key::Text(String::new()), |r| key_of(&text[r.clone()]));
                rows.push((key, s, e));
                done.fetch_add(e - s, AO::Relaxed);
            }
            rows.sort_by(|a, b| {
                let o = cmp_key(&a.0, &b.0);
                if opts.descending { o.reverse() } else { o }
            });
            let path = run_path(runs.len());
            let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).map_err(|e| e.to_string())?);
            runs.push(path);
            for (k, s, e) in &rows {
                write_key(&mut w, k).and_then(|_| w.write_all(&s.to_le_bytes())).and_then(|_| w.write_all(&e.to_le_bytes())).map_err(|e| e.to_string())?;
            }
            w.flush().map_err(|e| e.to_string())?;
            i = j;
        }
        // 2. マージして書き出す
        let nl = doc.encode(match doc.newline_name() {
            "CRLF" => "\r\n",
            "CR" => "\r",
            _ => "\n",
        });
        let temp = out.with_file_name(format!(".{name}.kazakiri-sort"));
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&temp).map_err(|e| e.to_string())?);
        let bom = doc.read(0, doc.text_start() as u64).into_owned();
        w.write_all(&bom).map_err(|e| e.to_string())?;
        let mut written = bom.len() as u64;
        if first == 1 {
            let (s, e) = doc.line_range(0).ok_or("見出し行がありません")?;
            w.write_all(&doc.read(s, e)).and_then(|_| w.write_all(&nl)).map_err(|e| e.to_string())?;
            written += (e - s) + nl.len() as u64;
        }
        let mut readers = Vec::new();
        for p in &runs {
            readers.push(RunReader { r: std::io::BufReader::with_capacity(1 << 18, std::fs::File::open(p).map_err(|e| e.to_string())?) });
        }
        let mut heap = std::collections::BinaryHeap::new();
        for (idx, r) in readers.iter_mut().enumerate() {
            if let Some((key, start, end)) = r.next().map_err(|e| e.to_string())? {
                heap.push(HeapItem { key, run: idx, start, end, descending: opts.descending });
            }
        }
        let mut n = 0u64;
        while let Some(item) = heap.pop() {
            n += 1;
            if n % 65536 == 0 && ctl.cancel.load(AO::Relaxed) {
                let _ = std::fs::remove_file(&temp);
                return Err("並べ替えを中止しました".into());
            }
            w.write_all(&doc.read(item.start, item.end)).and_then(|_| w.write_all(&nl)).map_err(|e| e.to_string())?;
            written += (item.end - item.start) + nl.len() as u64;
            done.fetch_add(item.end - item.start, AO::Relaxed);
            if let Some((key, start, end)) = readers[item.run].next().map_err(|e| e.to_string())? {
                heap.push(HeapItem { key, run: item.run, start, end, descending: opts.descending });
            }
        }
        w.flush().map_err(|e| e.to_string())?;
        let file = w.into_inner().map_err(|e| e.to_string())?;
        // 元が改行で終わっていなければ、最後の改行は付けない
        if !last_empty && written >= nl.len() as u64 && n + u64::from(first == 1) > 0 {
            file.set_len(written - nl.len() as u64).map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temp, out).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            e.to_string()
        })
    })();
    cleanup(&runs);
    result
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
    // 引用符の中に改行を含むレコードがあると、行ごとの並べ替えでレコードが壊れる
    if let Some(n) = first_continuation_line(doc, opts.delim, ctl) {
        return Err(format!(
            "{} 行目が前の行から続くレコード（引用符の中の改行）なので、並べ替えるとレコードが壊れます。並べ替えは行いません",
            n + 1
        ));
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
    fn external_sort_matches_in_memory_sort() {
        use std::sync::atomic::AtomicU64;
        let dir = std::env::temp_dir().join(format!("kazakiri-extsort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 乱数っぽい（重複のある）数値と文字列。最後は改行なし
        let mut text = String::from("id,name,score\r\n");
        let mut x: u64 = 12345;
        for i in 0..5000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            text.push_str(&format!("{i},user{},{}\r\n", (x >> 33) % 97, (x >> 40) % 500));
        }
        text.push_str("last,zzz,7");
        let d = ready(text.as_bytes());
        for (column, descending, header) in [(2usize, false, true), (2, true, true), (1, false, true), (1, true, false), (0, false, true)] {
            let o = SortOptions { column, delim: ',', descending, header };
            let want = sort_document(&d, o, &SearchControl::default()).unwrap();
            let out = dir.join("sorted.csv");
            sort_to_file(&d, o, &out, 137, &AtomicU64::new(0), &SearchControl::default()).unwrap();
            let got = std::fs::read(&out).unwrap();
            assert_eq!(String::from_utf8_lossy(&got), String::from_utf8_lossy(&want), "column {column} desc {descending} header {header}");
        }
        // 一時ファイルは残らない
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["sorted.csv"], "{left:?}");
        // 引用符の中の改行は断る
        let d = ready(b"a,b\n1,\"x\ny\"\n2,z\n");
        assert!(sort_to_file(&d, SortOptions { column: 0, delim: ',', descending: false, header: true }, &dir.join("q.csv"), 10, &AtomicU64::new(0), &SearchControl::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quote_state_across_lines() {
        assert!(!ends_in_quote("a,b,c", ',', false));
        assert!(ends_in_quote("a,\"multi", ',', false));
        assert!(!ends_in_quote("end\",x", ',', true));
        assert!(!ends_in_quote("a,\"x\"\"y\",b", ',', false));
        // フィールドの途中の " は引用の開始ではない（5" のような値）
        assert!(!ends_in_quote("a,5\" screen,c", ',', false));
        // 続きの行がさらに続く
        assert!(ends_in_quote("still inside", ',', true));
        // 引用の中の "" は " そのもの。閉じていなければ次の行に続く
        assert!(!ends_in_quote("\"\"\"quoted\"\"\"", ',', false));
        assert!(ends_in_quote("\"\"\"quoted\"\" start", ',', false));
    }

    #[test]
    fn sort_refuses_multiline_records() {
        let d = ready(b"id,note\r\n2,\"line one\r\nline two\"\r\n1,plain\r\n");
        assert_eq!(first_continuation_line(&d, ',', &SearchControl::default()), Some(2));
        let o = SortOptions { column: 0, delim: ',', descending: false, header: true };
        let e = sort_document(&d, o, &SearchControl::default()).unwrap_err();
        assert!(e.contains("3 行目"), "{e}");
        // 改行を含まないファイルはこれまでどおり
        let d = ready(b"id,note\r\n2,\"a, b\"\r\n1,plain\r\n");
        assert_eq!(first_continuation_line(&d, ',', &SearchControl::default()), None);
        assert!(sort_document(&d, o, &SearchControl::default()).is_ok());
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
