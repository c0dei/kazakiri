//! 矩形選択（Alt+ドラッグ、Alt+Shift+矢印）。
//!
//! 行の範囲と、行頭からの x 座標の範囲で四角を表す。各行のバイト範囲はそのつど求めるので、
//! 比例フォントや CSV 表示でも「見た目の四角」になる。
//! 選択（`sel`）はカーソル位置だけを持ち、矩形は `snap` と `sel` が一致している間だけ有効
//! （クリックや移動などで `sel` が変われば自然に無効になる）。

use kazakiri_core::Document;

use super::*;

/// 矩形の行数の上限（巨大な範囲を選んで固まらないように）
const RECT_MAX_LINES: u64 = 200_000;

#[derive(Clone, Copy, Debug)]
pub(super) struct RectSel {
    pub a_line: u64,
    pub a_x: f32,
    pub b_line: u64,
    pub b_x: f32,
    /// 押した位置（セルに合わせる前の x）。ドラッグのたびに、ここからセルを求め直す
    pub ra_x: f32,
    snap: (u64, u64),
}

impl RectSel {
    fn lines(&self) -> (u64, u64) {
        (self.a_line.min(self.b_line), self.a_line.max(self.b_line))
    }
    fn xs(&self) -> (f32, f32) {
        (self.a_x.min(self.b_x), self.a_x.max(self.b_x))
    }
    pub fn has_width(&self) -> bool {
        let (x1, x2) = self.xs();
        x2 - x1 > 0.5
    }
}

impl App {
    /// セル選択モード：行 `line` の、x（行頭からの位置）にあるセルの左端・右端の x
    fn cell_span(&self, doc: &Document, line: u64, x: f32) -> Option<(f32, f32)> {
        let v = self.csv.as_ref()?;
        let l = doc.lines(line, 1).into_iter().next()?;
        let d = Disp::csv(&l, v);
        let idx = unsafe { self.unit_at_x(&d.units[..d.shown], x.max(0.0)) }.unwrap_or(0).min(d.shown);
        let col = d.col_at(idx)?;
        let &(a, b) = d.cols.get(col)?;
        let (a, b) = (a as usize, (b as usize).min(d.units.len()));
        let x0 = unsafe { self.text_width(&d.units[..a]) }.ok()?;
        let x1 = unsafe { self.text_width(&d.units[..b]) }.ok()?;
        Some((x0, x1.max(x0 + self.char_w)))
    }

    /// 有効な矩形選択
    pub(super) fn rect(&self) -> Option<RectSel> {
        self.rect.filter(|r| self.sel == Some(r.snap))
    }

    /// 描画用：この行にかかる矩形の x 範囲
    pub(super) fn rect_span(&self, line: u64) -> Option<(f32, f32, bool)> {
        let r = self.rect()?;
        let (l1, l2) = r.lines();
        let (x1, x2) = r.xs();
        (line >= l1 && line <= l2).then_some((x1, x2, r.has_width()))
    }

    fn rect_point(&self, doc: &Document, line: u64, x: f32) -> Option<(u64, u64)> {
        let last = doc.line_count().0.saturating_sub(1);
        let line = line.min(last);
        let off = self.offset_in_line_at_x(doc, line, x.max(0.0))?;
        Some((line, off))
    }

    fn set_rect_end(&mut self, anchor: Option<(u64, f32)>, line: u64, x: f32) {
        let Some(doc) = self.doc.clone() else { return };
        let Some((line, off)) = self.rect_point(&doc, line, x) else { return };
        let (a_line, ra_x) = anchor.unwrap_or((line, x));
        let (mut a_x, mut b_x) = (ra_x, x);
        // セル選択モード：四角の左右を、セルの境目に合わせる
        if self.cell_mode && self.csv.is_some() {
            if let (Some((ax0, ax1)), Some((bx0, bx1))) = (self.cell_span(&doc, a_line, ra_x), self.cell_span(&doc, line, x)) {
                (a_x, b_x) = if bx0 >= ax0 { (ax0, bx1) } else { (ax1, bx0) };
            }
        }
        self.sel = Some((off, off));
        self.rect = Some(RectSel { a_line, a_x, b_line: line, b_x, ra_x, snap: (off, off) });
        self.goal_x = None;
        self.hit = None;
    }

