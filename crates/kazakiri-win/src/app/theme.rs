//! 配色（明るいテーマ・暗いテーマ）。

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

use super::rgb;

pub(super) struct Theme {
    pub bg: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub gutter_bg: D2D1_COLOR_F,
    pub gutter_text: D2D1_COLOR_F,
    pub status_bg: D2D1_COLOR_F,
    pub status_text: D2D1_COLOR_F,
    pub matched: D2D1_COLOR_F,
    pub select: D2D1_COLOR_F,
    pub dim: D2D1_COLOR_F,
    pub caret: D2D1_COLOR_F,
    pub column: D2D1_COLOR_F,
    pub panel_bg: D2D1_COLOR_F,
    pub panel_head: D2D1_COLOR_F,
    pub panel_sel: D2D1_COLOR_F,
    pub panel_file: D2D1_COLOR_F,
    /// 空白・タブ・改行の印
    pub marks: D2D1_COLOR_F,
    pub log_error: D2D1_COLOR_F,
    pub log_warn: D2D1_COLOR_F,
    pub log_debug: D2D1_COLOR_F,
    /// 構文の色分け
    pub syn_str: D2D1_COLOR_F,
    pub syn_num: D2D1_COLOR_F,
    pub syn_kw: D2D1_COLOR_F,
    pub syn_comment: D2D1_COLOR_F,
    pub syn_tag: D2D1_COLOR_F,
    pub syn_key: D2D1_COLOR_F,
}

pub(super) static LIGHT: Theme = Theme {
    bg: rgb(0xFF, 0xFF, 0xFF),
    text: rgb(0x1E, 0x1E, 0x1E),
    gutter_bg: rgb(0xF3, 0xF3, 0xF3),
    gutter_text: rgb(0x8A, 0x8A, 0x8A),
    status_bg: rgb(0xEE, 0xEE, 0xF0),
    status_text: rgb(0x40, 0x40, 0x46),
    matched: rgb(0xFF, 0xE0, 0x6B),
    select: rgb(0xAD, 0xD6, 0xFF),
    dim: rgb(0xB0, 0xB0, 0xB0),
    caret: rgb(0x00, 0x00, 0x00),
    column: rgb(0xEE, 0xF6, 0xEA),
    panel_bg: rgb(0xF7, 0xF7, 0xF7),
    panel_head: rgb(0xE4, 0xE8, 0xEE),
    panel_sel: rgb(0xCC, 0xE4, 0xFF),
    panel_file: rgb(0x1F, 0x5F, 0xA8),
    marks: rgb(0xB8, 0xC4, 0xD0),
    log_error: rgb(0xD0, 0x1F, 0x1F),
    log_warn: rgb(0xC0, 0x6A, 0x00),
    log_debug: rgb(0x90, 0x90, 0x90),
    syn_str: rgb(0x0A, 0x7A, 0x32),
    syn_num: rgb(0x00, 0x6F, 0xB4),
    syn_kw: rgb(0x9B, 0x2D, 0xC4),
    syn_comment: rgb(0x78, 0x82, 0x8C),
    syn_tag: rgb(0xB0, 0x30, 0x40),
    syn_key: rgb(0x1F, 0x5F, 0xA8),
};

pub(super) static DARK: Theme = Theme {
    bg: rgb(0x1E, 0x1F, 0x22),
    text: rgb(0xD8, 0xDA, 0xDE),
    gutter_bg: rgb(0x26, 0x28, 0x2C),
    gutter_text: rgb(0x6E, 0x74, 0x7C),
    status_bg: rgb(0x2C, 0x2E, 0x33),
    status_text: rgb(0xB8, 0xBC, 0xC4),
    matched: rgb(0x7A, 0x63, 0x10),
    select: rgb(0x26, 0x4F, 0x78),
    dim: rgb(0x70, 0x74, 0x7A),
    caret: rgb(0xF0, 0xF0, 0xF0),
    column: rgb(0x26, 0x33, 0x2A),
    panel_bg: rgb(0x23, 0x25, 0x29),
    panel_head: rgb(0x30, 0x34, 0x3A),
    panel_sel: rgb(0x2A, 0x45, 0x66),
    panel_file: rgb(0x6C, 0xB0, 0xF0),
    marks: rgb(0x4A, 0x52, 0x5C),
    log_error: rgb(0xFF, 0x6B, 0x6B),
    log_warn: rgb(0xF0, 0xB0, 0x50),
    log_debug: rgb(0x80, 0x86, 0x8E),
    syn_str: rgb(0x98, 0xC3, 0x79),
    syn_num: rgb(0xD1, 0x9A, 0x66),
    syn_kw: rgb(0xC6, 0x78, 0xDD),
    syn_comment: rgb(0x7F, 0x84, 0x8E),
    syn_tag: rgb(0xE0, 0x6C, 0x75),
    syn_key: rgb(0x61, 0xAF, 0xEF),
};

/// Windows のアクセントカラー（0xAARRGGBB。0 ならまだ取っていない）
static SYS_ACCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// システムのアクセントカラーを読む（取れなければ Windows 11 の既定の青）
pub(super) fn load_accent() {
    let mut c = 0u32;
    let mut opaque = windows::core::BOOL(0);
    let ok = unsafe { windows::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut c, &mut opaque) }.is_ok();
    SYS_ACCENT.store(if ok { c | 0xFF00_0000 } else { 0 }, std::sync::atomic::Ordering::Relaxed);
}

/// 選択中のタブの印などに使う色。ダークテーマでは見やすいよう明るくする
pub(super) fn accent(dark: bool) -> D2D1_COLOR_F {
    let v = SYS_ACCENT.load(std::sync::atomic::Ordering::Relaxed);
    let (r, g, b) = if v == 0 { (0x00, 0x67, 0xC0) } else { (((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8) };
    if dark {
        let up = |c: u8| (c as u32 + (255 - c as u32) * 40 / 100) as u8;
        rgb(up(r), up(g), up(b))
    } else {
        rgb(r, g, b)
    }
}
