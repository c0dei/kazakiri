//! ブックマーク（行に付ける印）。Ctrl+F2 で設定／解除、F2 で次へ、Shift+F2 で前へ、Ctrl+Shift+F2 で全解除。
//!
//! 行番号で持つ。行を足したり消したりしたときは、カーソルの位置を目安にずらす（消えた行の印は外す）。
//! 印はタブごと。左の行番号の欄に色の帯で出す。

use std::collections::BTreeSet;

use super::results::{ResultKind, Results};
use super::*;

impl App {
    fn caret_line(&self) -> Option<u64> {
        self.doc.as_ref()?.line_of_offset(self.caret())
    }

    pub(super) fn toggle_bookmark(&mut self) {
        let Some(l) = self.caret_line() else { return };
        if self.bookmarks.is_empty() {
            self.bm_lines = self.doc.as_ref().map_or(0, |d| d.line_count().0);
        }
        if !self.bookmarks.remove(&l) {
            self.bookmarks.insert(l);
            self.message = format!("ブックマークを設定しました（{} 件）", self.bookmarks.len());
        } else {
            self.message = format!("ブックマークを解除しました（{} 件）", self.bookmarks.len());
        }
        self.invalidate();
    }

    pub(super) fn clear_bookmarks(&mut self) {
        let n = self.bookmarks.len();
        self.bookmarks.clear();
        self.message = format!("ブックマークをすべて解除しました（{n} 件）");
        self.invalidate();
    }

    /// 次（前）のブックマークへ。端まで行ったら反対側から
    pub(super) fn goto_bookmark(&mut self, forward: bool) {
        if self.bookmarks.is_empty() {
            self.message = "ブックマークがありません（Ctrl+F2 で設定）".into();
            return self.invalidate();
        }
        let cur = self.caret_line().unwrap_or(0);
        let target = if forward {
            self.bookmarks.range(cur + 1..).next().or_else(|| self.bookmarks.iter().next())
        } else {
            self.bookmarks.range(..cur).next_back().or_else(|| self.bookmarks.iter().next_back())
        };
        let Some(&line) = target else { return };
        let Some((s, _)) = self.doc.as_ref().and_then(|d| d.line_range(line)) else {
            self.message = format!("{} 行目はまだ索引にありません", group(line + 1));
            return self.invalidate();
        };
        self.sel = Some((s, s));
        self.goal_x = None;
        let vis = self.visible_lines() as u64;
        if line < self.top || line + 1 >= self.top + vis {
            self.scroll_to(line.saturating_sub(vis / 3));
        }
        self.message = format!("ブックマーク {} 行目", group(line + 1));
        self.invalidate();
    }

    /// ブックマークの一覧を、結果パネルに出す
    pub(super) fn list_bookmarks(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        let ranges: Vec<(u64, u64)> = self.bookmarks.iter().filter_map(|&l| doc.line_range(l)).collect();
        if ranges.is_empty() {
            self.message = "ブックマークがありません（Ctrl+F2 で設定）".into();
            return self.invalidate();
        }
        let summary = format!("ブックマーク {} 件", ranges.len());
        self.results = Some(Results { kind: ResultKind::InDoc { ranges }, sel: 0, top: 0, summary });
        self.invalidate();
    }

    /// 編集で行数が変わったら、印の行番号をずらす（カーソルを編集位置の目安にする）
    pub(super) fn adjust_bookmarks(&mut self) {
        if self.bookmarks.is_empty() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else { return };
        let now = doc.line_count().0;
        let delta = now as i64 - self.bm_lines as i64;
        self.bm_lines = now;
        if delta == 0 {
            return;
        }
        let Some(p) = doc.line_of_offset(self.caret()) else { return };
        let moved: BTreeSet<u64> = if delta > 0 {
            // 行が増えた：増えた分の手前（カーソルの行 − 増えた行数）より後ろを下げる
            // 編集した行（カーソルの行 − 増えた行数）の後ろを下げる。行頭で改行を入れたときは、その行の文字も下へ動くので、その行も下げる
            let edit_line = p.saturating_sub(delta as u64);
            let first = if self.bm_col0 { edit_line } else { edit_line + 1 };
            self.bookmarks.iter().map(|&l| if l >= first { l + delta as u64 } else { l }).collect()
        } else {
            // 行が減った：カーソルの行の後ろの、消えた行数分の印は外し、残りを上げる
            let gone = (-delta) as u64;
            self.bookmarks.iter().filter(|&&l| !(l > p && l <= p + gone)).map(|&l| if l > p + gone { l - gone } else { l }).collect()
        };
        self.bookmarks = moved;
    }
}

