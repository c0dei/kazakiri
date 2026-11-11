//! マルチカーソル（複数の位置に同時に入力する）。
//!
//! Ctrl+クリックでカーソルを増やす。Ctrl+D は「いまの選択（なければカーソルの下の単語）と同じ文字列」を
//! 次の位置に追加する。入力・Backspace/Delete・切り取り・コピー・貼り付けは全カーソルに作用し、
//! 1 回の「元に戻す」でまとめて戻る。←→・Home・End は全カーソルが動く。上下の移動や
//! クリックなど、ほかの操作をすると 1 つのカーソルに戻る。
//!
//! 追加したカーソルは `sel`（いちばん新しいカーソル）と `snap` が一致している間だけ有効
//! （矩形選択と同じ仕組み）。

use super::*;

/// 追加できるカーソルの上限
const MAX_CURSORS: usize = 10_000;

#[derive(Clone, Debug, Default)]
pub(super) struct Multi {
    /// ほかのカーソル（アンカー, 先端）
    pub extras: Vec<(u64, u64)>,
    snap: (u64, u64),
}

impl App {
    /// 有効なほかのカーソル
    pub(super) fn extra_cursors(&self) -> &[(u64, u64)] {
        match (&self.multi, self.sel) {
            (Some(m), Some(sel)) if m.snap == sel => &m.extras,
            _ => &[],
        }
    }

    pub(super) fn multi_active(&self) -> bool {
        !self.extra_cursors().is_empty()
    }

    /// 全カーソルの範囲（小さい順、重なりは 1 つに）
    fn all_ranges(&self) -> Vec<(u64, u64)> {
        let mut v: Vec<(u64, u64)> = self.extra_cursors().iter().copied().chain(self.sel).map(|(a, b)| (a.min(b), a.max(b))).collect();
        v.sort_unstable();
        v.dedup();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(v.len());
        for r in v {
            match out.last_mut() {
                Some(l) if r.0 < l.1 || (r.0 == l.1 && r.0 == r.1 && l.0 == l.1) => l.1 = l.1.max(r.1),
                _ => out.push(r),
            }
        }
        out
    }

    /// 新しいカーソルを足す（それまでの `sel` は「ほかのカーソル」に回る）
    fn push_cursor(&mut self, new: (u64, u64)) {
        let mut extras = self.extra_cursors().to_vec();
        if let Some(old) = self.sel {
            if old != new && !extras.contains(&old) {
                extras.push(old);
            }
        }
        extras.truncate(MAX_CURSORS);
        self.sel = Some(new);
        self.multi = Some(Multi { extras, snap: new });
        self.goal_x = None;
        self.hit = None;
    }

