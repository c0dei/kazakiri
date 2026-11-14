//! 検索・置換バー（検索 Ctrl+F・置換 Ctrl+H）。本文の右上に浮かぶカードで、検索語（と置換語）を入れて Enter で次を探す。
//! 入力欄は 1 行の簡易な欄（行へ移動などの入力欄と同じ作り）で、いま入力している欄は `prompt_focus`（0 = 検索、1 = 置換）。
//! GREP（フォルダをまたぐ検索）は、従来のダイアログのまま。

use super::*;

/// バーの中の部品の位置（本文の座標。DIP）。(x0, y0, x1, y1)
type R4 = (f32, f32, f32, f32);

pub(super) struct FindLayout {
    pub card: R4,
    pub find: R4,
    pub repl: Option<R4>,
    pub buttons: Vec<(R4, FindAct)>,
    hint_y: f32,
}

fn inside(r: R4, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

impl App {
    pub(super) fn find_layout(&self) -> FindLayout {
        let w = self.client_size().0;
        let cw = (w - 40.0).clamp(360.0, 640.0).min(w);
        let (x0, y0) = ((w - cw - 20.0).max(0.0), self.pin_h() + 10.0);
        let (pad, bw, gap) = (10.0, 28.0, 4.0);
        let rh = self.ui_line_h + 10.0;
        let mut buttons = Vec::new();
        // 1 行目：検索欄、右に ［Aa］［.*］［語］［↑］［↓］［✕］
        let y1 = y0 + pad;
        let mut xr = x0 + cw - pad;
        for act in [FindAct::Close, FindAct::Next, FindAct::Prev, FindAct::Word, FindAct::Regex, FindAct::Case] {
            buttons.push(((xr - bw, y1, xr, y1 + rh), act));
            xr -= bw + gap;
        }
        let find = (x0 + pad, y1, xr - 4.0, y1 + rh);
        let mut bottom = y1 + rh;
        let mut repl = None;
        // 2 行目（置換）：置換欄、右に ［置換］［すべて置換］
        if self.prompt == Some(Prompt::Replace) {
            let y2 = bottom + 6.0;
            let (all_w, one_w) = (88.0, 56.0);
            let right = x0 + cw - pad;
            buttons.push(((right - all_w, y2, right, y2 + rh), FindAct::ReplAll));
            buttons.push(((right - all_w - gap - one_w, y2, right - all_w - gap, y2 + rh), FindAct::ReplOne));
            repl = Some((x0 + pad, y2, right - all_w - gap - one_w - 8.0, y2 + rh));
            bottom = y2 + rh;
        }
        let hint_y = bottom + 6.0;
        FindLayout { card: (x0, y0, x0 + cw, hint_y + self.ui_line_h + 8.0), find, repl, buttons, hint_y }
    }

    /// 入力欄のカード（検索・置換バーなど）の上にあるときのカーソルの形。カードの外なら None
    pub(super) fn prompt_cursor(&self, x: f32, y: f32) -> Option<PCWSTR> {
        self.prompt?;
        if !inside(self.prompt_card(), x, y) {
            return None;
        }
        if self.is_find_bar() {
            let l = self.find_layout();
            if l.buttons.iter().any(|(r, _)| inside(*r, x, y)) {
                return Some(IDC_HAND);
            }
            if inside(l.find, x, y) || l.repl.is_some_and(|r| inside(r, x, y)) {
                return Some(IDC_IBEAM);
            }
        } else if inside(self.prompt_input_rect(), x, y) {
            return Some(IDC_IBEAM);
        }
        Some(IDC_ARROW)
    }

    /// 検索・置換バーのクリック。バーの上なら true（本文は処理しない）。バーの外を押したら、バーを閉じて本文の操作に進む
    pub(super) fn find_bar_click(&mut self, x: f32, y: f32) -> bool {
        if !self.is_find_bar() {
            return false;
        }
        let l = self.find_layout();
        if !inside(l.card, x, y) {
            self.prompt = None;
            self.comp = None;
            self.invalidate();
            return false;
        }
        if let Some(&(_, act)) = l.buttons.iter().find(|(r, _)| inside(*r, x, y)) {
            self.find_bar_do(act);
        } else if inside(l.find, x, y) {
            self.prompt_focus = 0;
            self.comp = None;
        } else if l.repl.is_some_and(|r| inside(r, x, y)) {
            self.prompt_focus = 1;
            self.comp = None;
        }
        self.invalidate();
        true
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
                match self.comp.as_ref().filter(|_| self.ime_inline_on()) {
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

    /// 検索・置換バーを描く
    pub(super) unsafe fn draw_find_bar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let t = self.theme;
        let l = self.find_layout();
        let (cx0, cy0, cx1, cy1) = l.card;
        unsafe {
            // 影・枠・面
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.22 });
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 + 2.0, cx1 + 1.0, cy1 + 4.0, 10.0), brush);
            brush.SetColor(&t.dim);
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 - 1.0, cx1 + 1.0, cy1 + 1.0, 9.0), brush);
            brush.SetColor(&t.panel_head);
            rt.FillRoundedRectangle(&rrect(cx0, cy0, cx1, cy1, 8.0), brush);
            // 入力欄
            let replace_focus = self.prompt == Some(Prompt::Replace) && self.prompt_focus == 1;
            self.draw_field(rt, brush, l.find, &self.input, !replace_focus)?;
            if let Some(r) = l.repl {
                self.draw_field(rt, brush, r, &self.input2, replace_focus)?;
                if self.input2.is_empty() && !replace_focus {
                    brush.SetColor(&t.gutter_text);
                    self.draw_ui_str(rt, brush, "置換後の文字列", r.0 + 8.0, r.1 + (r.3 - r.1 - self.ui_line_h) / 2.0, r.2 - 8.0)?;
                }
            }
            if self.input.is_empty() && replace_focus {
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, "検索する文字列", l.find.0 + 8.0, l.find.1 + (l.find.3 - l.find.1 - self.ui_line_h) / 2.0, l.find.2 - 8.0)?;
            } else if self.input.is_empty() && self.comp.is_none() {
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, "検索する文字列", l.find.0 + 14.0, l.find.1 + (l.find.3 - l.find.1 - self.ui_line_h) / 2.0, l.find.2 - 8.0)?;
            }
            // ボタン
            let accent = theme::accent(self.settings.dark);
            for &((x0, y0, x1, y1), act) in &l.buttons {
                let on = match act {
                    FindAct::Case => self.opts.case_sensitive,
                    FindAct::Regex => self.opts.regex,
                    FindAct::Word => self.opts.whole_word,
                    _ => false,
                };
                let text_btn = matches!(act, FindAct::ReplOne | FindAct::ReplAll);
                if on {
                    brush.SetColor(&t.panel_sel);
                    rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, 5.0), brush);
                    brush.SetColor(&accent);
                    rt.DrawRoundedRectangle(&rrect(x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5, 5.0), brush, 1.0, None);
                } else if act == FindAct::ReplAll {
                    // 主な操作：アクセントカラーで塗る
                    brush.SetColor(&accent);
                    rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, 5.0), brush);
                } else if text_btn {
                    // 入力欄と同じ面に枠を付けて、ボタンに見せる
                    brush.SetColor(&t.dim);
                    rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, 5.0), brush);
                    brush.SetColor(&t.bg);
                    rt.FillRoundedRectangle(&rrect(x0 + 1.0, y0 + 1.0, x1 - 1.0, y1 - 1.0, 4.0), brush);
                }
                // アクセントで塗ったボタンの上の文字は、暗いテーマでは黒、明るいテーマでは白
                let on_accent = act == FindAct::ReplAll;
                brush.SetColor(if on_accent { if self.settings.dark { &D2D1_COLOR_F { r: 0.05, g: 0.05, b: 0.05, a: 1.0 } } else { &D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 } } } else { &t.text });
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let label = match act {
                    FindAct::Case => Some("Aa"),
                    FindAct::Regex => Some(".*"),
                    FindAct::Word => Some("語"),
                    FindAct::ReplOne => Some("置換"),
                    FindAct::ReplAll => Some("すべて置換"),
                    _ => None,
                };
                if let Some(label) = label {
                    let units: Vec<u16> = label.encode_utf16().collect();
                    let tw = self.ui_text_width(&units)?;
                    self.draw_ui_str(rt, brush, label, cx - tw / 2.0, cy - self.ui_line_h / 2.0, x1)?;
                    continue;
                }
                // ↑ ↓ ✕ は線で描く
                let line = |a: (f32, f32), b: (f32, f32)| rt.DrawLine(Vector2 { X: cx + a.0, Y: cy + a.1 }, Vector2 { X: cx + b.0, Y: cy + b.1 }, brush, 1.4, None);
                match act {
                    FindAct::Prev => {
                        line((0.0, 4.5), (0.0, -4.5));
                        line((-4.0, -0.5), (0.0, -4.5));
                        line((4.0, -0.5), (0.0, -4.5));
                    }
                    FindAct::Next => {
                        line((0.0, -4.5), (0.0, 4.5));
                        line((-4.0, 0.5), (0.0, 4.5));
                        line((4.0, 0.5), (0.0, 4.5));
                    }
                    _ => {
                        line((-4.0, -4.0), (4.0, 4.0));
                        line((-4.0, 4.0), (4.0, -4.0));
                    }
                }
            }
            // 操作の説明
            brush.SetColor(&t.gutter_text);
            let hint = if self.prompt == Some(Prompt::Replace) {
                "Enter: 次を検索（置換欄では置換して次へ）　Tab: 欄の移動　Shift+Enter: 前を検索　Esc: 閉じる"
            } else {
                "Enter: 次を検索　Shift+Enter: 前を検索　F3: 次へ　Esc: 閉じる"
            };
            self.draw_ui_str(rt, brush, hint, cx0 + 12.0, l.hint_y, cx1 - 12.0)?;
        }
        Ok(())
    }
}
