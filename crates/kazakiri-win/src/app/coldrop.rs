//! 「対象の列」のドロップダウン（モードバー）。欄の真下に開く一覧で、列を選ぶと並べ替え・列フィルタの対象になる。
//! 列が多いときは、12 行までの高さで、ホイールや矢印キーでスクロールする。Esc・欄の外のクリックで閉じる。

use super::*;

/// 一覧に出す行の数の上限
const MAX_ROWS: usize = 12;

pub(super) struct ColDrop {
    /// 表示と、選んだときの意味（None = カーソルのある列に合わせる、Some(n) = n 番目の列）
    items: Vec<(String, Option<usize>)>,
    /// いま対象になっている行
    current: usize,
    /// マウス・キーで指している行
    hover: usize,
    /// 先頭に見えている行
    top: usize,
}

/// 一覧の位置と大きさ（窓の座標。DIP）
struct DropRect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    row_h: f32,
    rows: usize,
}

impl App {
    /// 「対象の列」の一覧を開く（開いていれば閉じる）。欄が見えていないとき（モードバーが無いとき）は、従来のメニュー
    pub(super) fn toggle_col_drop(&mut self) -> Option<Deferred> {
        if self.col_drop.take().is_some() {
            self.invalidate();
            return None;
        }
        let Some((cur, total, _)) = self.target_column() else { return None };
        if self.colpick_box().is_none() {
            return Some(Deferred::ColMenu);
        }
        let Some(v) = self.csv.as_ref() else { return None };
        let follow = v.target.is_none();
        let names = self.csv_column_names(usize::MAX);
        let mut items = vec![("カーソルのある列に合わせる".to_string(), None)];
        for i in 0..total.min(200) {
            let label = match names.get(i) {
                Some(n) if !n.is_empty() => format!("{}: {}", i + 1, n),
                _ => format!("{} 列目", i + 1),
            };
            items.push((label, Some(i)));
        }
        let current = if follow { 0 } else { 1 + cur.min(total.saturating_sub(1)) };
        // いまの行が見える位置から開く
        let top = (current + 1).saturating_sub(MAX_ROWS);
        self.col_drop = Some(ColDrop { items, current, hover: current, top });
        self.hide_tip();
        self.hover = None;
        self.invalidate();
        None
    }

    /// CSV 表示でなくなっていたら、一覧は閉じる
    fn col_drop_validate(&mut self) {
        if self.col_drop.is_some() && self.csv.is_none() {
            self.col_drop = None;
        }
    }

    fn drop_rect(&self) -> Option<DropRect> {
        let d = self.col_drop.as_ref()?;
        let (bx0, bx1) = self.colpick_box()?;
        let w = self.client_size().0;
        let row_h = self.ui_line_h + 12.0;
        let widest = d.items.iter().map(|(s, _)| unsafe { self.ui_text_width(&s.encode_utf16().collect::<Vec<_>>()) }.unwrap_or(0.0)).fold(0.0f32, f32::max);
        let width = (bx1 - bx0).max(widest + 56.0).min((w - 8.0).max(120.0));
        let x0 = bx0.min((w - width - 4.0).max(0.0));
        let y0 = super::toolbar::ROW1_H + super::toolbar::MODE_H - 1.0;
        let rows = d.items.len().min(MAX_ROWS);
        Some(DropRect { x0, y0, x1: x0 + width, y1: y0 + rows as f32 * row_h + 8.0, row_h, rows })
    }