    /// Ctrl+クリック：その位置にカーソルを足す
    pub(super) fn multi_click(&mut self, x: f32, y: f32) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(off) = self.offset_at(&doc, x, y) else { return };
        if let Some(e) = &mut self.editor {
            e.break_group();
        }
        self.push_cursor((off, off));
        self.message = format!("カーソル {} 個（Esc で 1 つに戻す）", self.extra_cursors().len() + 1);
        self.update_ime_position();
        self.invalidate();
    }

    /// Ctrl+D：いまの選択と同じ文字列を、次の位置に追加して選ぶ
    pub(super) fn add_next_match(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        // 何も選んでいなければ、カーソルの下の単語を選ぶ
        let Some((_, b)) = self.selection() else {
            let c = self.caret();
            let line = doc.line_of_offset(c).unwrap_or(0);
            let Some(l) = doc.lines(line, 1).into_iter().next() else { return };
            let chars: Vec<char> = l.text.chars().collect();
            let pre = decode_units(&doc, &doc.read(l.start, c.min(l.end)));
            let before = String::from_utf16_lossy(&l.text.encode_utf16().take(pre).collect::<Vec<_>>()).chars().count();
            let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
            let (mut s, mut e) = (before.min(chars.len()), before.min(chars.len()));
            while s > 0 && is_word(chars[s - 1]) {
                s -= 1;
            }
            while e < chars.len() && is_word(chars[e]) {
                e += 1;
            }
            if s == e {
                self.message = "カーソルの位置に単語がありません".into();
                return self.invalidate();
            }
            let head: String = chars[..s].iter().collect();
            let word: String = chars[s..e].iter().collect();
            let start = l.start + doc.encode(&head).len() as u64;
            self.sel = Some((start, start + doc.encode(&word).len() as u64));
            self.hit = None;
            self.invalidate();
            return;
        };
        let Some(text) = self.selected_text(4096).filter(|t| !t.is_empty() && !t.contains('\n')) else {
            self.message = "Ctrl+D は 1 行の短い文字列を選んでいるときに使えます".into();
            return self.invalidate();
        };
        let taken = self.all_ranges();
        let overlaps = |s: u64, e: u64| taken.iter().any(|&(x, y)| s < y.max(x + 1) && e > x);
        // いちばん後ろの選択の続きから探し、末尾までなければ先頭から
        let from = taken.last().map_or(b, |l| l.1.max(b));
        let found = [from, doc.text_start() as u64].iter().find_map(|&f| {
            let mut pos = f;
            for _ in 0..taken.len() + 1 {
                let (s, e) = doc.find(&text, pos)?;
                if !overlaps(s, e) {
                    return Some((s, e));
                }
                pos = e;
            }
            None
        });
        let Some((s, e)) = found else {
            self.message = "ほかに同じ文字列はありません".into();
            return self.invalidate();
        };
        if let Some(ed) = &mut self.editor {
            ed.break_group();
        }
        self.push_cursor((s, e));
        self.ensure_visible(e);
        self.message = format!("カーソル {} 個（Ctrl+D で次を追加、Esc で 1 つに戻す）", self.extra_cursors().len() + 1);
        self.invalidate();
    }

    pub(super) fn multi_clear(&mut self) {
        self.multi = None;
    }

    /// 各カーソルの範囲を `repls`（1 つなら全部同じ）で置き換える。置換後は全カーソルが挿入した文字列の後ろへ
    fn multi_replace(&mut self, ranges: Vec<(u64, u64)>, repls: Vec<Vec<u8>>) {
        if !self.check_editable() || ranges.is_empty() {
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        let per = repls.len() > 1;
        let lens: Vec<u64> = (0..ranges.len()).map(|i| repls[if per { i } else { 0 }].len() as u64).collect();
        match self.with_editor(|ed| ed.replace_all(&ranges, &repls, sel)).unwrap() {
            Ok(_) => {
                // 置換後の位置：前の置換で増減した分だけずれる
                let mut shift: i64 = 0;
                let mut carets = Vec::with_capacity(ranges.len());
                for (i, &(a, b)) in ranges.iter().enumerate() {
                    let pos = (a as i64 + shift) as u64 + lens[i];
                    carets.push((pos, pos));
                    shift += lens[i] as i64 - (b - a) as i64;
                }
                let primary = carets.pop().unwrap();
                self.sel = Some(primary);
                self.multi = (!carets.is_empty()).then_some(Multi { extras: carets, snap: primary });
                self.goal_x = None;
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    /// 入力・貼り付け。マルチカーソルでなければ false
    pub(super) fn multi_type(&mut self, s: &str) -> bool {
        if !self.multi_active() {
            return false;
        }
        let Some(doc) = self.doc.clone() else { return true };
        if !doc.can_encode(s) {
            self.message = format!("この文字は {} で表せないため入力できません", doc.encoding().name());
            self.invalidate();
            return true;
        }
        let ranges = self.all_ranges();
        // 貼り付けた行数がカーソルの数と同じなら 1 行ずつ
        let unified = s.replace("\r\n", "\n");
        let body = unified.strip_suffix('\n').unwrap_or(&unified);
        let parts: Vec<&str> = body.split('\n').collect();
        let repls: Vec<Vec<u8>> = if parts.len() == ranges.len() && parts.len() > 1 {
            parts.iter().map(|p| doc.encode(p)).collect()
        } else {
            vec![doc.encode(s)]
        };
        drop(doc);
        self.multi_replace(ranges, repls);
        true
    }

    pub(super) fn multi_delete(&mut self, forward: bool) -> bool {
        if !self.multi_active() {
            return false;
        }
        let Some(doc) = self.doc.clone() else { return true };
        let ranges: Vec<(u64, u64)> = self
            .all_ranges()
            .into_iter()
            .filter_map(|(a, b)| {
                if a != b {
                    Some((a, b))
                } else if forward {
                    let n = doc.next_char(b);
                    (n > b).then_some((b, n))
                } else {
                    let p = doc.prev_char(a);
                    (p < a).then_some((p, a))
                }
            })
            .collect();
        drop(doc);
        // 隣り合うカーソルで 1 文字を取り合わないよう、重なりを除く
        let mut clean: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
        for r in ranges {
            if clean.last().is_none_or(|l| r.0 >= l.1) {
                clean.push(r);
            }
        }
        self.multi_replace(clean, vec![Vec::new()]);
        true
    }

    /// コピー（全カーソルの選択を、改行で区切って）
    pub(super) fn multi_copy(&mut self) -> bool {
        if !self.multi_active() {
            return false;
        }
        let Some(doc) = self.doc.clone() else { return true };
        let ranges = self.all_ranges();
        let total: u64 = ranges.iter().map(|(a, b)| b - a).sum();
        if total == 0 {
            return true;
        }
        if total > COPY_LIMIT {
            self.message = format!("選択範囲が大きすぎます（{}）", human_size(total));
            self.invalidate();
            return true;
        }
        let parts: Vec<String> = ranges.iter().filter(|(a, b)| a != b).map(|&(a, b)| doc.decode(&doc.read(a, b)).into_owned()).collect();
        let text = parts.join(self.newline_str());
        self.message = match clipboard::set_text(self.hwnd, &text) {
            Ok(()) => format!("{} か所をコピーしました", parts.len()),
            Err(e) => format!("コピーに失敗しました: {e}"),
        };
        self.invalidate();
        true
    }

    pub(super) fn multi_cut(&mut self) -> bool {
        if !self.multi_active() {
            return false;
        }
        self.multi_copy();
        let ranges: Vec<(u64, u64)> = self.all_ranges().into_iter().filter(|(a, b)| a != b).collect();
        self.multi_replace(ranges, vec![Vec::new()]);
        true
    }

    /// ←→・Home・End を全カーソルに
    pub(super) fn multi_move(&mut self, kind: MultiMove, extend: bool) -> bool {
        if !self.multi_active() {
            return false;
        }
        let Some(doc) = self.doc.clone() else { return true };
        let primary = self.sel;
        let mut moved: Vec<(u64, u64)> = Vec::new();
        let all: Vec<(u64, u64)> = self.extra_cursors().iter().copied().chain(primary).collect();
        for (anchor, head) in all {
            let (lo, hi) = (anchor.min(head), anchor.max(head));
            let target = match kind {
                MultiMove::Left if !extend && lo != hi => lo,
                MultiMove::Right if !extend && lo != hi => hi,
                MultiMove::Left => doc.prev_char(head),
                MultiMove::Right => doc.next_char(head),
                MultiMove::Home | MultiMove::End => {
                    let Some((s, e)) = doc.line_of_offset(head).and_then(|l| doc.line_range(l)) else { continue };
                    if matches!(kind, MultiMove::Home) { s } else { e }
                }
            };
            moved.push((if extend { anchor } else { target }, target));
        }
        let Some(last) = moved.pop() else { return true };
        self.sel = Some(last);
        self.multi = Some(Multi { extras: moved, snap: last });
        self.goal_x = None;
        self.ensure_visible(last.1);
        self.update_ime_position();
        self.invalidate();
        true
    }
}

#[derive(Clone, Copy)]
pub(super) enum MultiMove {
    Left,
    Right,
    Home,
    End,
}