impl App {
    /// ブックマークした行の範囲（行の先頭から、改行を含めて次の行の先頭まで）
    fn bookmark_ranges(&self, doc: &Document) -> Vec<(u64, u64)> {
        self.bookmarks
            .iter()
            .filter_map(|&l| {
                let (s, _) = doc.line_range(l)?;
                let e = doc.line_range(l + 1).map_or(doc.len(), |(ns, _)| ns);
                Some((s, e))
            })
            .collect()
    }

    /// ブックマークした行のコピー（`delete` なら、そのあと削除。`cut` はコピーも兼ねる）
    pub(super) fn bookmark_lines(&mut self, copy: bool, delete: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let ranges = self.bookmark_ranges(&doc);
        if ranges.is_empty() {
            self.message = "ブックマークがありません（Ctrl+F2 で設定）".into();
            return self.invalidate();
        }
        let total: u64 = ranges.iter().map(|(a, b)| b - a).sum();
        if (copy || !delete) && total > COPY_LIMIT {
            self.message = format!("ブックマークした行が大きすぎます（{}）", human_size(total));
            return self.invalidate();
        }
        if copy || !delete {
            let mut text = String::new();
            for &(a, b) in &ranges {
                text.push_str(&doc.decode(&doc.read(a, b)));
            }
            if !text.ends_with(['\n', '\r']) {
                text.push_str(self.newline_str());
            }
            if let Err(e) = clipboard::set_text(self.hwnd, &text) {
                self.message = format!("コピーに失敗しました: {e}");
                return self.invalidate();
            }
        }
        if !delete {
            self.message = format!("ブックマークした {} 行をコピーしました", group(ranges.len() as u64));
            return self.invalidate();
        }
        if !self.check_editable() {
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        let n = ranges.len();
        drop(doc);
        match self.with_editor(|ed| ed.replace_all(&ranges, &[Vec::new()], sel)).unwrap() {
            Ok(_) => {
                self.bookmarks.clear();
                let at = ranges[0].0;
                self.sel = Some((at, at));
                self.goal_x = None;
                self.message = format!("ブックマークした {} 行を{}しました", group(n as u64), if copy { "切り取り" } else { "削除" });
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    /// ブックマークした行だけを、新しいタブに出す
    pub(super) fn extract_bookmarks(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        let ranges = self.bookmark_ranges(&doc);
        if ranges.is_empty() {
            self.message = "ブックマークがありません（Ctrl+F2 で設定）".into();
            return self.invalidate();
        }
        let mut text = String::new();
        for &(a, b) in &ranges {
            text.push_str(&doc.decode(&doc.read(a, b)));
            if !text.ends_with(['\n', '\r']) {
                text.push_str(self.newline_str());
            }
        }
        let n = ranges.len();
        self.new_tab();
        self.type_text(&text, EditKind::Other);
        self.sel = Some((0, 0));
        self.top = 0;
        self.message = format!("ブックマークした {} 行を新しいタブに出しました", group(n as u64));
        self.invalidate();
    }

    /// ブックマークの有無を、すべての行で反転する
    pub(super) fn invert_bookmarks(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        let (n, exact) = doc.line_count();
        if !exact || n > 5_000_000 {
            self.message = "行数が多い（または索引の作成中）ため、反転できません（500 万行まで）".into();
            return self.invalidate();
        }
        self.bm_lines = n;
        // 最後の空行（ファイル末尾の改行のあと）には付けない
        let last = if doc.line_range(n - 1).is_some_and(|(s, e)| s == e) { n - 1 } else { n };
        self.bookmarks = (0..last).filter(|l| !self.bookmarks.contains(l)).collect();
        self.message = format!("ブックマークを反転しました（{} 件）", group(self.bookmarks.len() as u64));
        self.invalidate();
    }
}
