//! ファイルを開く・開き直す・再読み込み・末尾の追従。

use super::*;

impl App {
    /// 表示中のタブにファイルを読み込む（新しいタブで開くのは `open`）。
    /// `keep_view` なら表示位置と選択を保つ（再読み込み・開き直し）
    pub(super) fn open_keep(&mut self, path: PathBuf, forced: Option<&'static Encoding>, keep_view: bool) {
        self.cancel_search();
        self.layout_cache.borrow_mut().clear();
        match Document::open_with(&path, forced) {
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
                }
                if self.sel.is_none() {
                    let start = self.doc.as_ref().map_or(0, |d| d.text_start() as u64);
                    self.sel = Some((start, start));
                }
                self.hit = None;
                self.opened_at = Some(Instant::now());
                self.message.clear();
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
        self.invalidate();
    }
}
