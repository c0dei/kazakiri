//! 文字列の変換と、行の操作（エディタの「変換」「編集 → 行の操作」の中身）。
//!
//! どれも「文字列を受け取って新しい文字列を返す」だけの関数。位置や Undo は呼び出し側（エディタ）が扱う。

/// 文字の変換の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conv {
    Upper,
    Lower,
    /// 全角の英数字・記号・空白 → 半角
    ZenToHanAscii,
    /// 半角の英数字・記号・空白 → 全角
    HanToZenAscii,
    /// 全角カタカナ → 半角カタカナ（濁点は 2 文字になる）
    ZenKanaToHan,
    /// 半角カタカナ → 全角カタカナ（濁点・半濁点は合成する）
    HanKanaToZen,
    HiraToKata,
    KataToHira,
    /// タブ → 空白（幅は桁に合わせる）
    TabToSpace(usize),
    /// 行頭の空白 → タブ
    SpaceToTab(usize),
    /// JSON を整形（展開）する。インデントは空白の数
    JsonPretty(usize),
    /// JSON を 1 行に圧縮する
    JsonMinify,
    /// 各行の先頭の空白・タブを取る
    TrimLeading,
    /// 各単語の頭を大文字にする
    Capitalize,
    /// `& < > " '` を HTML の文字参照にする
    HtmlEscape,
    /// HTML の文字参照（`&amp;` `&#65;` `&#x41;` など）を文字に戻す
    HtmlUnescape,
    /// UTF-8 のバイト列を Base64 にする
    Base64Encode,
    Base64Decode,
    /// UTF-8 のバイト列をパーセントエンコードする（URL 用）
    UrlEncode,
    UrlDecode,
}

const HAN: &str = "｡｢｣､･ｦｧｨｩｪｫｬｭｮｯｰｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ";
const ZEN: &str = "。「」、・ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";
const DAKU_BASE: &str = "カキクケコサシスセソタチツテトハヒフヘホ";
const HANDAKU_BASE: &str = "ハヒフヘホ";

/// 表示上の幅（半角 1、全角 2 とみなす）
fn width(c: char) -> usize {
    let u = c as u32;
    if u < 0x80 || (0xFF61..=0xFF9F).contains(&u) {
        1
    } else {
        2
    }
}

pub fn convert(s: &str, conv: Conv) -> String {
    match conv {
        Conv::Upper => s.to_uppercase(),
        Conv::Lower => s.to_lowercase(),
        Conv::ZenToHanAscii => s
            .chars()
            .map(|c| match c as u32 {
                0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
                0x3000 => ' ',
                _ => c,
            })
            .collect(),
        Conv::HanToZenAscii => s
            .chars()
            .map(|c| match c as u32 {
                0x21..=0x7E => char::from_u32(c as u32 + 0xFEE0).unwrap_or(c),
                0x20 => '\u{3000}',
                _ => c,
            })
            .collect(),
        Conv::HiraToKata => s.chars().map(|c| if ('\u{3041}'..='\u{3096}').contains(&c) { char::from_u32(c as u32 + 0x60).unwrap_or(c) } else { c }).collect(),
        Conv::KataToHira => s.chars().map(|c| if ('\u{30A1}'..='\u{30F6}').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c }).collect(),
        Conv::HanKanaToZen => han_kana_to_zen(s),
        Conv::ZenKanaToHan => zen_kana_to_han(s),
        Conv::TabToSpace(w) => map_lines_raw(s, |line| tab_to_space(line, w.max(1))),
        Conv::SpaceToTab(w) => map_lines_raw(s, |line| space_to_tab(line, w.max(1))),
        Conv::JsonPretty(n) => json_pretty(s, n),
        Conv::JsonMinify => json_minify(s),
        Conv::TrimLeading => map_lines_raw(s, |l| l.trim_start_matches([' ', '\t', '\u{3000}']).to_string()),
        Conv::Capitalize => capitalize(s),
        Conv::HtmlEscape => html_escape(s),
        Conv::HtmlUnescape => html_unescape(s),
        Conv::Base64Encode => base64_encode(s.as_bytes()),
        Conv::Base64Decode => base64_decode(s).unwrap_or_else(|| s.to_string()),
        Conv::UrlEncode => url_encode(s),
        Conv::UrlDecode => url_decode(s),
    }
}

