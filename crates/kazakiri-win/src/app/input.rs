//! キーボード・マウスの入力（ボタンの押下、キー、文字、ドラッグ）。

use super::*;

impl App {
    /// ツールバー・機能ボタン・自前のメニューバー・ステータスバーを押した。該当すれば Some（中身は実行後の続き）
    pub(super) unsafe fn chrome_press(&mut self, x: f32, y: f32) -> Option<Option<Deferred>> {
        // ツールバー・機能ボタン
        if let Some(id) = self.hover_at(x, y).and_then(|h| self.hover_command(h)).map(|(id, _)| id) {
            self.hover = None;
            self.hide_tip();
            return Some(self.command(id));
        }
        if y < -self.tab_h() {
            return Some(None);
        }
        // ステータスバーの文字コード・改行コード・行桁・タブ幅・挿入／上書き
        if y >= 0.0 && y >= self.client_size().1 - self.status_base_h() {
            return Some(match self.status_hit(x) {
                Some(2) => self.command(menu::ID_GOTO),
                Some(4) => self.command(menu::ID_OVERWRITE),
                Some(k) => Some(Deferred::StatusMenu(k)),
                None => None,
            });
        }
        None
    }

    pub(super) unsafe fn on_key(&mut self, vk: VIRTUAL_KEY) -> Option<Deferred> {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if self.md_key(vk, ctrl) || self.diff_key(vk, ctrl, shift) {
            return None;
        }
        // キー割り当て（keys.ini）。入力欄が開いているときはタブの切り替えだけ効く
        if let Some(id) = self.keymap.lookup(ctrl, shift, vk.0) {
            let bar_ok = self.is_find_bar() && matches!(id, menu::ID_FIND_NEXT | menu::ID_FIND_PREV | menu::ID_FIND | menu::ID_REPLACE);
            if self.prompt.is_none() || bar_ok || matches!(id, menu::ID_NEXT_TAB | menu::ID_PREV_TAB) {
                return unsafe { self.command(id) };
            }
        }
        if self.prompt.is_some() {
            match vk {
                VK_ESCAPE => {
                    self.prompt = None;
                    self.comp = None;
                    self.invalidate();
                }
                // 検索・置換バー：Enter で次を検索（置換欄では置換して次へ）、Shift+Enter で前を検索、Tab で欄を移る
                VK_RETURN if self.is_find_bar() => {
                    let act = if self.prompt_focus == 1 && self.prompt == Some(Prompt::Replace) {
                        FindAct::ReplOne
                    } else if shift {
                        FindAct::Prev
                    } else {
                        FindAct::Next
                    };
                    self.find_bar_do(act);
                }
                VK_TAB if self.prompt == Some(Prompt::Replace) => {
                    self.prompt_focus = if shift { 0 } else { 1 - self.prompt_focus.min(1) };
                    self.comp = None;
                    self.invalidate();
                }
                VK_RETURN => return self.submit_prompt(),
                VK_BACK => {
                    self.prompt_buf().pop();
                    self.find_bar_changed();
                }
                _ if ctrl && vk == VK_V => {
                    if let Some(s) = clipboard_text() {
                        let line = s.lines().next().unwrap_or("").to_string();
                        self.prompt_buf().push_str(&line);
                        self.find_bar_changed();
                    }
                }
                _ => {}
            }
            return None;
        }
        match vk {
            VK_ESCAPE => {
                if self.search.is_some() {
                    self.cancel_search();
                    self.message = "検索を中止しました".into();
                } else if let Some(job) = self.rjob.take() {
                    job.cancel();
                    self.message = "中止しました".into();
                    self.update_scrollbars();
                } else if self.selection().is_none() && self.results.is_some() {
                    self.close_results();
                } else if let Some(job) = &self.saving {
                    job.cancel.store(true, Ordering::Relaxed);
                } else {
                    let c = self.caret();
                    self.sel = Some((c, c));
                    self.rect = None;
                    self.multi_clear();
                    self.hit = None;
                }
                self.invalidate();
            }
            // カーソル移動（Shift で選択、Ctrl+↑↓ は画面だけスクロール）
            VK_UP if ctrl => self.scroll_by(-1),
            VK_DOWN if ctrl => self.scroll_by(1),
            VK_UP => self.move_vertical(-1, shift),
            VK_DOWN => self.move_vertical(1, shift),
            VK_LEFT if ctrl => self.move_word(false, shift),
            VK_RIGHT if ctrl => self.move_word(true, shift),
            VK_LEFT => self.move_horizontal(false, shift),
            VK_RIGHT => self.move_horizontal(true, shift),
            VK_PRIOR => self.move_page(false, shift),
            VK_NEXT => self.move_page(true, shift),
            VK_HOME if ctrl => self.move_doc_edge(false, shift),
            VK_END if ctrl => self.move_doc_edge(true, shift),
            VK_HOME => self.move_line_edge(false, shift),
            VK_END => self.move_line_edge(true, shift),
            // 編集
            VK_INSERT if !ctrl && !shift => {
                self.overwrite = !self.overwrite;
                self.message = format!("{}モード", if self.overwrite { "上書き" } else { "挿入" });
                self.invalidate();
            }
            VK_RETURN => self.insert_newline(),
            VK_BACK => self.delete_char(false),
            VK_DELETE => self.delete_char(true),
            VK_TAB if !ctrl && !self.tab_key(shift) => self.type_text("\t", EditKind::Typing),
            VK_TAB if !ctrl => {}
            _ => {}
        }
        None
    }

