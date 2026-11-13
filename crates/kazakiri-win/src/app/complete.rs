//! 単語補完（Ctrl+Space）とクリップボード履歴からの貼り付け。
//!
//! 補完は、カーソルの前にある単語の頭の部分から、ファイルの中（カーソルの前後 256KB）の単語を探して
//! 近い順に入れる。続けて押すと、次の候補に置き換わる。

use kazakiri_core::textops;
use kazakiri_core::EditKind;

use super::*;

/// 補完の範囲（カーソルの前後）
const WINDOW: u64 = 256 << 10;

/// 補完の途中の状態
pub(super) struct Dab {
    /// 置き換える範囲の始まり（単語の頭）
    start: u64,
    /// いま入れている候補の終わり（ここにカーソルがある間だけ続きとみなす）
    end: u64,
    cands: Vec<String>,
    next: usize,
}

impl App {
    pub(super) fn complete_word(&mut self) {
        if !self.check_editable() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let caret = self.caret();
        let sel = self.sel.unwrap_or((0, 0));
        // 続けて押した：次の候補へ
        if let Some(d) = self.dabbrev.take().filter(|d| d.end == caret && sel.0 == sel.1) {
            let word = d.cands[d.next % d.cands.len()].clone();
            self.put_completion(&doc, d.start, caret, d, &word);
            return;
        }
        // カーソルの前の単語の頭の部分
        let from = caret.saturating_sub(256);
        let before = doc.decode(&doc.read(from, caret)).into_owned();
        let prefix: String = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
        if prefix.is_empty() {
            self.message = "補完する単語の頭の部分がありません".into();
            return self.invalidate();
        }
        let start = caret - doc.encode(&prefix).len() as u64;
        let (w0, w1) = (caret.saturating_sub(WINDOW), (caret + WINDOW).min(doc.len()));
        let window = doc.decode(&doc.read(w0, w1)).into_owned();
        let caret_chars = doc.decode(&doc.read(w0, caret)).chars().count();
        let cands = textops::complete_candidates(&window, caret_chars, &prefix);
        if cands.is_empty() {
            self.message = format!("「{prefix}」で始まる単語はありません");
            return self.invalidate();
        }
        let word = cands[0].clone();
        let d = Dab { start, end: caret, cands, next: 0 };
        self.put_completion(&doc, start, caret, d, &word);
    }

    fn put_completion(&mut self, doc: &Arc<Document>, start: u64, end: u64, mut d: Dab, word: &str) {
        if !doc.can_encode(word) {
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        let total = d.cands.len();
        d.next += 1;
        let cur = d.next;
        let w = word.to_string();
        if let Some(Ok((_, e))) = self.with_editor(|ed| ed.replace(start, end, &w, EditKind::Other, sel)) {
            d.end = e;
            self.sel = Some((e, e));
            self.goal_x = None;
            self.message = format!("補完: {w}（{} / {total}。続けて Ctrl+Space で次の候補）", (cur - 1) % total + 1);
            self.dabbrev = Some(d);
            self.after_edit();
        }
    }

    /// クリップボード履歴の `i` 番目を貼り付ける
    pub(super) fn paste_history(&mut self, i: usize) {
        let Some(text) = clipboard::history().get(i).cloned() else { return };
        let unified = text.replace("\r\n", "\n").replace('\r', "\n");
        let nl = self.newline_str();
        let s = if nl == "\n" { unified } else { unified.replace('\n', nl) };
        self.type_text(&s, EditKind::Other);
    }
}