    /// 窓の座標の点にある行の番号（一覧の上でなければ None）
    fn drop_row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (r, d) = (self.drop_rect()?, self.col_drop.as_ref()?);
        if x < r.x0 || x >= r.x1 || y < r.y0 + 4.0 || y >= r.y1 - 4.0 {
            return None;
        }
        let i = d.top + ((y - r.y0 - 4.0) / r.row_h) as usize;
        (i < d.items.len()).then_some(i)
    }

    /// マウスボタンを押した（本文の座標 → 窓の座標に直す）。一覧が開いていれば true（ほかの処理はしない）
    pub(super) unsafe fn col_drop_click(&mut self, x: f32, y: f32) -> bool {
        self.col_drop_validate();
        if self.col_drop.is_none() {
            return false;
        }
        match self.drop_row_at(x, y + self.chrome_h()) {
            Some(i) => self.col_drop_pick(i),
            None => {
                // 一覧の外：閉じる（欄を押したときも、開き直さずに閉じる）
                self.col_drop = None;
                self.invalidate();
            }
        }
        true
    }

    unsafe fn col_drop_pick(&mut self, i: usize) {
        let Some(d) = self.col_drop.take() else { return };
        if let Some((_, target)) = d.items.get(i) {
            match target {
                None => {
                    let _ = unsafe { self.command(menu::ID_COL_FOLLOW) };
                }
                Some(c) => {
                    let _ = unsafe { self.command(menu::ID_COL_BASE + *c as u32) };
                }
            }
        }
        self.invalidate();
    }

    /// マウスが動いた：指している行を変える
    pub(super) fn col_drop_move(&mut self, x: f32, y: f32) -> bool {
        if self.col_drop.is_none() {
            return false;
        }
        if let Some(i) = self.drop_row_at(x, y + self.chrome_h()) {
            if let Some(d) = self.col_drop.as_mut() {
                if d.hover != i {
                    d.hover = i;
                    self.invalidate();
                }
            }
        }
        true
    }

    pub(super) fn col_drop_wheel(&mut self, delta: i64) -> bool {
        let Some(d) = self.col_drop.as_mut() else { return false };
        let max_top = d.items.len().saturating_sub(MAX_ROWS);
        let step = (delta.abs() / 120).max(1) as usize * 3;
        d.top = if delta > 0 { d.top.saturating_sub(step) } else { (d.top + step).min(max_top) };
        self.invalidate();
        true
    }

    /// キー操作。一覧が開いていれば true（ほかの処理はしない）
    pub(super) unsafe fn col_drop_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        self.col_drop_validate();
        let Some(d) = self.col_drop.as_mut() else { return false };
        let last = d.items.len().saturating_sub(1);
        match vk {
            VK_ESCAPE => self.col_drop = None,
            VK_UP => d.hover = d.hover.saturating_sub(1),
            VK_DOWN => d.hover = (d.hover + 1).min(last),
            VK_PRIOR => d.hover = d.hover.saturating_sub(MAX_ROWS),
            VK_NEXT => d.hover = (d.hover + MAX_ROWS).min(last),
            VK_HOME => d.hover = 0,
            VK_END => d.hover = last,
            VK_RETURN | VK_SPACE => {
                let i = d.hover;
                unsafe { self.col_drop_pick(i) };
                return true;
            }
            _ => {}
        }
        // 指している行が見えるようにスクロール
        if let Some(d) = self.col_drop.as_mut() {
            if d.hover < d.top {
                d.top = d.hover;
            } else if d.hover >= d.top + MAX_ROWS {
                d.top = d.hover + 1 - MAX_ROWS;
            }
        }
        self.invalidate();
        true
    }

    /// 一覧を描く（窓の座標。ほかの描画の上に重ねる）
    pub(super) unsafe fn draw_col_drop(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let (Some(r), Some(d)) = (self.drop_rect(), self.col_drop.as_ref()) else { return Ok(()) };
        let t = self.theme;
        let accent = theme::accent(self.settings.dark);
        unsafe {
            // 影・枠・面
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.14 });
            rt.FillRoundedRectangle(&rrect(r.x0 - 2.0, r.y0 + 1.0, r.x1 + 2.0, r.y1 + 7.0, 11.0), brush);
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.18 });
            rt.FillRoundedRectangle(&rrect(r.x0 - 1.0, r.y0 + 2.0, r.x1 + 1.0, r.y1 + 4.0, 9.0), brush);
            brush.SetColor(&t.dim);
            rt.FillRoundedRectangle(&rrect(r.x0 - 1.0, r.y0 - 1.0, r.x1 + 1.0, r.y1 + 1.0, 9.0), brush);
            brush.SetColor(&t.panel_head);
            rt.FillRoundedRectangle(&rrect(r.x0, r.y0, r.x1, r.y1, 8.0), brush);
            rt.PushAxisAlignedClip(&rect(r.x0, r.y0 + 4.0, r.x1, r.y1 - 4.0), D2D1_ANTIALIAS_MODE_ALIASED);
            for k in 0..r.rows {
                let i = d.top + k;
                let Some((label, target)) = d.items.get(i) else { break };
                let y = r.y0 + 4.0 + k as f32 * r.row_h;
                if i == d.hover {
                    brush.SetColor(&t.panel_sel);
                    rt.FillRoundedRectangle(&rrect(r.x0 + 4.0, y + 1.0, r.x1 - 4.0, y + r.row_h - 1.0, 4.0), brush);
                }
                if i == d.current {
                    // いま対象の行：左にアクセントカラーの短い帯
                    brush.SetColor(&accent);
                    rt.FillRoundedRectangle(&rrect(r.x0 + 6.0, y + 7.0, r.x0 + 9.0, y + r.row_h - 7.0, 1.5), brush);
                }
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, label, r.x0 + 18.0, y + (r.row_h - self.ui_line_h) / 2.0, r.x1 - 10.0)?;
                // 先頭の項目（列ではなく設定）と列の間に、細い区切り線
                if target.is_none() {
                    brush.SetColor(&t.dim);
                    rt.FillRectangle(&rect(r.x0 + 10.0, y + r.row_h - 0.5, r.x1 - 10.0, y + r.row_h + 0.5), brush);
                }
            }
            rt.PopAxisAlignedClip();
            // スクロールの印
            if d.items.len() > MAX_ROWS {
                let track = r.y1 - r.y0 - 12.0;
                let (len, start) = (track * MAX_ROWS as f32 / d.items.len() as f32, track * d.top as f32 / d.items.len() as f32);
                brush.SetColor(&t.dim);
                rt.FillRoundedRectangle(&rrect(r.x1 - 6.0, r.y0 + 6.0 + start, r.x1 - 3.0, r.y0 + 6.0 + start + len, 1.5), brush);
            }
        }
        Ok(())
    }
}
