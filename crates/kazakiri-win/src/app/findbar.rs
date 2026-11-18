//! 入力欄（行へ移動・列フィルタなど）の部品。検索・置換の行は、標準の部品（findrow.rs）。
//! GREP（フォルダをまたぐ検索）は、従来のダイアログのまま。

use super::*;

/// バーの中の部品の位置（本文の座標。DIP）。(x0, y0, x1, y1)
type R4 = (f32, f32, f32, f32);

fn inside(r: R4, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

/// 列フィルタの演算子のチップ（演算子, 表示）
const FILTER_OPS: [(&str, &str); 10] = [("=", "="), ("!=", "!="), (":", ": 含む"), ("~", "~ 正規表現"), (">", ">"), (">=", ">="), ("<", "<"), ("<=", "<="), ("empty", "empty"), ("!empty", "!empty")];

impl App {
    /// 列フィルタのカードに足す、演算子のチップの行の高さ
    pub(super) fn filter_chip_row_h(&self) -> f32 {
        if self.prompt == Some(Prompt::ColumnFilter) { self.ui_line_h + 16.0 } else { 0.0 }
    }

    /// 列フィルタの演算子のチップの位置。(位置, 演算子, 表示)
    fn filter_chips(&self) -> Vec<(R4, &'static str, &'static str)> {
        if self.prompt != Some(Prompt::ColumnFilter) {
            return Vec::new();
        }
        let (cx0, _, cx1, _) = self.prompt_card();
        let (_, _, _, iy1) = self.prompt_input_rect();
        let (y0, h) = (iy1 + 8.0, self.ui_line_h + 8.0);
        let mut x = cx0 + 14.0;
        let mut out = Vec::new();
        for (op, label) in FILTER_OPS {
            let units: Vec<u16> = label.encode_utf16().collect();
            let w = unsafe { self.ui_text_width(&units) }.unwrap_or(40.0) + 18.0;
            if x + w > cx1 - 10.0 {
                break;
            }
            out.push(((x, y0, x + w, y0 + h), op, label));
            x += w + 6.0;
        }
        out
    }

    /// 入力欄の、いまの演算子（列フィルタ）
    fn filter_input_op(&self) -> Option<&'static str> {
        let t = self.input.trim();
        let rest = t[t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len())..].trim_start();
        ["!empty", "empty", ">=", "<=", "!=", "=", ":", "~", ">", "<"].into_iter().find(|op| rest.starts_with(op))
    }

    /// 入力欄の演算子を差し替える（「3 = ERROR」の「=」だけを変える）
    fn set_filter_op(&mut self, op: &str) {
        let t = self.input.trim().to_string();
        let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
        let col = if split == 0 { self.target_column().map_or(1, |(c, _, _)| c + 1).to_string() } else { t[..split].to_string() };
        let rest = t[split..].trim_start();
        let value = ["!empty", "empty", ">=", "<=", "!=", "=", ":", "~", ">", "<"].into_iter().find_map(|o| rest.strip_prefix(o)).unwrap_or(rest).trim();
        self.input = if matches!(op, "empty" | "!empty") { format!("{col} {op}") } else { format!("{col} {op} {value}").trim_end().to_string() + if value.is_empty() { " " } else { "" } };
        self.invalidate();
    }

    /// 列フィルタのカードのクリック。カードの上なら true
    pub(super) fn prompt_chip_click(&mut self, x: f32, y: f32) -> bool {
        if self.prompt != Some(Prompt::ColumnFilter) || !inside(self.prompt_card(), x, y) {
            return false;
        }
        if let Some(&(_, op, _)) = self.filter_chips().iter().find(|(r, _, _)| inside(*r, x, y)) {
            self.set_filter_op(op);
        }
        true
    }

    /// 列フィルタの演算子のチップを描く
    pub(super) unsafe fn draw_filter_chips(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let t = self.theme;
        let cur = self.filter_input_op();
        let accent = theme::accent(self.settings.dark);
        unsafe {
            for ((x0, y0, x1, y1), op, label) in self.filter_chips() {
                if Some(op) == cur {
                    brush.SetColor(&t.panel_sel);
                    rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, 5.0), brush);
                    brush.SetColor(&accent);
                    rt.DrawRoundedRectangle(&rrect(x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5, 5.0), brush, 1.0, None);
                } else {
                    brush.SetColor(&t.dim);
                    rt.DrawRoundedRectangle(&rrect(x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5, 5.0), brush, 1.0, None);
                }
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, label, x0 + 9.0, y0 + (y1 - y0 - self.ui_line_h) / 2.0, x1)?;
            }
        }
        Ok(())
    }

    /// 入力欄のカード（検索・置換バーなど）の上にあるときのカーソルの形。カードの外なら None
    pub(super) fn prompt_cursor(&self, x: f32, y: f32) -> Option<PCWSTR> {
        self.prompt?;
        // 検索・置換は、標準の入力欄の行（findrow.rs）。カーソルの形は部品が決める
        if self.is_find_bar() || !inside(self.prompt_card(), x, y) {
            return None;
        }
        if self.filter_chips().iter().any(|(r, _, _)| inside(*r, x, y)) {
            return Some(IDC_HAND);
        }
        if inside(self.prompt_input_rect(), x, y) {
            return Some(IDC_IBEAM);
        }
        Some(IDC_ARROW)
    }

    /// 1 行の入力欄を描く。いま入力している欄（`focused`）は、アクセントカラーの枠・カーソル・変換中の文字を出す
    pub(super) unsafe fn draw_field(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, r: R4, text: &str, focused: bool) -> Result<()> {
        let t = self.theme;
        let (ix0, iy0, ix1, iy1) = r;
        unsafe {
            if focused {
                brush.SetColor(&theme::accent(self.settings.dark));
            } else {
                brush.SetColor(&t.dim);
            }
            rt.FillRoundedRectangle(&rrect(ix0, iy0, ix1, iy1, 5.0), brush);
            brush.SetColor(&t.bg);
            rt.FillRoundedRectangle(&rrect(ix0 + 1.0, iy0 + 1.0, ix1 - 1.0, iy1 - 1.0, 4.0), brush);
            // 入力した文字（文字の上端は iy0 + 4）
            let ty = iy0 + (iy1 - iy0 - self.ui_line_h) / 2.0;
            brush.SetColor(&t.text);
            let units: Vec<u16> = text.encode_utf16().collect();
            let x = ix0 + 8.0 + self.ui_text_width(&units)?;
            rt.PushAxisAlignedClip(&rect(ix0 + 2.0, iy0, ix1 - 2.0, iy1), D2D1_ANTIALIAS_MODE_ALIASED);
            // 長いときは、末尾（入力中の位置）が見えるように左へずらす
            let shift = (x + 4.0 - (ix1 - 8.0)).max(0.0);
            self.draw_ui_str(rt, brush, text, ix0 + 8.0 - shift, ty, ix0 + 8.0 - shift + self.ui_text_width(&units)? + 4.0)?;
            if focused {
                let x = x - shift;
                match self.comp.as_ref() {
                    Some(comp) => {
                        // 変換中の文字：変換対象は背景、下線で状態を示す
                        let cw = self.ui_text_width(&comp.units)?;
                        let mut i = 0;
                        while i < comp.units.len() {
                            let a = comp.attrs.get(i).copied().unwrap_or(0);
                            let mut j = i + 1;
                            while j < comp.units.len() && comp.attrs.get(j).copied().unwrap_or(0) == a {
                                j += 1;
                            }
                            let (a0, a1) = (x + self.ui_text_width(&comp.units[..i])?, x + self.ui_text_width(&comp.units[..j])?);
                            let target = matches!(comp.attrs.get(i).copied().unwrap_or(0), 1 | 3);
                            if target {
                                brush.SetColor(&t.select);
                                rt.FillRectangle(&rect(a0, ty, a1, ty + self.ui_line_h), brush);
                            }
                            brush.SetColor(&t.text);
                            let th = if target { 2.0 } else { 1.0 };
                            let by = ty + self.ui_line_h - 1.0;
                            rt.FillRectangle(&rect(a0, by - th, a1, by), brush);
                            i = j;
                        }
                        self.draw_ui_str(rt, brush, &String::from_utf16_lossy(&comp.units), x, ty, x + cw + 4.0)?;
                        let cx = x + self.ui_text_width(&comp.units[..comp.cursor.min(comp.units.len())])?;
                        brush.SetColor(&t.caret);
                        rt.FillRectangle(&rect(cx - 0.5, ty + 1.0, cx + 1.5, ty + self.ui_line_h - 1.0), brush);
                    }
                    None => {
                        brush.SetColor(&t.caret);
                        rt.FillRectangle(&rect(x, ty + 1.0, x + 1.5, ty + self.ui_line_h - 1.0), brush);
                    }
                }
            }
            rt.PopAxisAlignedClip();
        }
        Ok(())
    }
}
