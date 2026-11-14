//! メニュー・ツールバー・キー割り当てから来るコマンドの実行（ID → 操作）。

use super::*;

impl App {
    pub(super) unsafe fn command(&mut self, id: u32) -> Option<Deferred> {
        match id {
            menu::ID_KEYS => return Some(Deferred::ShowKeys),
            menu::ID_SETTINGS => return Some(Deferred::ShowSettings),
            menu::ID_SPLIT => {
                if self.doc.as_ref().and_then(|d| d.path()).is_none() {
                    self.message = "分割するには、先にファイルとして保存してください".into();
                    self.invalidate();
                } else {
                    self.start_prompt(Prompt::Split);
                }
            }
            menu::ID_JOIN => return Some(Deferred::Join),
            id if (menu::ID_CONV_ENC_BASE..menu::ID_CONV_ENC_BASE + menu::CONV_ENCODINGS.len() as u32).contains(&id)
                || (menu::ID_CONV_NL_BASE..menu::ID_CONV_NL_BASE + menu::CONV_NEWLINES.len() as u32).contains(&id) =>
            {
                return self.start_convert_save(id);
            }
            id if (menu::ID_HERE_ENC_BASE..menu::ID_HERE_ENC_BASE + menu::CONV_ENCODINGS.len() as u32).contains(&id)
                || (menu::ID_HERE_NL_BASE..menu::ID_HERE_NL_BASE + menu::CONV_NEWLINES.len() as u32).contains(&id) =>
            {
                return Some(Deferred::ConfirmConvertHere(id));
            }
            menu::ID_BM_TOGGLE => self.toggle_bookmark(),
            menu::ID_BM_NEXT => self.goto_bookmark(true),
            menu::ID_BM_PREV => self.goto_bookmark(false),
            menu::ID_BM_CLEAR => self.clear_bookmarks(),
            menu::ID_BM_LIST => self.list_bookmarks(),
            menu::ID_DIFF => return self.diff_command(),
            menu::ID_SPLIT_VIEW => self.toggle_split_view(),
            id if (menu::ID_DIFF_TAB_BASE..menu::ID_DIFF_TAB_BASE + 64).contains(&id) => self.start_diff((id - menu::ID_DIFF_TAB_BASE) as usize),
            id if (menu::ID_TRAY_ACTIVATE..=menu::ID_TRAY_REMOVE).contains(&id) || (menu::ID_TRAY_FOLDER_BASE..menu::ID_TRAY_FOLDER_BASE + 10).contains(&id) || (menu::ID_TRAY_TAB_BASE..menu::ID_TRAY_TAB_BASE + 20).contains(&id) => {
                if let Some(r) = self.command_tray(id) {
                    return r;
                }
            }
            menu::ID_COLPICK => {
                if self.csv.is_some() {
                    return Some(Deferred::ColMenu);
                }
                self.message = "対象の列を選べるのは CSV/TSV モードのときです（Ctrl+Shift+C）".into();
                self.invalidate();
            }
            menu::ID_COL_FOLLOW => {
                if let Some(v) = &mut self.csv {
                    v.target = None;
                }
                self.message = "対象の列を、カーソルのある列に合わせます".into();
                self.invalidate();
            }
            id if (menu::ID_COL_BASE..menu::ID_COL_BASE + 200).contains(&id) => {
                let col = (id - menu::ID_COL_BASE) as usize;
                if let Some(v) = &mut self.csv {
                    v.target = Some(col);
                }
                let name = self.csv_column_names(usize::MAX).get(col).cloned().unwrap_or_default();
                self.message = format!("対象の列: {} 列目{}", col + 1, if name.is_empty() { String::new() } else { format!("「{name}」") });
                self.invalidate();
            }
            menu::ID_FILTER_LINES => self.start_filter_lines(),
            menu::ID_DIFF_IGN_EDGE => self.diff_ign_edge = !self.diff_ign_edge,
            menu::ID_DIFF_IGN_CASE => self.diff_ign_case = !self.diff_ign_case,
            menu::ID_SPLIT_SYNC => self.split_sync = !self.split_sync,
            menu::ID_CSV_LETTERS => {
                self.csv_letters = !self.csv_letters;
                self.invalidate();
            }
            menu::ID_BM_CUT => self.bookmark_lines(true, true),
            menu::ID_BM_COPY => self.bookmark_lines(false, false),
            menu::ID_BM_DELETE => self.bookmark_lines(false, true),
            menu::ID_BM_INVERT => self.invert_bookmarks(),
            menu::ID_BM_EXTRACT => self.extract_bookmarks(),
            menu::ID_LONGEST => self.find_line_by_length(true),
            menu::ID_SHORTEST => self.find_line_by_length(false),
            menu::ID_FREQUENT => self.frequent_lines(),
            menu::ID_COMPLETE => self.complete_word(),
            id if (menu::ID_MARK_BASE..menu::ID_MARK_BASE + markers::MARK_COLORS as u32).contains(&id) => self.toggle_marker((id - menu::ID_MARK_BASE) as usize),
            menu::ID_MARK_CLEAR => self.clear_markers(),
            id if (menu::ID_CLIP_BASE..menu::ID_CLIP_BASE + menu::CLIP_MAX).contains(&id) => self.paste_history((id - menu::ID_CLIP_BASE) as usize),
            id if self.command_lines(id) => {}
            menu::ID_NEW => self.new_tab(),
            menu::ID_CLOSE_TAB => return self.guarded(Pending::CloseTab),
            menu::ID_NEXT_TAB => self.cycle_tab(true),
            menu::ID_PREV_TAB => self.cycle_tab(false),
            menu::ID_OPEN => return self.guarded(Pending::OpenDialog),
            menu::ID_SAVE => return self.save(None),
            menu::ID_SAVE_AS => return Some(Deferred::SaveAs),
            menu::ID_RELOAD => {
                if self.is_modified() {
                    self.message = "編集中は再読み込みできません（保存するか、元に戻してください）".into();
                    self.invalidate();
                } else {
                    self.refresh(false)
                }
            }
            menu::ID_FOLLOW => {
                if self.is_modified() && !self.follow {
                    self.message = "編集中は追従できません".into();
                    self.invalidate();
                } else {
                    self.set_follow(!self.follow)
                }
            }
            menu::ID_EXIT => {
                self.clear_discard();
                return self.guarded(Pending::Close);
            }
            menu::ID_UNDO => self.undo(false),
            menu::ID_REDO => self.undo(true),
            menu::ID_CUT => self.cut(),
            menu::ID_PASTE => self.paste(),
            menu::ID_COPY => self.copy(),
            menu::ID_SELECT_ALL => {
                if let Some(doc) = &self.doc {
                    self.sel = Some((doc.text_start() as u64, doc.len()));
                    self.invalidate();
                }
            }
            menu::ID_FIND => return Some(Deferred::ShowFind(0)),
            menu::ID_FIND_NEXT => self.find(Direction::Forward),
            menu::ID_FIND_PREV => self.find(Direction::Backward),
            menu::ID_CASE => {
                self.opts.case_sensitive = !self.opts.case_sensitive;
                self.message = format!("大文字と小文字を区別: {}", on_off(self.opts.case_sensitive));
                self.invalidate();
            }
            menu::ID_REGEX => {
                self.opts.regex = !self.opts.regex;
                self.message = format!("正規表現: {}", on_off(self.opts.regex));
                self.invalidate();
            }
            menu::ID_GOTO => self.start_prompt(Prompt::GotoLine),
            menu::ID_FILTER => {
                if self.csv.is_some() {
                    self.start_prompt(Prompt::ColumnFilter)
                } else {
                    self.message = "列フィルタは CSV/TSV モードで使えます（Ctrl+Shift+C）".into();
                    self.invalidate();
                }
            }
            menu::ID_CSV => self.toggle_csv(),
            menu::ID_WHITESPACE => self.toggle_whitespace(),
            menu::ID_LOG_COLORS => self.toggle_log_colors(),
            menu::ID_WRAP => self.toggle_wrap(),
            menu::ID_SYNTAX => {
                self.settings.syntax = !self.settings.syntax;
                self.settings.save();
                self.message = format!("構文の色分け: {}", on_off(self.settings.syntax));
                self.layout_cache.borrow_mut().clear();
                self.invalidate();
            }
            menu::ID_IME_INLINE => {
                self.settings.ime_inline = !self.settings.ime_inline;
                self.settings.save();
                self.message = format!("IME の変換中の文字をエディタ内に表示: {}（次の入力から）", on_off(self.settings.ime_inline));
                self.invalidate();
            }
            menu::ID_TOOLBAR | menu::ID_FKEYS | menu::ID_MODEBAR => {
                match id {
                    menu::ID_TOOLBAR => self.settings.toolbar = !self.settings.toolbar,
                    menu::ID_MODEBAR => self.settings.modebar = !self.settings.modebar,
                    _ => self.settings.fkeys = !self.settings.fkeys,
                }
                self.settings.save();
                self.hover = None;
                self.layout_cache.borrow_mut().clear();
                self.clamp_top();
                self.update_scrollbars();
                self.update_ime_position();
                self.invalidate();
            }
            menu::ID_OVERWRITE => {
                self.overwrite = !self.overwrite;
                self.message = format!("{}モード", if self.overwrite { "上書き" } else { "挿入" });
                self.invalidate();
            }
            id if (menu::ID_TABW_BASE..menu::ID_TABW_BASE + menu::TAB_WIDTHS.len() as u32).contains(&id) => {
                let (font, size) = (self.settings.font.clone(), self.settings.font_size);
                self.set_text_settings(&font, size, menu::TAB_WIDTHS[(id - menu::ID_TABW_BASE) as usize]);
                self.message = format!("タブの幅: {}", self.settings.tab_width);
                self.invalidate();
            }
            menu::ID_ADD_NEXT => self.add_next_match(),
            menu::ID_DARK => self.set_dark(!self.settings.dark),
            menu::ID_ZOOM_IN => self.zoom(1.0),
            menu::ID_ZOOM_OUT => self.zoom(-1.0),
            menu::ID_ZOOM_RESET => self.zoom(0.0),
            menu::ID_CSV_HEADER => self.toggle_header(),
            menu::ID_SORT_ASC => self.sort_by_column(false),
            menu::ID_SORT_DESC => self.sort_by_column(true),
            id if (menu::ID_DELIM_BASE..menu::ID_DELIM_BASE + menu::DELIMS.len() as u32).contains(&id) => {
                self.set_delim(menu::DELIMS[(id - menu::ID_DELIM_BASE) as usize].1)
            }
            menu::ID_REPLACE => return Some(Deferred::ShowFind(1)),
            menu::ID_FIND_ALL => self.find_all(),
            menu::ID_GREP_REPLACE => self.start_grep_replace(),
            menu::ID_RESULTS_TAB => self.results_to_tab(),
            menu::ID_RESULTS_SAVE => {
                if self.results.is_none() {
                    self.message = "結果の一覧がありません".into();
                    self.invalidate();
                } else {
                    return Some(Deferred::SaveResults);
                }
            }
            menu::ID_GREP_BACKUP => {
                self.settings.grep_backup = !self.settings.grep_backup;
                self.settings.save();
                self.message = format!("GREP 置換の前にバックアップ (.bak) を作る: {}", on_off(self.settings.grep_backup));
                self.invalidate();
            }
            menu::ID_GREP => return Some(Deferred::ShowFind(2)),
            menu::ID_NEXT_RESULT => return self.next_result(true),
            menu::ID_PREV_RESULT => return self.next_result(false),
            menu::ID_CLOSE_RESULTS => self.close_results(),
            id if (menu::ID_ENC_BASE..menu::ID_ENC_BASE + menu::ENCODINGS.len() as u32).contains(&id) => {
                let (_, enc) = menu::ENCODINGS[(id - menu::ID_ENC_BASE) as usize];
                return self.guarded(Pending::Reopen(enc));
            }
            id if (menu::ID_RECENT_BASE..menu::ID_RECENT_BASE + menu::RECENT_MAX as u32).contains(&id) => {
                if let Some(p) = self.recent.get((id - menu::ID_RECENT_BASE) as usize).cloned() {
                    return self.guarded(Pending::OpenPath(p));
                }
            }
            _ => {}
        }
        None
    }

    // ---- キーボード ----
}
