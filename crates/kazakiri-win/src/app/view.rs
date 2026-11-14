//! 表示の設定：文字の大きさ、テーマ、空白の可視化、ログの色分け。変更は設定ファイルに保存する。

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};

use super::*;

impl App {
    /// フォントを作り直す（大きさの変更など）
    pub(super) fn apply_font(&mut self) {
        let s = &self.settings;
        let Ok(configured) = (unsafe { create_format(&self.dw, &s.font, s.font_size) }) else {
            self.message = "フォントを作れませんでした".into();
            return self.invalidate();
        };
        self.font_prop = unsafe { is_proportional(&self.dw, &configured) };
        // CSV 表示で等幅でないフォントなら、列がそろうよう既定の等幅フォントを使う
        let over = self.font_prop && self.csv.is_some();
        let format = if over { unsafe { create_format(&self.dw, "", s.font_size) }.unwrap_or(configured) } else { configured };
        self.font_overridden = over;
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
        self.settings.save();
        self.message = format!("ダークテーマ: {}", on_off(dark));
        self.invalidate();
    }

    /// ウィンドウの縁（DWM が描く細い枠線）とポップアップメニュー・スクロールバーを、テーマに合わせる（Windows 10 20H1 以降）
    pub(super) fn apply_title_theme(&mut self) {
        let v = BOOL::from(self.settings.dark);
        unsafe {
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, size_of::<BOOL>() as u32);
            super::dark::apply(self.hwnd, self.settings.dark);
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

impl App {
    /// 設定ダイアログの入力欄：フォント名（空なら自動）、大きさ、タブ幅
    pub(super) fn set_text_settings(&mut self, font: &str, size: f32, tab: usize) {
        let size = size.clamp(settings::MIN_FONT, settings::MAX_FONT);
        let tab = tab.clamp(1, 32);
        if font == self.settings.font && size == self.settings.font_size && tab == self.settings.tab_width {
            return;
        }
        self.settings.font = font.to_string();
        self.settings.font_size = size;
        self.settings.tab_width = tab;
        self.apply_font();
        self.settings.save();
    }
}

impl App {
    /// 画面の部品（メニュー・タブ・ツールバー・ステータスバー）のフォントを作り直す
    pub(super) fn set_ui_font(&mut self, font: &str, size: f32) {
        let size = size.clamp(settings::MIN_FONT, 40.0);
        if font == self.settings.ui_font && size == self.settings.ui_font_size {
            return;
        }
        let Ok(format) = (unsafe { create_ui_format(&self.dw, font, size) }) else {
            self.message = "フォントを作れませんでした".into();
            return self.invalidate();
        };
        let (char_w, line_h) = unsafe { measure(&self.dw, &format) }.unwrap_or((size * 0.5, (size * 1.4).ceil()));
        self.settings.ui_font = font.to_string();
        self.settings.ui_font_size = size;
        self.ui_format = format;
        self.ui_width_cache.borrow_mut().clear();
        self.ui_char_w = char_w;
        self.ui_line_h = line_h;
        self.layout_cache.borrow_mut().clear();
        self.update_scrollbars();
        self.update_ime_position();
        self.settings.save();
        self.invalidate();
    }
}
