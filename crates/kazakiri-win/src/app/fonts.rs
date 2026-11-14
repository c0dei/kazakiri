//! フォントの作成と文字の寸法の計測。

use super::*;

pub(super) const FONT_CANDIDATES: [&str; 4] = ["BIZ UDGothic", "BIZ UDゴシック", "MS Gothic", "Consolas"];
pub(super) const UI_FONT_CANDIDATES: [&str; 4] = ["Segoe UI Variable Text", "Yu Gothic UI", "Meiryo UI", "Segoe UI"];

pub(super) unsafe fn create_format(dw: &IDWriteFactory, font: &str, size: f32) -> Result<IDWriteTextFormat> {
    unsafe { create_format_from(dw, font, size, &FONT_CANDIDATES) }
}

pub(super) unsafe fn create_ui_format(dw: &IDWriteFactory, font: &str, size: f32) -> Result<IDWriteTextFormat> {
    unsafe { create_format_from(dw, font, size, &UI_FONT_CANDIDATES) }
}

pub(super) unsafe fn create_format_from(dw: &IDWriteFactory, font: &str, size: f32, defaults: &[&str]) -> Result<IDWriteTextFormat> {
    unsafe {
        let mut fonts = None;
        dw.GetSystemFontCollection(&mut fonts, false)?;
        let fonts = fonts.unwrap();
        // 環境変数 KAZAKIRI_FONT でフォントを指定できる
        // 次に設定ファイルの font
        let user_font = std::env::var("KAZAKIRI_FONT").ok();
        let setting = (!font.is_empty()).then_some(font);
        let candidates = user_font.iter().map(String::as_str).chain(setting).chain(defaults.iter().copied());
        let mut family = defaults[defaults.len() - 1];
        for name in candidates {
            let (mut idx, mut exists) = (0u32, BOOL(0));
            let wname = wide(name);
            fonts.FindFamilyName(PCWSTR(wname.as_ptr()), &mut idx, &mut exists)?;
            if exists.as_bool() {
                family = name;
                break;
            }
        }
        let wfamily = wide(family);
        let format = dw.CreateTextFormat(
            PCWSTR(wfamily.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("ja-jp"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// 等幅でないフォントか（細い文字と太い文字の幅が違う）
pub(super) unsafe fn is_proportional(dw: &IDWriteFactory, format: &IDWriteTextFormat) -> bool {
    unsafe {
        let width = |s: &str| -> f32 {
            let u: Vec<u16> = s.encode_utf16().collect();
            let Ok(layout) = dw.CreateTextLayout(&u, format, 10000.0, 1000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut m);
            m.width
        };
        let (i, w) = (width("iiiiiiiiii"), width("WWWWWWWWWW"));
        i > 0.0 && (w - i).abs() > i * 0.1
    }
}

/// 半角 1 文字の幅と行の高さ（DIP）
pub(super) unsafe fn measure(dw: &IDWriteFactory, format: &IDWriteTextFormat) -> Result<(f32, f32)> {
    unsafe {
        let s: Vec<u16> = "0000000000".encode_utf16().collect();
        let layout = dw.CreateTextLayout(&s, format, 10000.0, 1000.0)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut m)?;
        Ok((m.width / 10.0, (m.height * 1.15).ceil()))
    }
}