fn han_kana_to_zen(s: &str) -> String {
    let han: Vec<char> = HAN.chars().collect();
    let zen: Vec<char> = ZEN.chars().collect();
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match han.iter().position(|&h| h == c) {
            Some(k) => {
                let z = zen[k];
                let next = chars.get(i + 1).copied();
                if next == Some('ﾞ') && DAKU_BASE.contains(z) {
                    out.push(char::from_u32(z as u32 + 1).unwrap_or(z));
                    i += 2;
                    continue;
                }
                if next == Some('ﾞ') && z == 'ウ' {
                    out.push('ヴ');
                    i += 2;
                    continue;
                }
                if next == Some('ﾟ') && HANDAKU_BASE.contains(z) {
                    out.push(char::from_u32(z as u32 + 2).unwrap_or(z));
                    i += 2;
                    continue;
                }
                out.push(z);
            }
            None => out.push(match c {
                'ﾞ' => '゛',
                'ﾟ' => '゜',
                c => c,
            }),
        }
        i += 1;
    }
    out
}

fn zen_kana_to_han(s: &str) -> String {
    let han: Vec<char> = HAN.chars().collect();
    let zen: Vec<char> = ZEN.chars().collect();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(k) = zen.iter().position(|&z| z == c) {
            out.push(han[k]);
        } else if c == 'ヴ' {
            out.push_str("ｳﾞ");
        } else if let Some(base) = char::from_u32(c as u32 - 1).filter(|b| DAKU_BASE.contains(*b) && "ガギグゲゴザジズゼゾダヂヅデドバビブベボ".contains(c)) {
            let k = zen.iter().position(|&z| z == base).unwrap_or(0);
            out.push(han[k]);
            out.push('ﾞ');
        } else if let Some(base) = char::from_u32(c as u32 - 2).filter(|_| "パピプペポ".contains(c)) {
            let k = zen.iter().position(|&z| z == base).unwrap_or(0);
            out.push(han[k]);
            out.push('ﾟ');
        } else if c == '゛' {
            out.push('ﾞ');
        } else if c == '゜' {
            out.push('ﾟ');
        } else {
            out.push(c);
        }
    }
    out
}

/// 行ごとに変換する（改行はそのまま通す）
fn map_lines_raw(s: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        out.push_str(&f(&rest[..end]));
        let nl_len = rest[end..].chars().take_while(|c| *c == '\r' || *c == '\n').take(if rest[end..].starts_with("\r\n") { 2 } else { 1 }).count();
        out.push_str(&rest[end..end + nl_len]);
        rest = &rest[end + nl_len..];
    }
    out
}

fn tab_to_space(line: &str, w: usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = w - col % w;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += width(c);
        }
    }
    out
}

fn space_to_tab(line: &str, w: usize) -> String {
    // 行頭の空白とタブを幅に直して、タブと空白に組み直す
    let mut col = 0;
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        match c {
            ' ' => col += 1,
            '\t' => col += w - col % w,
            _ => break,
        }
        rest = &rest[c.len_utf8()..];
    }
    format!("{}{}{}", "\t".repeat(col / w), " ".repeat(col % w), rest)
}

