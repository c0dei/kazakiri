//! 表示の設定：文字の大きさ、テーマ、空白の可視化、ログの色分け。変更は設定ファイルに保存する。

use windows::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::MARGINS;

use super::*;

impl App {
    /// フォントを作り直す（大きさの変更など）
    pub(super) fn apply_font(&mut self) {
        let s = &self.settings;
        let Ok(format) = (unsafe { create_format(&self.dw, &s.font, s.font_size) }) else {
            self.message = "フォントを作れませんでした".into();
            return self.invalidate();
        };
        let (char_w, line_h) = unsafe { measure(&self.dw, &format) }.unwrap_or((s.font_size * 0.5, (s.font_size * 1.4).ceil()));
        unsafe {
            let _ = format.SetIncrementalTabStop(char_w * s.tab_width as f32);
        }
        self.format = format;
        self.char_w = char_w;
        self.line_h = line_h;
        self.layout_cache.borrow_mut().clear();
        self.update_scrollbars();
        self.update_ime_position();
        self.invalidate();
    }

    /// 文字の大きさを変える（`delta` は pt。0 なら標準に戻す）
    pub(super) fn zoom(&mut self, delta: f32) {
        let size = if delta == 0.0 {
            Settings::default().font_size
        } else {
            (self.settings.font_size + delta).clamp(settings::MIN_FONT, settings::MAX_FONT)
        };
        if size == self.settings.font_size {
            return;
        }
        self.settings.font_size = size;
        self.apply_font();
        self.settings.save();
        self.message = format!("文字の大きさ: {size} pt");
    }

    pub(super) fn set_dark(&mut self, dark: bool) {
        self.settings.dark = dark;
        self.theme = if dark { &theme::DARK } else { &theme::LIGHT };
        // ブラシを作り直す
        self.gfx = None;
        self.apply_title_theme();
        self.sync_menu_bar();
        self.settings.save();
        self.message = format!("ダークテーマ: {}", on_off(dark));
        self.invalidate();
    }

    /// タイトルバーをテーマに合わせる（Windows 10 20H1 以降）
    pub(super) fn apply_title_theme(&self) {
        let v = BOOL::from(self.settings.dark);
        unsafe {
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, size_of::<BOOL>() as u32);
            super::dark::apply(self.hwnd, self.settings.dark);
            // Windows 11：タイトルバーを Mica に（DWMWA_SYSTEMBACKDROP_TYPE: 1=なし, 2=Mica）。
            // Windows 10 などでは属性が無く、失敗しても何も起きない
            let backdrop: i32 = if self.settings.mica { 2 } else { 1 };
            let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(self.hwnd, &m);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const i32 as *const _, size_of::<i32>() as u32);
        }
    }

    pub(super) fn toggle_whitespace(&mut self) {
        self.settings.whitespace = !self.settings.whitespace;
        self.settings.save();
        self.message = format!("空白・タブ・改行の表示: {}", on_off(self.settings.whitespace));
        self.layout_cache.borrow_mut().clear();
        self.invalidate();
    }

    pub(super) fn toggle_log_colors(&mut self) {
        self.settings.log_colors = !self.settings.log_colors;
        self.settings.save();
        self.message = format!("ログレベルの色分け: {}", on_off(self.settings.log_colors));
        self.layout_cache.borrow_mut().clear();
        self.invalidate();
    }
}
