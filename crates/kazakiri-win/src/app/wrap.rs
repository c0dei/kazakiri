//! 折り返し表示。
//!
//! 長い行を画面の幅で表示上の複数の行（折り返し行）に分ける。折り返しの位置は 1 文字ずつの幅
//! （全角は半角の 2 倍、タブはタブ位置まで）から求めるので、比例フォントでは多少ずれる（はみ出た分は切れる）。
//! 折り返し行ごとに文字列の一部を描くので、選択・カーソル・クリックの位置は
//! 行ごとの処理（`unit_at_x` など）をそのまま使える。スクロールは元の行を単位に行う。

use kazakiri_core::csv::char_width;

use super::csv_view::Disp;
use super::*;

/// 1 行を折り返した結果
pub(super) struct Rows {
    pub line: Line,
    pub disp: Disp,
    /// 各折り返し行の先頭（表示文字列の UTF-16 位置）。先頭は 0
    pub starts: Vec<usize>,
}

impl Rows {
    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// 折り返し行 `r` の範囲 [開始, 終了)（最後の行は行末の印も含める）
    pub fn range(&self, r: usize) -> (usize, usize) {
        let a = self.starts[r];
        (a, self.starts.get(r + 1).copied().unwrap_or(self.disp.units.len()))
    }

    pub fn is_last(&self, r: usize) -> bool {
        r + 1 == self.starts.len()
    }
}

impl App {
    pub(super) fn wrap_on(&self) -> bool {
        self.settings.wrap
    }

    /// 本文を折り返す幅
    fn wrap_width(&self) -> f32 {
        let (w, _) = self.client_size();
        (w - self.gutter_width() - 8.0).max(self.char_w * 8.0)
    }

    /// 表示文字列を折り返したときの各行の先頭
    fn row_starts(&self, d: &Disp) -> Vec<usize> {
        let width = self.wrap_width();
        let tab = self.settings.tab_width.max(1);
        let units = &d.units[..d.shown];
        let mut starts = vec![0usize];
        let (mut x, mut col, mut i) = (0f32, 0usize, 0usize);
        while i < units.len() {
            let (c, len) = match char::decode_utf16(units[i..].iter().copied().take(2)).next() {
                Some(Ok(c)) => (c, c.len_utf16()),
                _ => ('\u{FFFD}', 1),
            };
            let cols = if c == '\t' { tab - col % tab } else { char_width(c) };
            let w = cols as f32 * self.char_w;
            if x + w > width && i > *starts.last().unwrap_or(&0) {
                starts.push(i);
                x = 0.0;
                col = 0;
            }
            x += w;
            col += cols;
            i += len;
        }
        starts
    }

    pub(super) fn rows_of(&self, line: Line) -> Rows {
        let disp = self.display(&line);
        let starts = self.row_starts(&disp);
        Rows { line, disp, starts }
    }

    fn rows_of_number(&self, doc: &Document, n: u64) -> Option<Rows> {
        doc.lines(n, 1).into_iter().next().map(|l| self.rows_of(l))
    }

    /// 行 [from, to) が占める折り返し行の数
    fn rows_between(&self, doc: &Document, from: u64, to: u64) -> u64 {
        if to <= from {
            return 0;
        }
        doc.lines(from, (to - from).min(2000) as usize).into_iter().map(|l| self.rows_of(l).count() as u64).sum()
    }

    pub(super) fn rows_between_pub(&self, doc: &Document, from: u64, to: u64) -> u64 {
        self.rows_between(doc, from, to)
    }

    pub(super) fn set_caret_keep_goal(&mut self, pos: u64, extend: bool, x: f32) {
        self.set_caret(pos, extend);
        self.goal_x = Some(x);
    }

    /// 画面の上から `y` の位置にある折り返し行：(行の折り返し結果, 何行目か)
    pub(super) fn row_at_y(&self, doc: &Document, y: f32) -> Option<(Rows, usize)> {
        let mut want = (y.max(0.0) / self.line_h) as usize;
        let mut n = self.top;
        loop {
            let rows = self.rows_of_number(doc, n)?;
            if want < rows.count() {
                return Some((rows, want));
            }
            want -= rows.count();
            n += 1;
        }
    }

    /// 位置 `pos` の (行番号, 折り返し行, 折り返し行の中の x)
    pub(super) fn locate(&self, doc: &Document, pos: u64) -> Option<(Rows, usize, f32)> {
        let n = doc.line_of_offset(pos)?;
        let rows = self.rows_of_number(doc, n)?;
        let src = decode_units(doc, &doc.read(rows.line.start, pos.clamp(rows.line.start, rows.line.end)));
        let idx = rows.disp.to_disp(src).min(rows.disp.shown);
        let r = rows.starts.partition_point(|&s| s <= idx).saturating_sub(1);
        let a = rows.starts[r];
        let x = unsafe { self.text_width(&rows.disp.units[a..idx]) }.unwrap_or((idx - a) as f32 * self.char_w);
        Some((rows, r, x))
    }