fn capitalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphabetic() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = !(c.is_alphanumeric() || c == '\'' || c == '_');
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Base64（改行・空白は無視）を UTF-8 の文字列に戻す。できなければ None
pub fn base64_decode(s: &str) -> Option<String> {
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        bits = bits << 6 | v as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
            bits &= (1 << n) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// JSON を整形する（字句だけを見るので、壊れた JSON でもそのまま整える）。改行は `\n`
pub fn json_pretty(s: &str, indent: usize) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    let chars: Vec<char> = s.chars().collect();
    let pad = |out: &mut String, d: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', d * indent));
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' | '[' => {
                // 空の {} [] はそのまま
                let close = if c == '{' { '}' } else { ']' };
                let next = chars[i + 1..].iter().position(|x| !x.is_whitespace()).map(|k| chars[i + 1 + k]);
                if next == Some(close) {
                    out.push(c);
                    out.push(close);
                    i += 1 + chars[i + 1..].iter().position(|x| *x == close).unwrap_or(0) + 1;
                    continue;
                }
                out.push(c);
                depth += 1;
                pad(&mut out, depth);
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                pad(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                pad(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// JSON を 1 行に圧縮する（文字列の中の空白は残す）
pub fn json_minify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut esc) = (false, false);
    for c in s.chars() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 単語補完の候補：`window` の中の単語のうち `prefix` で始まるものを、カーソル（`caret`＝文字位置）に近い順に。
/// カーソルの直前の単語そのもの（補完中の語）は含めない
pub fn complete_candidates(window: &str, caret: usize, prefix: &str) -> Vec<String> {
    let chars: Vec<char> = window.chars().collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_word(chars[i]) {
            let start = i;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let is_current = start + prefix.chars().count() == caret && i == caret;
            if word.len() > prefix.len() && word.starts_with(prefix) && !is_current {
                let dist = start.abs_diff(caret);
                found.push((dist, word));
            }
        } else {
            i += 1;
        }
    }
    found.sort_by_key(|(d, _)| *d);
    let mut seen = std::collections::HashSet::new();
    found.into_iter().filter_map(|(_, w)| seen.insert(w.clone()).then_some(w)).collect()
}

// ---- 行の操作 ----

/// 行に分ける。(行の一覧, 最後が改行で終わっているか)
pub fn split_lines<'a>(text: &'a str, eol: &str) -> (Vec<&'a str>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let ends = text.ends_with(eol);
    let body = if ends { &text[..text.len() - eol.len()] } else { text };
    (body.split(eol).collect(), ends)
}

pub fn join_lines<S: AsRef<str>>(lines: &[S], eol: &str, ends: bool) -> String {
    let mut s = lines.iter().map(|l| l.as_ref()).collect::<Vec<_>>().join(eol);
    if ends && !lines.is_empty() {
        s.push_str(eol);
    }
    s
}

/// 行を二重にする（各行をその直後に複製）。選択した行のかたまりをそのまま繰り返す
pub fn duplicate(text: &str, eol: &str) -> String {
    let (lines, ends) = split_lines(text, eol);
    let once = join_lines(&lines, eol, true);
    let mut out = once.clone();
    out.push_str(&join_lines(&lines, eol, ends));
    out
}

/// 各行の先頭に `unit`（タブか空白）を足す（空行は除く）
pub fn indent(text: &str, eol: &str, unit: &str) -> String {
    let (lines, ends) = split_lines(text, eol);
    let v: Vec<String> = lines.iter().map(|l| if l.is_empty() { String::new() } else { format!("{unit}{l}") }).collect();
    join_lines(&v, eol, ends)
}

/// 各行の先頭から、タブ 1 つか、空白（最大 `tab` 個）を 1 段分取る
pub fn unindent(text: &str, eol: &str, tab: usize) -> String {
    let (lines, ends) = split_lines(text, eol);
    let v: Vec<String> = lines
        .iter()
        .map(|l| {
            if let Some(r) = l.strip_prefix('\t') {
                r.to_string()
            } else if let Some(r) = l.strip_prefix('\u{3000}') {
                r.to_string()
            } else {
                let n = l.chars().take(tab.max(1)).take_while(|c| *c == ' ').count();
                l[n..].to_string()
            }
        })
        .collect();
    join_lines(&v, eol, ends)
}

pub fn trim_trailing(text: &str, eol: &str) -> String {
    let (lines, ends) = split_lines(text, eol);
    let v: Vec<&str> = lines.iter().map(|l| l.trim_end_matches([' ', '\t', '\u{3000}'])).collect();
    join_lines(&v, eol, ends)
}

/// 重複した行を取り除く（最初の 1 行を残す）
pub fn dedupe(text: &str, eol: &str) -> String {
    let (lines, ends) = split_lines(text, eol);
    let mut seen = std::collections::HashSet::new();
    let v: Vec<&str> = lines.into_iter().filter(|l| seen.insert(*l)).collect();
    join_lines(&v, eol, ends)
}

pub fn sort(text: &str, eol: &str, desc: bool) -> String {
    let (mut lines, ends) = split_lines(text, eol);
    lines.sort();
    if desc {
        lines.reverse();
    }
    join_lines(&lines, eol, ends)
}

/// 行の並べ替えの基準
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// 先頭の数値（数値でない行は最後）
    Number,
    /// 文字数
    Length,
    /// 同じ行の出現回数（同じ回数の中は、もとの順）
    Frequency,
    /// 行の中の最初の IPv4 アドレス（なければ最後）
    Ip,
    /// 行の順序を逆にする
    Reverse,
    /// ランダムに並べ替える
    Random,
}