    /// Alt+クリック：矩形選択を始める
    pub(super) fn rect_start(&mut self, x: f32, y: f32) {
        if self.wrap_on() {
            self.message = "折り返して表示しているときは矩形選択できません（表示 → 折り返して表示 をオフに）".into();
            self.invalidate();
            return;
        }
        let line = self.top + (y.max(0.0) / self.line_h) as u64;
        self.set_rect_end(None, line, x - self.text_left());
        if let Some(e) = &mut self.editor {
            e.break_group();
        }
        self.alt_used = true;
        self.dragging = true;
        unsafe {
            SetCapture(self.hwnd);
        }
        self.update_ime_position();
        self.invalidate();
    }

    /// 矩形選択のドラッグ。矩形選択中でなければ false
    pub(super) fn rect_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(r) = self.rect() else { return false };
        if y < 0.0 {
            self.scroll_by(-1);
        } else if y > self.text_height() {
            self.scroll_by(1);
        }
        let y = y.clamp(0.0, self.text_height() - 1.0);
        let line = self.top + (y / self.line_h) as u64;
        self.set_rect_end(Some((r.a_line, r.ra_x)), line, x - self.text_left());
        self.invalidate();
        true
    }

    /// Alt+Shift+矢印
    pub(super) fn rect_key(&mut self, vk: VIRTUAL_KEY) {
        if self.wrap_on() {
            return;
        }
        self.alt_used = true;
        let Some(doc) = self.doc.clone() else { return };
        let r = match self.rect() {
            Some(r) => r,
            None => {
                let c = self.caret();
                let line = doc.line_of_offset(c).unwrap_or(0);
                let x = self.x_of_offset(&doc, c);
                RectSel { a_line: line, a_x: x, b_line: line, b_x: x, ra_x: x, snap: (c, c) }
            }
        };
        let (mut line, mut x) = (r.b_line, r.b_x);
        match vk {
            VK_UP => line = line.saturating_sub(1),
            VK_DOWN => line += 1,
            VK_LEFT => x = (x - self.char_w).max(0.0),
            VK_RIGHT => x += self.char_w,
            _ => return,
        }
        self.set_rect_end(Some((r.a_line, r.ra_x)), line, x);
        let c = self.caret();
        self.ensure_visible(c);
        self.update_ime_position();
        self.invalidate();
    }

    /// 矩形の各行のバイト範囲（行番号, 開始, 終了）
    fn rect_ranges(&self, doc: &Document, r: &RectSel) -> Vec<(u64, u64, u64)> {
        let (l1, l2) = r.lines();
        let (x1, x2) = r.xs();
        let wide = r.has_width();
        let l2 = l2.min(l1 + RECT_MAX_LINES);
        (l1..=l2)
            .filter_map(|l| {
                let a = self.offset_in_line_at_x(doc, l, x1)?;
                let b = if wide { self.offset_in_line_at_x(doc, l, x2)? } else { a };
                Some((l, a, b.max(a)))
            })
            .collect()
    }

    pub(super) fn rect_copy(&mut self) -> bool {
        let Some(r) = self.rect().filter(|r| r.has_width()) else { return false };
        let Some(doc) = self.doc.clone() else { return false };
        let ranges = self.rect_ranges(&doc, &r);
        let total: u64 = ranges.iter().map(|(_, a, b)| b - a).sum();
        if total > COPY_LIMIT {
            self.message = format!("選択範囲が大きすぎます（{}）", human_size(total));
            self.invalidate();
            return true;
        }
        let parts: Vec<String> = ranges.iter().map(|&(_, a, b)| doc.decode(&doc.read(a, b)).into_owned()).collect();
        let text = parts.join(self.newline_str());
        self.message = match clipboard::set_text(self.hwnd, &text) {
            Ok(()) => format!("矩形をコピーしました（{} 行）", parts.len()),
            Err(e) => format!("コピーに失敗しました: {e}"),
        };
        self.invalidate();
        true
    }

    pub(super) fn rect_cut(&mut self) -> bool {
        if self.rect().is_none_or(|r| !r.has_width()) {
            return false;
        }
        if !self.rect_copy() {
            return false;
        }
        self.rect_edit(vec![String::new()], false);
        true
    }

    /// 矩形の中へ入力（選択があれば置き換え）。矩形選択中でなければ false
    pub(super) fn rect_type(&mut self, s: &str) -> bool {
        if self.rect().is_none() {
            return false;
        }
        let unified = s.replace("\r\n", "\n");
        let body = unified.strip_suffix('\n').unwrap_or(&unified);
        let parts: Vec<String> = if body.contains('\n') { body.split('\n').map(String::from).collect() } else { vec![s.to_string()] };
        self.rect_edit(parts, true);
        true
    }

    /// Backspace / Delete
    pub(super) fn rect_delete(&mut self, forward: bool) -> bool {
        let Some(r) = self.rect() else { return false };
        if r.has_width() {
            self.rect_edit(vec![String::new()], false);
            return true;
        }
        // 幅がなければ各行のカーソルの前（後）の 1 文字
        let Some(doc) = self.doc.clone() else { return true };
        let ranges: Vec<(u64, u64)> = self
            .rect_ranges(&doc, &r)
            .into_iter()
            .filter_map(|(l, a, _)| {
                let (s, e) = doc.line_range(l)?;
                if forward {
                    (a < e).then(|| (a, doc.next_char(a)))
                } else {
                    (a > s).then(|| (doc.prev_char(a), a))
                }
            })
            .collect();
        if ranges.is_empty() {
            return true;
        }
        let x = if forward { r.xs().0 } else { (r.xs().0 - self.char_w).max(0.0) };
        self.apply_rect_edit(r, ranges, vec![Vec::new()], Some(x));
        true
    }

    /// 各行の矩形の範囲を `parts` で置き換える。`parts` が 1 つなら全行同じ、行数と同じなら行ごと
    fn rect_edit(&mut self, parts: Vec<String>, typing: bool) {
        let Some(r) = self.rect() else { return };
        let Some(doc) = self.doc.clone() else { return };
        let mut rows = self.rect_ranges(&doc, &r);
        let per_line = parts.len() > 1 && parts.len() == rows.len();
        if typing && !r.has_width() && !per_line {
            // 幅のない矩形への入力は、桁に届いていない短い行を飛ばす
            let x1 = r.xs().0;
            rows.retain(|&(_, a, _)| self.x_of_offset(&doc, a) >= x1 - self.char_w * 0.5);
        }
        if rows.is_empty() {
            return;
        }
        let texts: Vec<&str> = (0..rows.len()).map(|i| if per_line { parts[i].as_str() } else { parts[0].as_str() }).collect();
        if texts.iter().any(|t| !doc.can_encode(t)) {
            self.message = format!("この文字は {} で表せないため入力できません", doc.encoding().name());
            self.invalidate();
            return;
        }
        let bytes: Vec<Vec<u8>> = texts.iter().map(|t| doc.encode(t)).collect();
        let ranges: Vec<(u64, u64)> = rows.iter().map(|&(_, a, b)| (a, b)).collect();
        drop(doc);
        self.apply_rect_edit(r, ranges, bytes, None);
    }

    fn apply_rect_edit(&mut self, r: RectSel, ranges: Vec<(u64, u64)>, repls: Vec<Vec<u8>>, x_after: Option<f32>) {
        if !self.check_editable() {
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        let first_end = ranges[0].0 + repls[0].len() as u64;
        match self.with_editor(|ed| ed.replace_all(&ranges, &repls, sel)).unwrap() {
            Ok(_) => {
                self.layout_cache.borrow_mut().clear();
                let Some(doc) = self.doc.clone() else { return };
                let (l1, l2) = r.lines();
                // 入力した文字列の後ろへ桁を移す。削除なら左端のまま
                let x = x_after.unwrap_or_else(|| if repls[0].is_empty() { r.xs().0 } else { self.x_of_offset(&doc, first_end) });
                self.set_rect_end(Some((l1, x)), l2, x);
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }
}
