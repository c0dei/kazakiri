//! スクロールバーの横の印（概観の帯）。ファイル全体のうち、検索の一致・ブックマーク・いまの位置が、どのあたりにあるかを
//! 本文の右端の細い帯に出す。印の位置は、スクロールバーのつまみと同じく行番号の割合で決める。
//! 検索の一致が多いときは、一致のたびに行を求めるのが重いので、帯の画素ごとの有無を求めておき（`mark_cache`）、
//! 一致の一覧や帯の大きさが変わったときだけ作り直す。

use super::results::ResultKind;
use super::*;

/// 一致の印の保存：(キー, 画素ごとの有無)
pub(super) type MarkCache = Option<((usize, u64, u64, u64, u32), Vec<bool>)>;
/// 帯の幅（論理ピクセル）
const STRIP_W: f32 = 6.0;
/// 一致がこの数より多いときは、行を求めずにバイト位置の割合で置く
const EXACT_LIMIT: usize = 50_000;

/// 帯の画素ごとに印があるか。`rows` は帯の高さ（画素）、`line` は印の行、`total` は総行数
fn bucket(rows: usize, total: u64, line: u64) -> usize {
    ((line as u128 * rows as u128 / total.max(1) as u128) as usize).min(rows.saturating_sub(1))
}

impl App {
    /// 検索の一致の印（画素ごと）。一覧か帯の大きさが変わるまでは、作ったものを使い回す
    fn result_marks(&self, doc: &Document, total: u64, rows: usize) -> Vec<bool> {
        let Some(Results { kind: ResultKind::InDoc { ranges }, .. }) = &self.results else { return Vec::new() };
        if ranges.is_empty() {
            return Vec::new();
        }
        let key = (ranges.len(), ranges[0].0, ranges[ranges.len() - 1].0, doc.len() ^ total, rows as u32);
        if let Some((k, m)) = &*self.mark_cache.borrow() {
            if *k == key {
                return m.clone();
            }
        }
        let mut marks = vec![false; rows];
        let exact = ranges.len() <= EXACT_LIMIT;
        let len = doc.len().max(1);
        for &(s, _) in ranges {
            let line = if exact { doc.line_of_offset(s) } else { Some((s as u128 * total as u128 / len as u128) as u64) };
            if let Some(l) = line {
                marks[bucket(rows, total, l)] = true;
            }
        }
        *self.mark_cache.borrow_mut() = Some((key, marks.clone()));
        marks
    }

    /// 本文の右端（スクロールバーの隣）に印を描く。`w` は本文の幅、`h` は本文の高さ
    pub(super) unsafe fn draw_scroll_marks(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, h: f32) {
        let Some(doc) = &self.doc else { return };
        let total = doc.line_count().0;
        let rows = h.floor() as usize;
        if total == 0 || rows < 8 || self.bookmarks.is_empty() && self.results.is_none() {
            return;
        }
        let x0 = w - STRIP_W;
        let draw = |marks: &[bool], inset: f32, color: &D2D1_COLOR_F| {
            brush.SetColor(color);
            let mut i = 0;
            while i < marks.len() {
                if marks[i] {
                    // 連続する画素は、1 つの長方形にまとめる（高さは 2 画素以上）
                    let start = i;
                    while i < marks.len() && marks[i] {
                        i += 1;
                    }
                    unsafe {
                        rt.FillRectangle(&rect(x0 + inset, start as f32, w - inset, (i.max(start + 2)) as f32), brush);
                    }
                } else {
                    i += 1;
                }
            }
        };
        let results = self.result_marks(doc, total, rows);
        draw(&results, 0.0, &rgb(0xE6, 0xB4, 0x2E));
        if !self.bookmarks.is_empty() {
            let mut marks = vec![false; rows];
            for &l in &self.bookmarks {
                marks[bucket(rows, total, l)] = true;
            }
            draw(&marks, 1.0, &theme::accent(self.settings.dark));
        }
        // いまの位置（カーソルのある行）
        if let Some(l) = doc.line_of_offset(self.caret()) {
            let mut marks = vec![false; rows];
            marks[bucket(rows, total, l)] = true;
            draw(&marks, 2.0, &self.theme.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_maps_lines_to_rows() {
        assert_eq!(bucket(100, 1000, 0), 0);
        assert_eq!(bucket(100, 1000, 500), 50);
        assert_eq!(bucket(100, 1000, 999), 99);
        // 行数が少ないときも範囲の外に出ない
        assert_eq!(bucket(100, 3, 2), 66);
        assert_eq!(bucket(100, 0, 5), 99);
    }
}