fn leading_number(l: &str) -> Option<f64> {
    let t = l.trim_start();
    let end = t.char_indices().take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))).last().map_or(0, |(i, c)| i + c.len_utf8());
    t[..end].parse::<f64>().ok()
}

fn first_ipv4(l: &str) -> Option<u32> {
    let b = l.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            let parts: Vec<&str> = l[i..j].split('.').collect();
            if parts.len() == 4 {
                let n: Option<Vec<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
                if let Some(n) = n {
                    return Some(u32::from_be_bytes([n[0], n[1], n[2], n[3]]));
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

/// 行を `key` の基準で並べ替える。`desc` は降順（`Frequency` は多い順）
pub fn sort_by(text: &str, eol: &str, key: SortKey, desc: bool, seed: u64) -> String {
    let (mut lines, ends) = split_lines(text, eol);
    match key {
        SortKey::Number => {
            lines.sort_by(|a, b| {
                let (x, y) = (leading_number(a), leading_number(b));
                match (x, y) {
                    (Some(x), Some(y)) => {
                        let o = x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
                        if desc { o.reverse() } else { o }
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
        }
        SortKey::Length => lines.sort_by_key(|l| {
            let n = l.chars().count();
            if desc { usize::MAX - n } else { n }
        }),
        SortKey::Frequency => {
            let mut count: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for l in &lines {
                *count.entry(*l).or_default() += 1;
            }
            lines.sort_by_key(|l| {
                let n = count[l];
                if desc { usize::MAX - n } else { n }
            });
        }
        SortKey::Ip => lines.sort_by(|a, b| match (first_ipv4(a), first_ipv4(b)) {
            (Some(x), Some(y)) => {
                if desc { y.cmp(&x) } else { x.cmp(&y) }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }),
        SortKey::Reverse => lines.reverse(),
        SortKey::Random => {
            let mut x = seed | 1;
            for i in (1..lines.len()).rev() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                lines.swap(i, (x % (i as u64 + 1)) as usize);
            }
        }
    }
    join_lines(&lines, eol, ends)
}

/// 先頭の `n` 行を、残りの行の後ろへ回す（`up` のとき。選択した行を上へ動かす＝直前の 1 行を後ろへ）。
/// `up`: 先頭の 1 行（となり）を末尾へ。`!up`: 末尾の 1 行（となり）を先頭へ
pub fn rotate_neighbor(text: &str, eol: &str, up: bool) -> String {
    let (mut lines, ends) = split_lines(text, eol);
    if lines.len() >= 2 {
        if up {
            lines.rotate_left(1);
        } else {
            lines.rotate_right(1);
        }
    }
    join_lines(&lines, eol, ends)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_tables_have_same_length() {
        assert_eq!(HAN.chars().count(), ZEN.chars().count());
        assert_eq!(HAN.chars().count(), 61);
    }

    #[test]
    fn width_conversions() {
        assert_eq!(convert("ＡＢＣ１２３！　x", Conv::ZenToHanAscii), "ABC123! x");
        assert_eq!(convert("ABC 12", Conv::HanToZenAscii), "ＡＢＣ\u{3000}１２");
        assert_eq!(convert("ｶﾞｷﾞﾊﾟｳﾞｱ｡ｰ", Conv::HanKanaToZen), "ガギパヴア。ー");
        assert_eq!(convert("ガギパヴア。ー", Conv::ZenKanaToHan), "ｶﾞｷﾞﾊﾟｳﾞｱ｡ｰ");
        assert_eq!(convert("ひらがなゔ", Conv::HiraToKata), "ヒラガナヴ");
        assert_eq!(convert("カタカナヴ", Conv::KataToHira), "かたかなゔ");
        assert_eq!(convert("abC", Conv::Upper), "ABC");
        assert_eq!(convert("abC", Conv::Lower), "abc");
    }

    #[test]
    fn tabs_and_spaces() {
        assert_eq!(convert("a\tb\r\n\tc\n", Conv::TabToSpace(4)), "a   b\r\n    c\n");
        assert_eq!(convert("あ\tx", Conv::TabToSpace(4)), "あ  x");
        assert_eq!(convert("        x\n  y\n    \n", Conv::SpaceToTab(4)), "\t\tx\n  y\n\t\n");
    }

    #[test]
    fn json_and_completion() {
        let src = "{\"a\":1,\"b\":[1,2,{}],\"c\":\"x, y: {z}\"}";
        let pretty = json_pretty(src, 2);
        assert_eq!(pretty, "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2,\n    {}\n  ],\n  \"c\": \"x, y: {z}\"\n}");
        assert_eq!(json_minify(&pretty), src);
        let text = "foo foobar fooz bar foo";
        // 末尾の foo を補完：近い順
        assert_eq!(complete_candidates(text, 23, "foo"), vec!["fooz".to_string(), "foobar".to_string()]);
    }

    #[test]
    fn sorts_and_encodings() {
        assert_eq!(sort_by("10 b\n2 a\nx\n-1 c\n", "\n", SortKey::Number, false, 1), "-1 c\n2 a\n10 b\nx\n");
        assert_eq!(sort_by("10 b\n2 a\nx\n", "\n", SortKey::Number, true, 1), "10 b\n2 a\nx\n");
        assert_eq!(sort_by("ccc\na\nbb\n", "\n", SortKey::Length, false, 1), "a\nbb\nccc\n");
        assert_eq!(sort_by("ccc\na\nbb\n", "\n", SortKey::Length, true, 1), "ccc\nbb\na\n");
        assert_eq!(sort_by("a\nb\na\nc\nb\na\n", "\n", SortKey::Frequency, true, 1), "a\na\na\nb\nb\nc\n");
        assert_eq!(sort_by("host 10.0.0.10\nhost 10.0.0.2\nnone\n9.1.1.1 x\n", "\n", SortKey::Ip, false, 1), "9.1.1.1 x\nhost 10.0.0.2\nhost 10.0.0.10\nnone\n");
        assert_eq!(sort_by("1\n2\n3\n", "\n", SortKey::Reverse, false, 1), "3\n2\n1\n");
        let r = sort_by("1\n2\n3\n4\n5\n", "\n", SortKey::Random, false, 12345);
        let mut v: Vec<&str> = r.lines().collect();
        v.sort();
        assert_eq!(v, vec!["1", "2", "3", "4", "5"]);
        assert_eq!(convert("hello wORLD it's", Conv::Capitalize), "Hello World It's");
        assert_eq!(convert("  a\n\tb c  \n", Conv::TrimLeading), "a\nb c  \n");
        assert_eq!(convert("<a href=\"x\">&</a>", Conv::HtmlEscape), "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;");
        assert_eq!(convert("&lt;b&gt; &amp; &#65;&#x42; &unknown; &", Conv::HtmlUnescape), "<b> & AB &unknown; &");
        assert_eq!(base64_encode("Man".as_bytes()), "TWFu");
        assert_eq!(base64_encode("Ma".as_bytes()), "TWE=");
        assert_eq!(convert("日本語 abc", Conv::Base64Decode), "日本語 abc"); // Base64 でなければそのまま
        assert_eq!(convert(&convert("日本語 abc", Conv::Base64Encode), Conv::Base64Decode), "日本語 abc");
        assert_eq!(convert("a b/日", Conv::UrlEncode), "a%20b%2F%E6%97%A5");
        assert_eq!(convert("a%20b%2F%E6%97%A5+x", Conv::UrlDecode), "a b/日 x");
    }

    #[test]
    fn line_ops() {
        assert_eq!(duplicate("a\nb\n", "\n"), "a\nb\na\nb\n");
        assert_eq!(duplicate("a\nb", "\n"), "a\nb\na\nb");
        assert_eq!(indent("a\n\nb\n", "\n", "\t"), "\ta\n\n\tb\n");
        assert_eq!(unindent("\ta\n    b\n  c\nd", "\n", 4), "a\nb\nc\nd");
        assert_eq!(trim_trailing("a  \r\nb\t\r\n", "\r\n"), "a\r\nb\r\n");
        assert_eq!(dedupe("a\nb\na\nc\nb\n", "\n"), "a\nb\nc\n");
        assert_eq!(sort("b\na\nc", "\n", false), "a\nb\nc");
        assert_eq!(sort("b\na\nc\n", "\n", true), "c\nb\na\n");
        assert_eq!(rotate_neighbor("n\ns1\ns2\n", "\n", true), "s1\ns2\nn\n");
        assert_eq!(rotate_neighbor("s1\ns2\nn", "\n", false), "n\ns1\ns2");
    }
}
