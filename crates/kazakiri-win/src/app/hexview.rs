//! 16 進表示（表示 → 16 進表示、Ctrl+Shift+H）。ファイルのバイト列を、オフセット・16 進・ASCII の行で見る。
//!
//! 見えている行（16 バイトずつ）だけ読むので、ファイルがどれだけ大きくても表示のコストは一定。
//! 読み取り専用。↑↓ PageUp/PageDown Home/End・ホイール・スクロールバーで動かし、Esc（または同じコマンド）で閉じる。
//! 開くときは本文のカーソルの位置から、閉じるときは先頭の行の位置へカーソルを移す。

use super::*;

/// 1 行に並べるバイト数
const COLS: u64 = 16;

pub(super) struct HexView {
    /// 先頭の行（オフセット ÷ 16）
    top: u64,
}

/// 16 進表示の 1 行ぶんの文字列（オフセット、16 進、ASCII の 3 つ）。`width` はオフセットの桁数
fn row_text(offset: u64, bytes: &[u8], width: usize) -> (String, String, String) {
    let mut hex = String::with_capacity(COLS as usize * 3 + 1);
    for i in 0..COLS as usize {
        if i == 8 {
            hex.push(' ');
        }
        match bytes.get(i) {
            Some(b) => hex.push_str(&format!("{b:02X} ")),
            None => hex.push_str("   "),
        }
    }
    let ascii = bytes.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
    (format!("{offset:0width$X}"), hex, ascii)
}

/// オフセットの桁数（4GB を超えるなら 12 桁）
fn offset_width(len: u64) -> usize {
    if len > u32::MAX as u64 {
        12
    } else {
        8
    }
}

impl App {
    fn hex_rows(&self) -> u64 {
        self.doc.as_ref().map_or(0, |d| d.len().div_ceil(COLS))
    }

    fn hex_rows_visible(&self) -> usize {
        (((self.client_size().1 - self.status_h() - self.line_h - 6.0) / self.line_h).floor() as usize).max(1)
    }

    /// 開く・閉じる
    pub(super) fn hex_toggle(&mut self) {
        if self.hexview.is_some() {
            return self.hex_close();
        }
        let Some(doc) = self.doc.clone() else { return };
        self.md_close();
        self.diffview = None;
        let at = self.caret().min(doc.len());
        let top = at / COLS;
        self.hexview = Some(HexView { top });
        self.message = "16 進表示: ↑↓ PageUp/PageDown Home/End・ホイールで動かす。Esc で閉じる（先頭の行の位置へカーソルが移る）".into();
        self.hex_scroll(0);
        self.apply_font();
    }

    /// 閉じる：先頭の行の位置へカーソルを移して、本文をそこへ
    pub(super) fn hex_close(&mut self) {
        let Some(v) = self.hexview.take() else { return };
        if let Some(doc) = self.doc.clone() {
            let off = (v.top * COLS).min(doc.len());
            let off = off.max(doc.text_start() as u64);
            if let Some(line) = doc.line_of_offset(off) {
                self.scroll_to(line);
            }
            self.sel = Some((off, off));
        }
        self.message = "16 進表示を閉じました".into();
        self.apply_font();
    }

