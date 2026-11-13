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
