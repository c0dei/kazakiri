//! 編集操作：カーソル移動、入力、削除、貼り付け、元に戻す、保存、IME。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use encoding_rs::Encoding;
use kazakiri_core::{commit_save, Document, EditKind, Editor};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::LOGFONTW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::*;

use super::*;

/// 保存スレッドからの完了通知
pub(super) const WM_SAVE_DONE: u32 = WM_APP + 2;
/// 保存後に保留していた操作を実行する
pub(super) const WM_RUN_PENDING: u32 = WM_APP + 3;

/// 実行中の保存
pub(super) struct SaveJob {
    pub written: Arc<AtomicU64>,
    pub cancel: Arc<AtomicBool>,
    pub total: u64,
    pub target: PathBuf,
}

pub(super) struct SaveDone {
    result: std::result::Result<PathBuf, String>,
    target: PathBuf,
    /// 変換して保存したときの文字コード（保存後に開き直すときに使う）
    converted: Option<&'static Encoding>,
}

/// 「保存しますか？」のあとに行う操作
#[derive(Clone)]
pub(super) enum Pending {
    OpenDialog,
    OpenPath(PathBuf),
    Reopen(Option<&'static Encoding>),
    /// ファイルを開いて、その中の位置へ（GREP の結果から）。行, 開始, 終了
    OpenAt(PathBuf, u64, u64, u64),
    /// 表示中のタブを閉じる
    CloseTab,
    /// アプリを終了する（未保存のタブを順に確認する）
    Close,
}

impl App {
    // ---- 状態 ----

    pub(super) fn is_modified(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.is_modified())
    }

    /// カーソル位置（選択の先端）
    pub(super) fn caret(&self) -> u64 {
        self.sel.map_or(0, |(_, h)| h)
    }

    fn set_caret(&mut self, pos: u64, extend: bool) {
        // 次の検索はカーソル位置から
        self.hit = None;
        self.sel = match (extend, self.sel) {
            (true, Some((a, _))) => Some((a, pos)),
            _ => Some((pos, pos)),
        };
        if let Some(e) = &mut self.editor {
            e.break_group();
        }
        self.ensure_visible(pos);
        self.update_ime_position();
        self.invalidate();
    }

    /// 編集できる状態か。できなければ理由を表示して false
    pub(super) fn check_editable(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let msg = if self.saving.is_some() {
            Some("保存中は編集できません".to_string())
        } else if !doc.can_edit() {
            Some("索引の作成中は編集できません（もう少しお待ちください）".to_string())
        } else {
            None
        };
        if let Some(m) = msg {
            self.message = m;
            self.invalidate();
            return false;
        }
        if self.follow {
            self.set_follow(false);
        }
        true
    }

    /// 編集後の共通処理
    /// エディタを操作する。表示用の Document の参照をいったん手放すので、
    /// 編集はピース列を複製せずにその場で行える
    pub(super) fn with_editor<R>(&mut self, f: impl FnOnce(&mut Editor) -> R) -> Option<R> {
        self.doc = None;
        let r = self.editor.as_mut().map(f);
        self.doc = self.editor.as_ref().map(|e| e.doc().clone());
        r
    }

    pub(super) fn after_edit(&mut self) {
        let Some(ed) = &self.editor else { return };
        self.doc = Some(ed.doc().clone());
        self.hit = None;
        // ファイル内の検索結果は位置がずれるので閉じる（GREP の結果は残す）
        if matches!(self.results.as_ref().map(|r| &r.kind), Some(super::results::ResultKind::InDoc { .. })) {
            self.results = None;
        }
        self.layout_cache.borrow_mut().clear();
        self.update_title();
        self.update_scrollbars();
        self.ensure_visible(self.caret());
        self.update_ime_position();
        self.invalidate();
    }