    pub(super) fn hex_scroll(&mut self, delta: i64) {
        let vis = self.hex_rows_visible() as u64;
        let max = self.hex_rows().saturating_sub(vis) as i64;
        if let Some(v) = &mut self.hexview {
            v.top = (v.top as i64).saturating_add(delta).clamp(0, max) as u64;
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// 16 進表示中のキー。処理したら true
    pub(super) fn hex_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.hexview.is_none() {
            return false;
        }
        let page = self.hex_rows_visible() as i64;
        match vk {
            VK_ESCAPE => self.hex_close(),
            VK_UP => self.hex_scroll(-1),
            VK_DOWN => self.hex_scroll(1),
            VK_PRIOR => self.hex_scroll(-page),
            VK_NEXT => self.hex_scroll(page),
            VK_HOME => self.hex_scroll(i64::MIN / 2),
            VK_END => self.hex_scroll(i64::MAX / 2),
            // 内容を変える Ctrl のキー（貼り付け・切り取り・元に戻す／やり直し）は、読み取り専用なので受け付けない。
            // そのほかの Ctrl 付きのキー（タブの切り替え、この表示を閉じるなど）は通常どおり
            VK_V | VK_X | VK_Z | VK_Y if ctrl => {}
            _ if ctrl => return false,
            _ => {}
        }
        true
    }

    /// スクロールバーの (最大, ページ, 位置)。行数が多いときは縮尺をかける
    pub(super) fn hex_scroll_info(&self) -> Option<(i32, u32, i32, u64)> {
        let v = self.hexview.as_ref()?;
        let total = self.hex_rows();
        let scale = (total / (i32::MAX as u64 / 2)).max(1);
        let page = self.hex_rows_visible() as u64;
        Some((((total.saturating_sub(1)) / scale) as i32, (page / scale).max(1) as u32, (v.top / scale) as i32, scale))
    }

    /// スクロールバーの操作
    pub(super) fn hex_vscroll(&mut self, code: SCROLLBAR_COMMAND, track: i32) {
        let page = self.hex_rows_visible() as i64;
        let scale = self.hex_scroll_info().map_or(1, |i| i.3);
        match code {
            SB_LINEUP => self.hex_scroll(-1),
            SB_LINEDOWN => self.hex_scroll(1),
            SB_PAGEUP => self.hex_scroll(-page),
            SB_PAGEDOWN => self.hex_scroll(page),
            SB_TOP => self.hex_scroll(i64::MIN / 2),
            SB_BOTTOM => self.hex_scroll(i64::MAX / 2),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let cur = self.hexview.as_ref().map_or(0, |v| v.top) as i64;
                self.hex_scroll((track as u64).saturating_mul(scale) as i64 - cur);
            }
            _ => {}
        }
    }

    // ---- 描画 ----

    pub(super) unsafe fn draw_hexview(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, body_h: f32) -> Result<()> {
        let (Some(v), Some(doc)) = (&self.hexview, &self.doc) else { return Ok(()) };
        let t = self.theme;
        let len = doc.len();
        let width = offset_width(len);
        let head = self.line_h + 6.0;
        let cw = self.char_w;
        let x_off = cw * 1.0;
        let x_hex = x_off + cw * (width as f32 + 2.0);
        let x_ascii = x_hex + cw * (COLS as f32 * 3.0 + 1.0 + 2.0);
        unsafe {
            // 見出し：列の位置（00 01 … 0F）
            brush.SetColor(&t.gutter_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, head), brush);
            brush.SetColor(&t.gutter_text);
            self.draw_str(rt, brush, "Offset", x_off, 3.0, x_hex)?;
            let cols: String = (0..COLS as usize).map(|i| format!("{}{i:02X} ", if i == 8 { " " } else { "" })).collect();
            self.draw_str(rt, brush, &cols, x_hex, 3.0, x_ascii)?;
            self.draw_str(rt, brush, "ASCII", x_ascii, 3.0, w)?;
            let vis = ((body_h - head) / self.line_h).ceil() as u64;
            for n in 0..vis {
                let off = (v.top + n) * COLS;
                if off >= len {
                    break;
                }
                let bytes = doc.read(off, (off + COLS).min(len));
                let y = head + n as f32 * self.line_h;
                let (o, h, a) = row_text(off, &bytes, width);
                brush.SetColor(&t.gutter_text);
                self.draw_str(rt, brush, &o, x_off, y, x_hex)?;
                brush.SetColor(&t.text);
                self.draw_str(rt, brush, &h, x_hex, y, x_ascii)?;
                brush.SetColor(&t.dim);
                self.draw_str(rt, brush, &a, x_ascii, y, w)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_formats_full_and_short_rows() {
        let (o, h, a) = row_text(0x10, b"Hello, World!\n\x00\xFF", 8);
        assert_eq!(o, "00000010");
        assert_eq!(h, "48 65 6C 6C 6F 2C 20 57  6F 72 6C 64 21 0A 00 FF ");
        assert_eq!(a, "Hello, World!...");
        // 最後の行は、16 進の欄を空白で埋めて ASCII の位置をそろえる
        let (_, h, a) = row_text(0, b"AB", 8);
        assert_eq!(h.len(), 16 * 3 + 1);
        assert!(h.starts_with("41 42    "));
        assert_eq!(a, "AB");
    }

    #[test]
    fn offset_width_grows_over_4gb() {
        assert_eq!(offset_width(1 << 20), 8);
        assert_eq!(offset_width(5 << 30), 12);
    }
}
