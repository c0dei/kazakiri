//! 小さな共通関数（桁区切り・大きさの表示・四角形・UTF-16 の長さなど）。

use super::*;

/// 描画する UTF-16 列と、そのうち本文の長さ（省略記号を除く）
pub(super) fn render_units(line: &Line) -> (Vec<u16>, usize) {
    let mut units: Vec<u16> = line.text.encode_utf16().take(MAX_RENDER_UNITS).collect();
    let shown = units.len();
    if line.truncated || shown == MAX_RENDER_UNITS {
        units.extend("  …(省略)".encode_utf16());
    }
    (units, shown)
}

/// 行末の改行コードのバイト数
pub(super) fn newline_len(doc: &Document, line: &Line) -> u64 {
    doc.line_range(line.number + 1).map_or(0, |(next, _)| next - line.end)
}

/// バイト列をデコードしたときの UTF-16 単位数
pub(super) fn decode_units(doc: &Document, bytes: &[u8]) -> usize {
    doc.decode(bytes).encode_utf16().count()
}

pub(super) fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        OpenClipboard(None).ok()?;
        let result = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let mem = HGLOBAL(h.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut n = 0;
            while *ptr.add(n) != 0 && n < 1 << 20 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n));
            let _ = GlobalUnlock(mem);
            Some(s)
        })();
        let _ = CloseClipboard();
        result
    }
}

pub(super) fn on_off(b: bool) -> &'static str {
    if b { "オン" } else { "オフ" }
}

pub(super) fn rrect(l: f32, t: f32, r: f32, b: f32, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(l, t, r, b), radiusX: radius, radiusY: radius }
}

pub(super) fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

pub(super) fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}

/// 3 桁区切り
pub(super) fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(super) fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.2} {}", U[i]) }
}

#[cfg(test)]
mod multi_select_tests {
    use super::parse_multi_select;
    use std::path::PathBuf;

    fn w(parts: &[&str]) -> Vec<u16> {
        let mut v = Vec::new();
        for p in parts {
            v.extend(p.encode_utf16());
            v.push(0);
        }
        v.push(0);
        v.resize(v.len() + 8, 0);
        v
    }

    #[test]
    fn single_and_multiple() {
        assert_eq!(parse_multi_select(&w(&["C:\\a\\x.log"])), vec![PathBuf::from("C:\\a\\x.log")]);
        assert_eq!(
            parse_multi_select(&w(&["C:\\a", "x.log", "日本語.csv"])),
            vec![PathBuf::from("C:\\a").join("x.log"), PathBuf::from("C:\\a").join("日本語.csv")]
        );
        assert!(parse_multi_select(&w(&[])).is_empty());
    }
}
