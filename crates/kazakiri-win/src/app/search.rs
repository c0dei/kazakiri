//! 検索（次／前、バックグラウンド検索の開始・完了）と、ステータスバーの入力欄（行へ移動など）。

use super::*;

impl App {
    pub(super) fn find(&mut self, dir: Direction) {
        let Some(doc) = self.doc.clone() else { return };
        if self.last_find.is_empty() {
            self.start_find_bar(false);
            return;
        }
        let query = match Query::new(&doc, &self.last_find, self.opts) {
            Ok(q) => q,
            Err(e) => {
                self.message = e;
                self.invalidate();
                return;
            }
        };
        // 起点：直前の一致の次 → 選択の端 → カーソル位置 → 画面先頭
        let from = match (self.hit, self.selection(), self.sel) {
            // 空の一致（正規表現の ^ など）は 1 つ進めないと同じ所で止まり続ける
            (Some((s, e)), _, _) => match dir {
                Direction::Forward => if e > s { e } else { s + 1 },
                Direction::Backward => s,
            },
            (None, Some((a, b)), _) => if dir == Direction::Forward { b } else { a },
            (None, None, Some((_, c))) => c,
            (None, None, None) => doc.line_range(self.top).map_or(0, |(s, _)| s),
        };
        self.start_search(doc, query, dir, from, false, from);
    }