    pub(super) fn on_char(&mut self, ch: u32) {
        if self.diffview.is_some() || self.md_mode() == Some(mdview::MdMode::Design) {
            return;
        }
        // 制御文字（Enter, BS, Tab, Esc, Ctrl+?）は WM_KEYDOWN 側で処理済み
        if ch < 0x20 || ch == 0x7F {
            return;
        }
        // 絵文字などはサロゲートペアで 2 回に分けて届く
        let c = match ch {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(ch as u16);
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(hi) = self.high_surrogate.take() else { return };
                char::decode_utf16([hi, ch as u16]).next().and_then(|r| r.ok())
            }
            _ => char::from_u32(ch),
        };
        let Some(c) = c else { return };
        if self.prompt.is_some() {
            self.prompt_buf().push(c);
            self.find_bar_changed();
        } else {
            self.type_text(c.encode_utf8(&mut [0; 4]), EditKind::Typing);
        }
    }

    pub(super) fn mouse_pos(&self, lparam: LPARAM) -> (f32, f32) {
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let x = loword(lparam.0 as usize) as i16 as f32 / scale;
        // 本文の座標（タブバーの下を 0 とする）。タブバーの上なら負になる
        let y = hiword(lparam.0 as usize) as i16 as f32 / scale - self.chrome_h();
        (x, y)
    }

    pub(super) fn on_mouse_down(&mut self, x: f32, y: f32, shift: bool) {
        let Some(doc) = self.doc.clone() else { return };
        if y >= self.text_height() {
            return;
        }
        // 行番号欄のクリックは行全体を選択
        if x < self.gutter_width() {
            let line = self.line_at_y(&doc, y);
            if let Some((s, _)) = doc.line_range(line) {
                let next = doc.line_range(line + 1).map_or(doc.len(), |(n, _)| n);
                self.sel = Some((s, next));
                self.invalidate();
            }
            return;
        }
        let Some(off) = self.offset_at(&doc, x, y) else { return };
        self.sel = match (shift, self.sel) {
            (true, Some((a, _))) => Some((a, off)),
            _ => Some((off, off)),
        };
        self.dragging = true;
        self.goal_x = None;
        self.hit = None;
        if let Some(e) = &mut self.editor {
            e.break_group();
        }
        unsafe {
            SetCapture(self.hwnd);
        }
        self.update_ime_position();
        self.invalidate();
    }

    pub(super) fn on_drag(&mut self, x: f32, y: f32) {
        if self.rect_drag(x, y) {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        // 画面の外へドラッグしたら自動スクロール
        if y < 0.0 {
            self.scroll_by(-1);
        } else if y > self.text_height() {
            self.scroll_by(1);
        }
        let y = y.clamp(0.0, self.text_height() - 1.0);
        if let (Some(off), Some((a, _))) = (self.offset_at(&doc, x.max(self.gutter_width()), y), self.sel) {
            self.sel = Some((a, off));
            self.invalidate();
        }
    }

    pub(super) fn select_word_at(&mut self, x: f32, y: f32) {
        let Some(doc) = self.doc.clone() else { return };
        if x < self.gutter_width() || y >= self.text_height() {
            return;
        }
        let line_no = self.line_at_y(&doc, y);
        let Some(line) = self.lines_view(&doc, line_no, 1).into_iter().next() else { return };
        let Some(off) = self.offset_at(&doc, x, y) else { return };
        let chars: Vec<char> = line.text.chars().collect();
        let prefix_chars = doc.decode(&doc.read(line.start, off)).chars().count();
        let i = prefix_chars.min(chars.len().saturating_sub(1));
        if chars.is_empty() {
            return;
        }
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 3 };
        let k = class(chars[i]);
        let mut a = i;
        while a > 0 && class(chars[a - 1]) == k && k != 3 {
            a -= 1;
        }
        let mut b = i + 1;
        while b < chars.len() && class(chars[b]) == k && k != 3 {
            b += 1;
        }
        let s: String = chars[..a].iter().collect();
        let m: String = chars[a..b].iter().collect();
        let start = line.start + doc.encode(&s).len() as u64;
        let end = start + doc.encode(&m).len() as u64;
        self.sel = Some((start, end.min(line.end)));
        self.dragging = false;
        self.invalidate();
    }
}
