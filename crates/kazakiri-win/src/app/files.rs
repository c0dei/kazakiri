//! ファイルを開く・開き直す・再読み込み・末尾の追従。

use kazakiri_core::DiskState;

use super::results::ResultKind;
use super::*;

impl App {
    /// 表示中のタブにファイルを読み込む（新しいタブで開くのは `open`）。
    /// `keep_view` なら表示位置と選択を保つ（再読み込み・開き直し）
    pub(super) fn open_keep(&mut self, path: PathBuf, forced: Option<&'static Encoding>, keep_view: bool) {
        self.cancel_search();
        self.layout_cache.borrow_mut().clear();
        let memory_limit = (self.settings.memory_load_mb as u64) << 20;
        match Document::open_with_limit(&path, forced, memory_limit) {
            Ok(doc) => {
                let doc = Arc::new(doc);
                self.editor = Some(Editor::new(doc.clone()));
                self.doc = Some(doc);
                self.goal_x = None;
                if !keep_view {
                    self.top = 0;
                    self.x_off = 0.0;
                    self.sel = None;
                    self.auto_csv();
                    self.auto_md();
                }
                if self.sel.is_none() {
                    let start = self.doc.as_ref().map_or(0, |d| d.text_start() as u64);
                    self.sel = Some((start, start));
                }
                self.hit = None;
                self.ext_warned = false;
                self.opened_at = Some(Instant::now());
                self.message.clear();
                self.check_autosave(&path);
                recent::push(&mut self.recent, &path);
                let title = wide(&format!("{} - {APP_NAME}", path.display()));
                unsafe {
                    let _ = SetWindowTextW(self.hwnd, PCWSTR(title.as_ptr()));
                    SetTimer(Some(self.hwnd), TIMER_INDEX, 100, None);
                }
            }
            Err(e) => self.message = format!("開けませんでした: {} ({e})", path.display()),
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// 文字コードを変えて開き直す
    pub(super) fn reopen(&mut self, forced: Option<&'static Encoding>) {
        let Some(path) = self.doc.as_ref().and_then(|d| d.path().map(|p| p.to_path_buf())) else { return };
        self.open_keep(path, forced, true);
    }

    /// ディスク上の変更を読み込む。追記だけなら増えた分だけ索引を作る。
    /// `quiet` なら変化がないときに何も表示しない（追従モード用）
    pub(super) fn refresh(&mut self, quiet: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let was_bottom = self.top >= self.max_top();
        match doc.refresh() {
            Ok(Refresh::Unchanged) => {
                if !quiet {
                    self.message = "変更はありません".into();
                    self.invalidate();
                }
            }
            Ok(Refresh::Appended(new)) => {
                let added = new.len() - doc.len();
                self.cancel_search();
                self.layout_cache.borrow_mut().clear();
                let new = Arc::new(new);
                if let Some(ed) = &mut self.editor {
                    ed.reset(new.clone());
                }
                self.doc = Some(new);
                self.message = format!("追記 +{} を読み込みました", human_size(added));
                if was_bottom && (self.follow || !quiet) {
                    self.stick_bottom = true;
                }
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_INDEX, 50, None);
                }
                self.update_scrollbars();
                self.invalidate();
            }
            Ok(Refresh::Replaced) => {
                let path = doc.path().map(|p| p.to_path_buf());
                if let Some(path) = path {
                    self.open_keep(path, doc.forced_encoding(), true);
                    self.forget_stale_positions();
                    self.message = "ファイルが置き換わったため開き直しました".into();
                    if self.follow {
                        self.stick_bottom = true;
                    }
                }
            }
            Err(e) => {
                if !quiet {
                    self.message = format!("再読み込みに失敗しました: {e}");
                    self.invalidate();
                }
            }
        }
    }

    /// 中身が置き換わった（短くなったかもしれない）ファイルを読み直したあと、前の内容を指す位置を捨てる。
    /// 表示の先頭の行は、行索引ができた時点で範囲に収める（`TIMER_INDEX`）
    fn forget_stale_positions(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        let (start, len) = (doc.text_start() as u64, doc.len());
        // 選択は解除して、カーソルだけを範囲内に残す（前の範囲が別の内容を選んだまま、入力で置き換わらないように）
        self.sel = self.sel.map(|(_, caret)| {
            let c = caret.clamp(start.min(len), len);
            (c, c)
        });
        self.rect = None;
        self.multi = None;
        self.dabbrev = None;
        self.pending_goto = None;
        self.dlg_scope = None;
        self.replace_scope = None;
        if matches!(self.results.as_ref().map(|r| &r.kind), Some(ResultKind::InDoc { .. })) {
            self.results = None;
        }
        if self.hexview.is_some() {
            self.hex_scroll(0);
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// 外部での変更の定期確認（表示中のタブ）。
    /// 編集していなければ、変わった内容を自動で読み込む（追記は増えた分だけ）。
    /// 編集中は内容を守るため読み込まず、一度だけ知らせる
    pub(super) fn watch_tick(&mut self) {
        // 追従中は追従のタイマーが読み込む。保存・検索・置換の最中は触らない
        if self.follow || self.saving.is_some() || self.search.is_some() || self.rjob.is_some() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        match doc.disk_state() {
            DiskState::Same => self.ext_warned = false,
            DiskState::Changed if !self.is_modified() => {
                self.refresh(true);
            }
            DiskState::Changed if !self.ext_warned => {
                self.ext_warned = true;
                self.message = "ファイルが外部で変更されました（編集中のため読み込んでいません。保存すると上書きされます）".into();
                self.invalidate();
            }
            DiskState::Missing if !self.ext_warned => {
                self.ext_warned = true;
                self.message = "ファイルが削除または移動されました（内容は開いたままです。「名前を付けて保存」で残せます）".into();
                self.invalidate();
            }
            _ => {}
        }
    }

    pub(super) fn set_follow(&mut self, on: bool) {
        self.follow = on;
        unsafe {
            if on {
                SetTimer(Some(self.hwnd), TIMER_FOLLOW, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_FOLLOW);
            }
        }
        if on {
            self.refresh(true);
            self.scroll_to(u64::MAX);
            self.stick_bottom = true;
        }
        self.message = format!("末尾の追従: {}", on_off(on));
        if on && self.doc.as_ref().is_some_and(|d| d.is_mapped()) {
            self.message.push_str("（大きいファイルのため、開いている間、ほかのアプリはこのファイルを切り詰められません）");
        }
        self.invalidate();
    }
}