    /// 折り返し行 `r` の中で、x 座標に最も近い位置
    pub(super) fn offset_in_row(&self, doc: &Document, rows: &Rows, r: usize, x: f32) -> u64 {
        let (a, b) = rows.range(r);
        let end = b.min(rows.disp.shown);
        let mut idx = a + unsafe { self.unit_at_x(&rows.disp.units[a..end.max(a)], x) }.unwrap_or(0);
        // 行の途中の折り返し行の右端は、次の折り返し行の先頭と同じ位置になるので 1 文字手前に
        if !rows.is_last(r) && idx >= b && idx > a {
            idx = b - 1;
            if (0xDC00..0xE000).contains(&rows.disp.units[idx]) && idx > a {
                idx -= 1;
            }
        }
        self.src_units_to_offset(doc, &rows.line, rows.disp.to_src(idx))
    }

    /// 画面の点に対応するバイト位置（折り返し表示のとき）
    pub(super) fn offset_at_wrapped(&self, doc: &Document, x: f32, y: f32) -> Option<u64> {
        let Some((rows, r)) = self.row_at_y(doc, y) else {
            return doc.progress().done.then(|| doc.len());
        };
        Some(self.offset_in_row(doc, &rows, r, x - self.text_left()))
    }

    /// 画面の上から `y` の位置にある行の番号
    pub(super) fn line_at_y(&self, doc: &Document, y: f32) -> u64 {
        if self.wrap_on() {
            if let Some((rows, _)) = self.row_at_y(doc, y) {
                return rows.line.number;
            }
        }
        self.top + (y.max(0.0) / self.line_h) as u64
    }

    /// 折り返し行を単位にした上下の移動
    pub(super) fn move_rows(&mut self, delta: i64, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some((mut rows, mut r, x_cur)) = self.locate(&doc, c) else { return };
        let x = self.goal_x.unwrap_or(x_cur);
        let total = doc.line_count().0;
        let mut at_edge = false;
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if r + 1 < rows.count() {
                    r += 1;
                } else if rows.line.number + 1 < total {
                    match self.rows_of_number(&doc, rows.line.number + 1) {
                        Some(next) => {
                            rows = next;
                            r = 0;
                        }
                        None => break,
                    }
                } else {
                    at_edge = true;
                    break;
                }
            } else if r > 0 {
                r -= 1;
            } else if rows.line.number > 0 {
                match self.rows_of_number(&doc, rows.line.number - 1) {
                    Some(prev) => {
                        r = prev.count() - 1;
                        rows = prev;
                    }
                    None => break,
                }
            } else {
                at_edge = true;
                break;
            }
        }
        let p = if at_edge && self.locate(&doc, c).is_some_and(|(_, rr, _)| rr == r) {
            // 先頭・最終の折り返し行でさらに上下：行頭・行末へ
            if delta < 0 { rows.line.start } else { rows.line.end }
        } else {
            self.offset_in_row(&doc, &rows, r, x)
        };
        self.set_caret_keep_goal(p, extend, x);
    }

    /// 位置が画面に入るようにスクロールする（縦だけ。折り返し表示のとき）
    pub(super) fn ensure_visible_wrapped(&mut self, doc: &Document, pos: u64) {
        let Some((rows, r, _)) = self.locate(doc, pos) else { return };
        let line = rows.line.number;
        let vis = self.visible_lines() as u64;
        if line < self.top {
            self.top = line;
            return;
        }
        // 先頭から数えて、その折り返し行が画面に入るまで先頭を送る
        while self.top < line && self.rows_between(doc, self.top, line) + r as u64 + 1 > vis {
            self.top += 1;
        }
    }

    /// 末尾の行が画面の下に収まる中で、いちばん下までスクロールしたときの先頭の行
    pub(super) fn max_top_wrapped(&self) -> u64 {
        let Some(doc) = &self.doc else { return 0 };
        let total = doc.line_count().0;
        if total == 0 {
            return 0;
        }
        let vis = self.visible_lines() as u64;
        let (mut top, mut used) = (total, 0u64);
        while top > 0 {
            let Some(rows) = self.rows_of_number(doc, top - 1) else { break };
            if used + rows.count() as u64 > vis {
                break;
            }
            used += rows.count() as u64;
            top -= 1;
        }
        top.min(total - 1)
    }

    /// 折り返し行の一部を描くレイアウト（色を変える範囲も合わせる）
    pub(super) unsafe fn row_layout(&self, d: &Disp, a: usize, b: usize) -> Result<IDWriteTextLayout> {
        let g = self.gfx.as_ref().ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        unsafe {
            let layout = self.dw.CreateTextLayout(&d.units[a..b], &self.format, 1.0e6, self.line_h)?;
            for &(s, e, eff) in &d.effects {
                let (s, e) = ((s as usize).max(a), (e as usize).min(b));
                if e <= s {
                    continue;
                }
                let brush = g.brush_for(eff);
                let _ = layout.SetDrawingEffect(brush, DWRITE_TEXT_RANGE { startPosition: (s - a) as u32, length: (e - s) as u32 });
            }
            Ok(layout)
        }
    }

    pub(super) fn toggle_wrap(&mut self) {
        self.settings.wrap = !self.settings.wrap;
        self.settings.save();
        self.x_off = 0.0;
        self.rect = None;
        self.layout_cache.borrow_mut().clear();
        self.goal_x = None;
        self.clamp_top();
        let c = self.caret();
        self.ensure_visible(c);
        self.message = format!("折り返して表示: {}", on_off(self.settings.wrap));
        self.update_scrollbars();
        self.invalidate();
    }
}
