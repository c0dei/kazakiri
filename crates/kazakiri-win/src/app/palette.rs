//! コマンドパレット（Ctrl+Shift+P）。メニューのコマンドを、名前で絞り込んで実行する。
//! 本文の上に浮かぶカードで、入力欄は 1 行の簡易な欄（検索バーなどと同じ作り。日本語入力も欄の中で行う）。
//! 一覧は、メニューを作るときに集めたもの（`menu::palette_items`）。

use super::*;

/// 一覧に出す行の数の上限
const MAX_ROWS: usize = 10;

type R4 = (f32, f32, f32, f32);

pub(super) struct PaletteLayout {
    pub card: R4,
    pub input: R4,
    rows: R4,
    row_h: f32,
    count: usize,
}

fn inside(r: R4, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

impl App {
    pub(super) fn start_palette(&mut self) {
        self.prompt = Some(Prompt::Palette);
        self.input.clear();
        self.palette_sel = 0;
        self.palette_top = 0;
        self.comp = None;
        self.invalidate();
    }

    /// 入力で絞り込んだコマンド（名前, キー, ID）。語は空白で区切り、すべて含むものだけ。名前が先頭から合うものを前に
    pub(super) fn palette_results(&self) -> Vec<(String, String, u32)> {
        let q = self.input.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut v: Vec<(String, String, u32)> = menu::palette_items().into_iter().filter(|(name, key, _)| {
            let n = name.to_lowercase();
            terms.iter().all(|t| n.contains(t) || key.to_lowercase().contains(t))
        }).collect();
        if let Some(first) = terms.first() {
            v.sort_by_key(|(name, _, _)| !name.to_lowercase().starts_with(first));
        }
        v
    }

    pub(super) fn palette_layout(&self) -> PaletteLayout {
        let w = self.client_size().0;
        let cw = (w - 40.0).clamp(320.0, 620.0).min(w);
        let (x0, y0) = (((w - cw) / 2.0).max(0.0), self.pin_h() + 14.0);
        let input = (x0 + 12.0, y0 + 12.0, x0 + cw - 12.0, y0 + 12.0 + self.ui_line_h + 12.0);
        let row_h = self.ui_line_h + 12.0;
        let count = self.palette_results().len().clamp(1, MAX_ROWS);
        let rows = (x0 + 6.0, input.3 + 8.0, x0 + cw - 6.0, input.3 + 8.0 + count as f32 * row_h);
        PaletteLayout { card: (x0, y0, x0 + cw, rows.3 + 8.0), input, rows, row_h, count }
    }

    /// 選んでいる行を動かす（矢印・PageUp/Down・Home/End）
    pub(super) fn palette_move(&mut self, vk: VIRTUAL_KEY) {
        let n = self.palette_results().len();
        if n == 0 {
            return;
        }
        let last = n - 1;
        self.palette_sel = match vk {
            VK_UP => self.palette_sel.saturating_sub(1),
            VK_DOWN => (self.palette_sel + 1).min(last),
            VK_PRIOR => self.palette_sel.saturating_sub(MAX_ROWS),
            VK_NEXT => (self.palette_sel + MAX_ROWS).min(last),
            VK_HOME => 0,
            _ => last,
        };
        if self.palette_sel < self.palette_top {
            self.palette_top = self.palette_sel;
        } else if self.palette_sel >= self.palette_top + MAX_ROWS {
            self.palette_top = self.palette_sel + 1 - MAX_ROWS;
        }
        self.invalidate();
    }

    /// クリック：行を押したらそのコマンドを実行。パレットの外を押したら閉じる。
    /// パレットの上なら Some（中身は、実行したコマンドの続き）、外なら None
    pub(super) unsafe fn palette_click(&mut self, x: f32, y: f32) -> Option<Option<Deferred>> {
        if self.prompt != Some(Prompt::Palette) {
            return None;
        }
        let l = self.palette_layout();
        if !inside(l.card, x, y) {
            self.prompt = None;
            self.comp = None;
            self.invalidate();
            return None;
        }
        if inside(l.rows, x, y) {
            let i = self.palette_top + ((y - l.rows.1) / l.row_h) as usize;
            if let Some(id) = self.palette_results().get(i).map(|e| e.2) {
                self.prompt = None;
                self.comp = None;
                self.invalidate();
                return Some(unsafe { self.command(id) });
            }
        }
        self.invalidate();
        Some(None)
    }

    pub(super) unsafe fn draw_palette(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let t = self.theme;
        let l = self.palette_layout();
        let (cx0, cy0, cx1, cy1) = l.card;
        let results = self.palette_results();
        unsafe {
            // 影・枠・面
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.14 });
            rt.FillRoundedRectangle(&rrect(cx0 - 2.0, cy0 + 1.0, cx1 + 2.0, cy1 + 9.0, 12.0), brush);
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.18 });
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 + 3.0, cx1 + 1.0, cy1 + 5.0, 10.0), brush);
            brush.SetColor(&t.dim);
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 - 1.0, cx1 + 1.0, cy1 + 1.0, 10.0), brush);
            brush.SetColor(&t.panel_head);
            rt.FillRoundedRectangle(&rrect(cx0, cy0, cx1, cy1, 9.0), brush);
            // 入力欄
            self.draw_field(rt, brush, l.input, &self.input, true)?;
            if self.input.is_empty() && self.comp.is_none() {
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, "コマンドを検索…", l.input.0 + 14.0, l.input.1 + (l.input.3 - l.input.1 - self.ui_line_h) / 2.0, l.input.2 - 8.0)?;
            }
            // 一覧
            let (rx0, ry0, rx1, _) = l.rows;
            if results.is_empty() {
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, "一致するコマンドがありません", rx0 + 12.0, ry0 + (l.row_h - self.ui_line_h) / 2.0, rx1 - 12.0)?;
                return Ok(());
            }
            for k in 0..l.count {
                let i = self.palette_top + k;
                let Some((name, key, _)) = results.get(i) else { break };
                let y = ry0 + k as f32 * l.row_h;
                if i == self.palette_sel {
                    brush.SetColor(&t.panel_sel);
                    rt.FillRoundedRectangle(&rrect(rx0, y + 1.0, rx1, y + l.row_h - 1.0, 5.0), brush);
                }
                let ty = y + (l.row_h - self.ui_line_h) / 2.0;
                let kw = if key.is_empty() { 0.0 } else { self.ui_text_width(&key.encode_utf16().collect::<Vec<_>>())? + 10.0 };
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, name, rx0 + 12.0, ty, rx1 - kw - 20.0)?;
                if !key.is_empty() {
                    brush.SetColor(&t.gutter_text);
                    self.draw_ui_str(rt, brush, key, rx1 - 12.0 - kw + 10.0, ty, rx1 - 6.0)?;
                }
            }
        }
        Ok(())
    }
}
