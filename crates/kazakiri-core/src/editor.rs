//! 編集の履歴（元に戻す・やり直す）と「保存していない変更があるか」の管理。
//!
//! `Document` は編集のたびに新しいものが作られる（中身は共有するので安い）。
//! 履歴には「編集で取り除いたピース」だけを記録し、元に戻すときはそれを差し戻す。
//! ピースは元ファイルや追加文字列を指すだけなので、記録は編集の大きさに比例するだけで済む。

use std::sync::Arc;

use crate::document::{Document, EditError};
use crate::pieces::{Piece, Pieces, Repls};


/// 履歴の最大数
const MAX_UNDO: usize = 100_000;

/// 編集の種類。続けて入力した文字はまとめて 1 回で元に戻せるようにする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// 文字の入力（直前の入力の続きならまとめる）
    Typing,
    /// Backspace / Delete での削除（続けて押したらまとめる）
    Deleting,
    /// それ以外（貼り付け、切り取り、置換など）
    Other,
}

/// 「[a, a+len) を restore に戻す」という記録
struct Entry {
    a: u64,
    len: u64,
    restore: Vec<Piece>,
    /// 戻したあとの版（保存済みかどうかの判定用）
    version: u64,
    /// 戻したあとの選択範囲
    sel: (u64, u64),
}

pub struct Editor {
    /// 編集中だけ一時的に None になる（持ち主を自分だけにして、その場で書き換えるため）
    doc: Option<Arc<Document>>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    version: u64,
    next_version: u64,
    saved_version: u64,
    /// 直前の編集の種類と、そのあとのカーソル位置（まとめる判定用）
    last: Option<(EditKind, u64)>,
    /// いまの状態の選択範囲（やり直しで戻す位置）
    cur_sel: (u64, u64),
}

impl Editor {
    pub fn new(doc: Arc<Document>) -> Editor {
        Editor { doc: Some(doc), undo: Vec::new(), redo: Vec::new(), version: 0, next_version: 1, saved_version: 0, last: None, cur_sel: (0, 0) }
    }

    pub fn doc(&self) -> &Arc<Document> {
        self.doc.as_ref().expect("document")
    }

    /// 編集を適用する（確認済みであること）
    fn edit<R>(&mut self, f: impl FnOnce(&mut Pieces, &crate::pieces::Ctx) -> R) -> R {
        let doc = self.doc.take().expect("document");
        let (new_doc, r) = Document::edit_owned(doc, f);
        self.doc = Some(Arc::new(new_doc));
        r
    }

    /// 編集を伴わない Document の差し替え（追記の読み込み、保存後の開き直しなど）。履歴は消す
    pub fn reset(&mut self, doc: Arc<Document>) {
        *self = Editor::new(doc);
    }