    pub(super) fn start_search(&mut self, doc: Arc<Document>, query: Query, dir: Direction, from: u64, wrapped: bool, origin: u64) {
        let span = match (dir, wrapped) {
            (Direction::Forward, false) => doc.len().saturating_sub(from),
            (Direction::Backward, false) => from,
            (Direction::Forward, true) => origin,
            (Direction::Backward, true) => doc.len().saturating_sub(origin),
        };

        self.cancel_search();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        self.search = Some(SearchJob { id, ctl: ctl.clone(), started: Instant::now(), dir, span, wrapped, origin });
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let t = Instant::now();
            let outcome = query.find(&doc, from, dir, &ctl);
            let done = Box::new(SearchDone { id, outcome, elapsed: t.elapsed() });
            let ptr = Box::into_raw(done);
            let hwnd = HWND(hwnd as *mut _);
            if unsafe { PostMessageW(Some(hwnd), WM_SEARCH_DONE, WPARAM(0), LPARAM(ptr as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(ptr) });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    pub(super) fn cancel_search(&self) {
        if let Some(job) = &self.search {
            job.ctl.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn on_search_done(&mut self, done: SearchDone) {
        if self.search.as_ref().map(|j| j.id) != Some(done.id) {
            return; // 古い検索の結果
        }
        let job = self.search.take().unwrap();
        let ms = done.elapsed.as_secs_f64() * 1000.0;
        let Some(doc) = self.doc.clone() else { return };
        // 折り返し後に、検索を始めた位置より先で見つかったものは「見つからない」扱い
        let outcome = match done.outcome {
            SearchOutcome::Found(s, _) if job.wrapped && ((job.dir == Direction::Forward && s >= job.origin) || (job.dir == Direction::Backward && s < job.origin)) => {
                SearchOutcome::NotFound
            }
            o => o,
        };
        match outcome {
            SearchOutcome::Found(s, e) => {
                self.hit = Some((s, e));
                self.sel = Some((s, e));
                let wrap_note = match (job.wrapped, job.dir) {
                    (true, Direction::Forward) => "末尾まで達したので先頭から探しました。",
                    (true, Direction::Backward) => "先頭まで達したので末尾から探しました。",
                    _ => "",
                };
                match doc.line_of_offset(s) {
                    Some(line) => {
                        self.message = format!("{wrap_note}{} 行目で見つかりました（{ms:.0} ms）", group(line + 1));
                        let vis = self.visible_lines() as u64;
                        if line < self.top || line >= self.top + vis.saturating_sub(1) {
                            self.scroll_to(line.saturating_sub(vis / 3));
                        }
                        self.reveal_column(&doc, line, s);
                    }
                    None => self.message = "見つかりましたが、索引作成中のためまだ移動できません".into(),
                }
            }
            // まだ探していない側（前方なら先頭〜開始位置、後方なら開始位置〜末尾）があれば折り返す
            SearchOutcome::NotFound
                if !job.wrapped
                    && match job.dir {
                        Direction::Forward => job.origin > doc.text_start() as u64,
                        Direction::Backward => job.origin < doc.len(),
                    } =>
            {
                // 反対の端から、始めた位置まで探し直す
                if let Ok(query) = Query::new(&doc, &self.last_find, self.opts) {
                    let from = if job.dir == Direction::Forward { doc.text_start() as u64 } else { doc.len() };
                    self.start_search(doc, query, job.dir, from, true, job.origin);
                    return;
                }
            }
            SearchOutcome::NotFound => {
                self.message = format!("「{}」は見つかりませんでした（{ms:.0} ms）", self.last_find);
            }
            SearchOutcome::Cancelled => self.message = "検索を中止しました".into(),
        }
        self.invalidate();
    }

    /// 一致が横にはみ出していたら横スクロールする
    pub(super) fn reveal_column(&mut self, doc: &Document, line: u64, offset: u64) {
        let Some(l) = doc.lines(line, 1).into_iter().next() else { return };
        let prefix = decode_units(doc, &doc.read(l.start, offset.min(l.end)));
        let x = prefix as f32 * self.char_w; // おおよその位置で十分
        let w = self.editor_w();
        let view = w - self.gutter_width() - 8.0;
        if x < self.x_off || x > self.x_off + view - self.char_w * 8.0 {
            self.x_off = (x - view / 3.0).max(0.0);
            self.update_scrollbars();
        }
    }

    // ---- マウス・選択 ----

    /// 検索・置換バーを開く（すでに開いていれば、検索欄に戻る）。選択中の 1 行の文字列があれば検索語にする
    pub(super) fn start_find_bar(&mut self, replace: bool) {
        let was = self.is_find_bar();
        let selected = self.selected_text(1024).filter(|s| !s.is_empty() && !s.contains('\n'));
        self.prompt = Some(if replace { Prompt::Replace } else { Prompt::Find });
        if !was || selected.is_some() {
            self.input = selected.unwrap_or_else(|| self.last_find.clone());
        }
        if !was {
            self.input2 = self.last_replace.clone();
        }
        self.prompt_focus = 0;
        self.comp = None;
        self.find_bar_changed();
    }

    pub(super) fn is_find_bar(&self) -> bool {
        matches!(self.prompt, Some(Prompt::Find | Prompt::Replace))
    }

    /// 入力中の欄（検索・置換バーでは、いま入力している欄。それ以外は入力欄）
    pub(super) fn prompt_buf(&mut self) -> &mut String {
        if self.prompt == Some(Prompt::Replace) && self.prompt_focus == 1 { &mut self.input2 } else { &mut self.input }
    }

    /// 検索・置換バーの入力が変わった：F3 などでも同じ語で探せるように、検索語・置換語を更新する
    pub(super) fn find_bar_changed(&mut self) {
        if self.prompt == Some(Prompt::Palette) {
            // 絞り込みが変わったら、先頭の行を選び直す
            self.palette_sel = 0;
            self.palette_top = 0;
        }
        if self.is_find_bar() {
            self.last_find = self.input.clone();
            self.last_replace = self.input2.clone();
        }
        self.invalidate();
    }

    /// 検索・置換バーの操作（Enter・ボタン）
    pub(super) fn find_bar_do(&mut self, act: FindAct) {
        self.find_bar_changed();
        if self.last_find.is_empty() && !matches!(act, FindAct::Close | FindAct::Case | FindAct::Regex | FindAct::Word) {
            self.message = "検索する文字列を入力してください".into();
            self.invalidate();
            return;
        }
        match act {
            FindAct::Next => self.find_and_remember(Direction::Forward),
            FindAct::Prev => self.find_and_remember(Direction::Backward),
            FindAct::ReplOne => {
                self.history.push(crate::history::Kind::Replace, &self.last_replace.clone());
                self.replace_next(true);
            }
            FindAct::ReplAll => {
                self.history.push(crate::history::Kind::Replace, &self.last_replace.clone());
                self.replace_scope = None;
                self.replace_all();
            }
            FindAct::Case => self.opts.case_sensitive = !self.opts.case_sensitive,
            FindAct::Regex => self.opts.regex = !self.opts.regex,
            FindAct::Word => self.opts.whole_word = !self.opts.whole_word,
            FindAct::Close => {
                self.prompt = None;
                self.comp = None;
            }
        }
        self.invalidate();
    }

    fn find_and_remember(&mut self, dir: Direction) {
        let q = self.last_find.clone();
        self.history.push(crate::history::Kind::Find, &q);
        self.find(dir);
    }

    pub(super) fn start_prompt(&mut self, p: Prompt) {
        self.prompt = Some(p);
        self.input = match p {
            Prompt::GotoLine => String::new(),
            // 対象の列（カーソルのある列）を入れておく。「: 」は「含む」
            Prompt::ColumnFilter => match self.target_column() {
                Some((col, _, _)) => format!("{} : ", col + 1),
                None => self.last_filter.clone(),
            },
            Prompt::Split => "1000000".into(),
            Prompt::Sequence => "1,1,0".into(),
            Prompt::LineFilter => self.last_find.clone(),
            Prompt::Find | Prompt::Replace => self.last_find.clone(),
            Prompt::Palette => String::new(),
        };
        self.invalidate();
    }

    /// 索引ができて行の位置が分かれば、保留していた移動をする
    pub(super) fn apply_pending_goto(&mut self) {
        let Some((line, s, e)) = self.pending_goto else { return };
        if self.doc.as_ref().is_some_and(|d| d.line_range(line).is_some()) {
            self.pending_goto = None;
            self.goto_range_or_line(line, s, e);
        }
    }

    pub(super) fn submit_prompt(&mut self) -> Option<Deferred> {
        match self.prompt.take() {
            Some(Prompt::Find | Prompt::Replace) => {}
            Some(Prompt::Palette) => {
                let id = self.palette_results().get(self.palette_sel).map(|e| e.2);
                self.comp = None;
                self.invalidate();
                return id.and_then(|id| unsafe { self.command(id) });
            }
            Some(Prompt::ColumnFilter) => {
                self.last_filter = std::mem::take(&mut self.input);
                self.start_filter();
            }
            Some(Prompt::Split) => {
                let spec = std::mem::take(&mut self.input);
                self.start_split(&spec);
            }
            Some(Prompt::LineFilter) => {
                let text = std::mem::take(&mut self.input);
                if text.is_empty() {
                    self.message = "フィルターする文字列を入力してください".into();
                } else {
                    self.last_find = text;
                    self.history.push(crate::history::Kind::Find, &self.last_find);
                    self.filter_pending = true;
                    self.find_all();
                }
            }
            Some(Prompt::Sequence) => {
                let spec = std::mem::take(&mut self.input);
                self.insert_sequence(&spec);
            }
            Some(Prompt::GotoLine) => {
                let input = std::mem::take(&mut self.input);
                match input.trim().replace(',', "").parse::<u64>() {
                    Ok(n) if n >= 1 => {
                        let target = n - 1;
                        let known = self.doc.as_ref().map_or(0, |d| d.line_count().0);
                        if target >= known {
                            self.message = format!("{} 行目はまだ索引にありません", group(n));
                        }
                        self.scroll_to(target.saturating_sub(self.visible_lines() as u64 / 3));
                        if let Some((s, e)) = self.doc.as_ref().and_then(|d| d.line_range(target)) {
                            self.sel = Some((s, e));
                        }
                    }
                    _ => self.message = "行番号を入力してください".into(),
                }
            }
            None => {}
        }
        self.invalidate();
        None
    }

    // ---- 検索 ----
}
