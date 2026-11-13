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