    pub(super) fn update_title(&self) {
        let name = self
            .doc
            .as_ref()
            .and_then(|d| d.path().map(|p| p.display().to_string()))
            .unwrap_or_else(|| "（無題）".into());
        let mark = if self.is_modified() { "* " } else { "" };
        let title = wide(&format!("{mark}{name} - {APP_NAME}"));
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(title.as_ptr()));
        }
    }

    /// 改行として挿入する文字列（ファイルの改行コードに合わせる。新規・改行なしは CRLF）
    pub(super) fn newline_str(&self) -> &'static str {
        match self.doc.as_ref().map(|d| (d.newline(), d.newline_name(), d.line_count().0)) {
            Some((kazakiri_core::Newline::Cr, _, _)) => "\r",
            Some((_, "LF", n)) if n > 1 => "\n",
            _ => "\r\n",
        }
    }

    // ---- 入力・削除 ----

    /// 選択範囲（なければカーソル位置）を文字列で置き換える
    pub(super) fn type_text(&mut self, s: &str, kind: EditKind) {
        if !self.check_editable() {
            return;
        }
        if self.rect_type(s) {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        if !doc.can_encode(s) {
            self.message = format!("この文字は {} で表せないため入力できません", doc.encoding().name());
            self.invalidate();
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
        let kind = if a != b && kind == EditKind::Typing { EditKind::Other } else { kind };
        drop(doc);
        match self.with_editor(|ed| ed.replace(a, b, s, kind, sel)).unwrap() {
            Ok((_, end)) => {
                self.sel = Some((end, end));
                self.goal_x = None;
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    pub(super) fn insert_newline(&mut self) {
        let nl = self.newline_str();
        self.type_text(nl, EditKind::Other);
    }

    /// Backspace（`forward` なら Delete）
    pub(super) fn delete_char(&mut self, forward: bool) {
        if !self.check_editable() {
            return;
        }
        if self.rect_delete(forward) {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let sel = self.sel.unwrap_or((0, 0));
        let (a, b) = if sel.0 != sel.1 {
            (sel.0.min(sel.1), sel.0.max(sel.1))
        } else if forward {
            (sel.1, doc.next_char(sel.1))
        } else {
            (doc.prev_char(sel.1), sel.1)
        };
        if a == b {
            return;
        }
        let kind = if sel.0 != sel.1 { EditKind::Other } else { EditKind::Deleting };
        drop(doc);
        match self.with_editor(|ed| ed.replace(a, b, "", kind, sel)).unwrap() {
            Ok(_) => {
                self.sel = Some((a, a));
                self.goal_x = None;
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    pub(super) fn cut(&mut self) {
        if self.rect_cut() {
            return;
        }
        if self.selection().is_none() || !self.check_editable() {
            return;
        }
        self.copy();
        let sel = self.sel.unwrap();
        let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
        if b - a > COPY_LIMIT {
            return; // コピーできなかった大きさは切り取らない
        }
        if let Some(Ok(_)) = self.with_editor(|ed| ed.replace(a, b, "", EditKind::Other, sel)) {
            self.sel = Some((a, a));
            self.after_edit();
        }
    }

    pub(super) fn paste(&mut self) {
        let Some(text) = clipboard_text_full() else { return };
        // 改行をファイルの改行コードにそろえる
        let unified = text.replace("\r\n", "\n").replace('\r', "\n");
        let nl = self.newline_str();
        let s = if nl == "\n" { unified } else { unified.replace('\n', nl) };
        self.type_text(&s, EditKind::Other);
    }

    pub(super) fn undo(&mut self, redo: bool) {
        if self.saving.is_some() {
            return;
        }
        let r = self.with_editor(|ed| if redo { ed.redo() } else { ed.undo() }).flatten();
        match r {
            Some(sel) => {
                self.sel = Some(sel);
                self.after_edit();
            }
            None => {
                self.message = if redo { "やり直す操作はありません" } else { "元に戻す操作はありません" }.into();
                self.invalidate();
            }
        }
    }

    // ---- カーソル移動 ----

    pub(super) fn move_horizontal(&mut self, forward: bool, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        // 選択中に矢印だけ押したら、選択の端へ
        if let (false, Some((a, b))) = (extend, self.selection()) {
            self.goal_x = None;
            return self.set_caret(if forward { b } else { a }, false);
        }
        let c = self.caret();
        let p = if forward { doc.next_char(c) } else { doc.prev_char(c) };
        self.goal_x = None;
        self.set_caret(p, extend);
    }

    /// 単語単位の移動（Ctrl+←→）
    pub(super) fn move_word(&mut self, forward: bool, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let class = |p: u64| -> u8 {
            let bytes = doc.read(p, doc.next_char(p));
            match doc.decode(&bytes).chars().next() {
                Some(c) if c.is_alphanumeric() || c == '_' => 1,
                Some(c) if c.is_whitespace() => 0,
                Some(_) => 2,
                None => 3,
            }
        };
        let mut p = self.caret();
        let len = doc.len();
        if forward {
            let k = class(p);
            while p < len && class(p) == k && k != 3 {
                p = doc.next_char(p);
            }
            while p < len && class(p) == 0 {
                p = doc.next_char(p);
            }
        } else {
            let floor = doc.text_start() as u64;
            while p > floor && class(doc.prev_char(p)) == 0 {
                p = doc.prev_char(p);
            }
            if p > floor {
                let k = class(doc.prev_char(p));
                while p > floor && class(doc.prev_char(p)) == k {
                    p = doc.prev_char(p);
                }
            }
        }
        self.goal_x = None;
        self.set_caret(p, extend);
    }

    pub(super) fn move_vertical(&mut self, lines: i64, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some(line) = doc.line_of_offset(c) else { return };
        let x = match self.goal_x {
            Some(x) => x,
            None => self.x_of_offset(&doc, c),
        };
        let total = doc.line_count().0;
        let target = (line as i64 + lines).clamp(0, total.saturating_sub(1) as i64) as u64;
        let p = if target == line && lines != 0 {
            // 先頭行・最終行でさらに上下：行頭・行末へ
            if lines < 0 { doc.line_start(line).unwrap_or(0) } else { doc.line_range(line).map_or(c, |(_, e)| e) }
        } else {
            self.offset_in_line_at_x(&doc, target, x).unwrap_or(c)
        };
        self.set_caret(p, extend);
        self.goal_x = Some(x);
    }

    pub(super) fn move_line_edge(&mut self, end: bool, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some((s, e)) = doc.line_of_offset(c).and_then(|l| doc.line_range(l)) else { return };
        // Home は「インデントの後ろ」と「行頭」を行き来する
        let p = if end {
            e
        } else {
            let bytes = doc.read(s, e.min(s + 4096));
            let text = doc.decode(&bytes);
            let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t' || *c == '　').collect();
            let after = s + doc.encode(&indent).len() as u64;
            if c == after { s } else { after }
        };
        self.goal_x = None;
        self.set_caret(p, extend);
    }

    pub(super) fn move_doc_edge(&mut self, end: bool, extend: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let p = if end { doc.len() } else { doc.text_start() as u64 };
        self.goal_x = None;
        self.set_caret(p, extend);
    }

    pub(super) fn move_page(&mut self, down: bool, extend: bool) {
        let page = self.visible_lines().saturating_sub(1).max(1) as i64;
        let delta = if down { page } else { -page };
        self.scroll_by(delta);
        self.move_vertical(delta, extend);
    }

    /// 行頭からの x 座標（横スクロールを含まない）
    pub(super) fn x_of_offset(&self, doc: &Document, pos: u64) -> f32 {
        let Some((s, _)) = doc.line_of_offset(pos).and_then(|l| doc.line_range(l)) else { return 0.0 };
        let src = doc.decode(&doc.read(s, pos)).encode_utf16().count();
        let Some(l) = doc.lines(doc.line_of_offset(pos).unwrap_or(0), 1).into_iter().next() else { return 0.0 };
        let d = self.display(&l);
        let prefix = &d.units[..d.to_disp(src).min(d.shown)];
        unsafe { self.text_width(prefix) }.unwrap_or(prefix.len() as f32 * self.char_w)
    }

    /// 行 `line` の中で x 座標に最も近い位置
    pub(super) fn offset_in_line_at_x(&self, doc: &Document, line: u64, x: f32) -> Option<u64> {
        let l = doc.lines(line, 1).into_iter().next()?;
        let d = self.display(&l);
        let idx = unsafe { self.unit_at_x(&d.units[..d.shown], x) }.unwrap_or(0);
        Some(self.src_units_to_offset(doc, &l, d.to_src(idx)))
    }

    /// 位置が画面に入るようにスクロールする
    pub(super) fn ensure_visible(&mut self, pos: u64) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(line) = doc.line_of_offset(pos) else { return };
        let vis = self.visible_lines() as u64;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + vis {
            self.top = line + 1 - vis;
        }
        let x = self.x_of_offset(&doc, pos);
        let (w, _) = self.client_size();
        let view = (w - self.gutter_width() - 8.0 - 20.0).max(self.char_w * 8.0);
        if x < self.x_off {
            self.x_off = (x - self.char_w * 8.0).max(0.0);
        } else if x > self.x_off + view {
            self.x_off = x - view + self.char_w * 8.0;
        }
        self.update_scrollbars();
    }

    // ---- IME ----

    /// 変換中の文字と候補ウィンドウをカーソルの位置に出す
    pub(super) fn update_ime_position(&self) {
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some(line) = doc.line_of_offset(c) else { return };
        if line < self.top {
            return;
        }
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let x = (self.text_left() + self.x_of_offset(&doc, c)) * scale;
        let y = (self.tab_h() + self.pin_h() + (line - self.top) as f32 * self.line_h) * scale;
        let pt = POINT { x: x as i32, y: y as i32 };
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return;
            }
            let cf = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: pt, rcArea: RECT::default() };
            let _ = ImmSetCompositionWindow(himc, &cf);
            let cand = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: pt,
                rcArea: RECT { left: pt.x, top: pt.y, right: pt.x + 1, bottom: pt.y + (self.line_h * scale) as i32 },
            };
            let _ = ImmSetCandidateWindow(himc, &cand);
            // 変換中の文字を本文と同じ大きさにする
            let mut lf = LOGFONTW { lfHeight: -((self.settings.font_size * scale) as i32), lfCharSet: windows::Win32::Graphics::Gdi::SHIFTJIS_CHARSET, ..Default::default() };
            let face: Vec<u16> = "BIZ UDゴシック".encode_utf16().collect();
            lf.lfFaceName[..face.len()].copy_from_slice(&face);
            let _ = ImmSetCompositionFontW(himc, &lf);
            let _ = ImmReleaseContext(self.hwnd, himc);
        }
    }

    /// 確定した文字列をまとめて受け取る（1 文字ずつの WM_CHAR より速く、元に戻すも 1 回）
    pub(super) fn ime_result(&mut self, lparam: LPARAM) -> bool {
        if lparam.0 as u32 & GCS_RESULTSTR.0 == 0 || self.prompt.is_some() {
            return false;
        }
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return false;
            }
            let bytes = ImmGetCompositionStringW(himc, GCS_RESULTSTR, None, 0);
            let mut s = None;
            if bytes > 0 {
                let mut buf = vec![0u16; bytes as usize / 2];
                ImmGetCompositionStringW(himc, GCS_RESULTSTR, Some(buf.as_mut_ptr() as *mut _), bytes as u32);
                s = Some(String::from_utf16_lossy(&buf));
            }
            let _ = ImmReleaseContext(self.hwnd, himc);
            match s {
                Some(s) => {
                    self.type_text(&s, EditKind::Typing);
                    true
                }
                None => false,
            }
        }
    }

    // ---- 新規・保存 ----

    pub(super) fn new_doc(&mut self) {
        self.cancel_search();
        self.layout_cache.borrow_mut().clear();
        let doc = Arc::new(Document::from_bytes(Vec::new()));
        self.editor = Some(Editor::new(doc.clone()));
        self.doc = Some(doc);
        self.top = 0;
        self.x_off = 0.0;
        self.sel = Some((0, 0));
        self.hit = None;
        self.message = "新規作成（UTF-8）".into();
        self.csv = None;
        self.update_title();
        self.update_scrollbars();
        self.invalidate();
    }

    /// 保存を始める。保存先が決まっていなければ「名前を付けて保存」を返す
    pub(super) fn save(&mut self, target: Option<PathBuf>) -> Option<Deferred> {
        let doc = self.doc.clone()?;
        if self.saving.is_some() {
            return None;
        }
        let target = match target.or_else(|| doc.path().map(|p| p.to_path_buf())) {
            Some(t) => t,
            None => return Some(Deferred::SaveAs),
        };
        let conv = self.convert.take();
        if conv.is_none() && !self.is_modified() && doc.path() == Some(target.as_path()) {
            self.message = "変更はありません".into();
            self.invalidate();
            return None;
        }
        self.cancel_search();
        let written = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        self.saving = Some(SaveJob { written: written.clone(), cancel: cancel.clone(), total: doc.len(), target: target.clone() });
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = doc.write_temp_with(&target, &written, &cancel, conv.as_ref()).map_err(|e| e.to_string());
            drop(doc);
            let done = Box::into_raw(Box::new(SaveDone { result, target, converted: conv.map(|c| c.encoding) }));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_SAVE_DONE, WPARAM(0), LPARAM(done as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(done) });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
        None
    }

    /// 文字コード（または改行コード）を変えて保存：保存先を選んでから書き出す
    pub(super) fn start_convert_save(&mut self, id: u32) -> Option<Deferred> {
        let doc = self.doc.clone()?;
        if self.saving.is_some() {
            return None;
        }
        let cur = doc.encoding();
        let mut conv = kazakiri_core::Convert { encoding: cur.encoding, bom: cur.bom_len > 0, newline: None };
        if let Some(&(_, enc, bom)) = id.checked_sub(menu::ID_CONV_ENC_BASE).and_then(|i| menu::CONV_ENCODINGS.get(i as usize)) {
            conv.encoding = enc;
            conv.bom = bom;
        } else if let Some(&(_, nl)) = id.checked_sub(menu::ID_CONV_NL_BASE).and_then(|i| menu::CONV_NEWLINES.get(i as usize)) {
            conv.newline = Some(nl);
        }
        self.convert = Some(conv);
        Some(Deferred::SaveAs)
    }

    pub(super) fn on_save_done(&mut self, done: SaveDone) {
        self.saving = None;
        let temp = match done.result {
            Ok(t) => t,
            Err(e) => {
                self.message = format!("保存できませんでした: {e}");
                self.pending = None;
                self.invalidate();
                return;
            }
        };
        let Some(doc) = self.doc.take() else { return };
        let forced = done.converted.or(doc.forced_encoding());
        let view = (self.top, self.x_off, self.sel);
        // マップを閉じる（Windows はマップ中のファイルを置き換えられない）
        let weak = Arc::downgrade(&doc);
        drop(doc);
        self.editor = None;
        self.layout_cache.borrow_mut().clear();
        let deadline = Instant::now() + Duration::from_secs(3);
        while weak.strong_count() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let target = done.target;
        let (open_path, msg) = match commit_save(&temp, &target) {
            Ok(()) => (target.clone(), format!("保存しました（{}）", target.file_name().map_or_else(|| target.display().to_string(), |n| n.to_string_lossy().into_owned()))),
            Err(e) => (
                temp.clone(),
                format!("保存先を置き換えられませんでした（{e}）。内容は {} にあります。「名前を付けて保存」で保存し直してください", temp.display()),
            ),
        };
        self.open_keep(open_path, forced, true);
        let (top, x_off, sel) = view;
        self.top = top.min(self.max_top());
        self.x_off = x_off;
        let len = self.doc.as_ref().map_or(0, |d| d.len());
        self.sel = sel.map(|(a, b)| (a.min(len), b.min(len)));
        self.message = msg;
        self.update_title();
        self.update_scrollbars();
        self.invalidate();
        if self.pending.is_some() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_RUN_PENDING, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 未保存の変更があれば確認してから行う
    pub(super) fn guarded(&mut self, action: Pending) -> Option<Deferred> {
        // 表示中の内容を捨てる操作だけ確認する（開く・新規は別のタブになるので確認しない）
        let discards = matches!(action, Pending::Reopen(_) | Pending::CloseTab | Pending::Close);
        if discards && self.is_modified() {
            Some(Deferred::Confirm(action))
        } else {
            Some(Deferred::Perform(action))
        }
    }
}

/// 「名前を付けて保存」ダイアログ
pub(super) unsafe fn save_dialog(hwnd: HWND, current: Option<PathBuf>) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    if let Some(p) = current.as_ref().and_then(|p| p.file_name()) {
        let w: Vec<u16> = p.to_string_lossy().encode_utf16().collect();
        buf[..w.len()].copy_from_slice(&w);
    }
    let filter = wide("すべてのファイル\0*.*\0テキストファイル\0*.txt;*.log;*.csv;*.tsv\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_EXPLORER,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut ofn) }.as_bool() {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    } else {
        None
    }
}

/// クリップボードの文字列（大きさの上限なし）
fn clipboard_text_full() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        OpenClipboard(None).ok()?;
        let result = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let mem = HGLOBAL(h.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max = GlobalSize(mem) / 2;
            let mut n = 0;
            while n < max && *ptr.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n));
            let _ = GlobalUnlock(mem);
            Some(s)
        })();
        let _ = CloseClipboard();
        result
    }
}