    pub fn is_modified(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 入力のまとめを打ち切る（カーソルを動かしたときなど）
    pub fn break_group(&mut self) {
        self.last = None;
    }

    /// [a, b) を文字列 `s` で置き換える。戻り値は置き換えた後の [a, a + 新しい長さ)
    pub fn replace(&mut self, a: u64, b: u64, s: &str, kind: EditKind, sel_before: (u64, u64)) -> Result<(u64, u64), EditError> {
        let bytes = self.doc().encode(s);
        self.replace_bytes(a, b, bytes, kind, sel_before)
    }

    /// `replace` と同じだが、置き換える内容をファイルの文字コードのバイト列で渡す
    pub fn replace_bytes(&mut self, a: u64, b: u64, bytes: Vec<u8>, kind: EditKind, sel_before: (u64, u64)) -> Result<(u64, u64), EditError> {
        self.doc().check_edit(&[(a, b)])?;
        let removed = self.edit(|t, ctx| t.replace(ctx, a, b, &bytes));
        let n = bytes.len() as u64;
        let end = a + n;

        // 続けて入力・削除した場合は直前の記録にまとめる
        let top = self.undo.last_mut();
        let merged = match (kind, self.last, top) {
            (EditKind::Typing, Some((EditKind::Typing, at)), Some(e)) if a == b && at == a && e.a + e.len == a => {
                e.len += n;
                true
            }
            (EditKind::Deleting, Some((EditKind::Deleting, at)), Some(e)) if n == 0 && e.len == 0 => {
                if b == at && b == e.a {
                    // Backspace：前に広がる
                    let mut r = removed.clone();
                    r.extend(std::mem::take(&mut e.restore));
                    e.restore = r;
                    e.a = a;
                    true
                } else if a == at && a == e.a {
                    // Delete：後ろに広がる
                    e.restore.extend(removed.iter().cloned());
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !merged {
            self.undo.push(Entry { a, len: n, restore: removed, version: self.version, sel: sel_before });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.version = self.next_version;
        self.next_version += 1;
        self.last = Some((kind, if kind == EditKind::Typing { end } else { a }));
        self.cur_sel = (end, end);
        Ok((a, end))
    }

    /// すべて置換。`repls` は一致ごとの置換後のバイト列（1 つだけなら全件同じ）。
    /// 1 回の「元に戻す」で全部戻る。置換した件数を返す
    pub fn replace_all(&mut self, ranges: &[(u64, u64)], repls: &[Vec<u8>], sel_before: (u64, u64)) -> Result<usize, EditError> {
        if ranges.is_empty() {
            return Ok(0);
        }
        let refs = if repls.len() == ranges.len() && ranges.len() > 1 {
            Repls::Each(repls)
        } else {
            Repls::One(repls.first().map_or(&[], |r| r.as_slice()))
        };
        self.doc().check_edit(ranges)?;
        let removed = self.edit(|t, ctx| t.replace_many(ctx, ranges, refs));
        let a = ranges[0].0;
        let old_span = ranges.last().unwrap().1 - a;
        let removed_bytes: u64 = ranges.iter().map(|(s, e)| e - s).sum();
        let added: u64 = match refs {
            Repls::One(b) => b.len() as u64 * ranges.len() as u64,
            Repls::Each(list) => list.iter().map(|r| r.len() as u64).sum(),
        };
        let new_span = old_span - removed_bytes + added;
        self.undo.push(Entry { a, len: new_span, restore: removed, version: self.version, sel: sel_before });
        self.redo.clear();
        self.version = self.next_version;
        self.next_version += 1;
        self.last = None;
        self.cur_sel = (a, a);
        Ok(ranges.len())
    }

    /// 元に戻す。戻した後の選択範囲を返す
    pub fn undo(&mut self) -> Option<(u64, u64)> {
        let e = self.undo.pop()?;
        let back = self.apply(e)?;
        self.redo.push(back);
        Some(self.cur_sel)
    }

    /// やり直す。やり直した後の選択範囲を返す
    pub fn redo(&mut self) -> Option<(u64, u64)> {
        let e = self.redo.pop()?;
        let back = self.apply(e)?;
        self.undo.push(back);
        Some(self.cur_sel)
    }

    /// 記録を適用し、逆向きの記録を返す
    fn apply(&mut self, e: Entry) -> Option<Entry> {
        let restored_len = Pieces::total_len(&e.restore);
        self.doc().check_edit(&[(e.a, e.a + e.len)]).ok()?;
        let removed = self.edit(|t, ctx| t.splice(ctx, e.a, e.a + e.len, e.restore));
        let back = Entry { a: e.a, len: restored_len, restore: removed, version: self.version, sel: self.cur_sel };
        self.version = e.version;
        self.cur_sel = e.sel;
        self.last = None;
        Some(back)
    }

    /// 保存した。開き直した Document に切り替える（元ファイルが変わるので履歴は消す）
    pub fn saved_as(&mut self, doc: Arc<Document>) {
        self.reset(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(bytes: &[u8]) -> Arc<Document> {
        let d = Document::from_bytes(bytes.to_vec());
        while !d.progress().done {
            std::thread::yield_now();
        }
        Arc::new(d)
    }

    fn text(e: &Editor) -> String {
        String::from_utf8(e.doc().read(0, e.doc().len()).into_owned()).unwrap()
    }

    #[test]
    fn typing_is_grouped_and_undoable() {
        let mut e = Editor::new(ready(b"hello\nworld\n"));
        assert!(!e.is_modified());
        let mut pos = 5;
        for c in [" ", "t", "h", "e", "r", "e"] {
            pos = e.replace(pos, pos, c, EditKind::Typing, (pos, pos)).unwrap().1;
        }
        assert_eq!(text(&e), "hello there\nworld\n");
        assert!(e.is_modified());
        // 入力はまとめて 1 回で戻る
        assert_eq!(e.undo(), Some((5, 5)));
        assert_eq!(text(&e), "hello\nworld\n");
        assert!(!e.is_modified());
        assert_eq!(e.redo(), Some((11, 11)));
        assert_eq!(text(&e), "hello there\nworld\n");
        // カーソル移動で区切る
        e.break_group();
        e.replace(0, 0, "X", EditKind::Typing, (0, 0)).unwrap();
        e.replace(1, 1, "Y", EditKind::Typing, (1, 1)).unwrap();
        e.replace(0, 5, "", EditKind::Other, (0, 5)).unwrap();
        assert_eq!(text(&e), "lo there\nworld\n");
        e.undo();
        assert_eq!(text(&e), "XYhello there\nworld\n");
        e.undo();
        assert_eq!(text(&e), "hello there\nworld\n");
        assert!(e.is_modified());
        assert!(e.undo().is_some());
        assert!(!e.is_modified());
        assert!(e.undo().is_none());
    }

    #[test]
    fn replace_all_and_undo_once() {
        let mut e = Editor::new(ready(b"a-b-c\nd-e\n"));
        let ranges = [(1, 2), (3, 4), (7, 8)];
        let n = e.replace_all(&ranges, &[b"==".to_vec()], (0, 0)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(text(&e), "a==b==c\nd==e\n");
        assert_eq!(e.doc().line_count(), (3, true));
        // 改行を含む置換、一致ごとに違う置換
        e.undo();
        assert_eq!(text(&e), "a-b-c\nd-e\n");
        e.replace_all(&ranges, &[b"\n".to_vec(), b"".to_vec(), b"X\nY".to_vec()], (0, 0)).unwrap();
        assert_eq!(text(&e), "a\nbc\ndX\nYe\n");
        assert_eq!(e.doc().line_count(), (5, true));
        assert_eq!(e.doc().lines(3, 1)[0].text, "Ye");
        e.undo();
        assert_eq!(text(&e), "a-b-c\nd-e\n");
        e.redo();
        assert_eq!(text(&e), "a\nbc\ndX\nYe\n");
    }

    #[test]
    fn backspace_is_grouped() {
        let mut e = Editor::new(ready(b"abcdef"));
        for pos in (3..6).rev() {
            e.replace(pos, pos + 1, "", EditKind::Deleting, (pos + 1, pos + 1)).unwrap();
        }
        assert_eq!(text(&e), "abc");
        e.undo();
        assert_eq!(text(&e), "abcdef");
    }
}
