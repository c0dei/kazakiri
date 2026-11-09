//! 置換、全件検索（結果一覧）、GREP。
//!
//! 結果一覧は本文とステータスバーのあいだに出るパネル。
//! クリック・F8（次）・Shift+F8（前）で該当箇所へ移動する。GREP の結果なら別のファイルを開く。

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kazakiri_core::grep::{self, GrepOptions, GrepProgress, GrepResult};
use kazakiri_core::{Document, EditKind, FindAll, Query, SearchControl};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

use super::edit::Pending;
use super::*;

/// 全件検索の結果
pub(super) const WM_RESULTS_DONE: u32 = WM_APP + 4;
/// すべて置換の対象を探し終えた
pub(super) const WM_REPLACE_FOUND: u32 = WM_APP + 5;

/// 一覧に載せる最大件数（ファイル内の全件検索）
const FIND_ALL_LIMIT: usize = 1_000_000;
/// すべて置換できる最大件数
const REPLACE_ALL_LIMIT: usize = 10_000_000;
/// GREP の最大件数
const GREP_LIMIT: usize = 200_000;
/// GREP で対象にするファイルの既定
pub(super) const GREP_DEFAULT_FILES: &str = "*.log;*.txt;*.csv;*.tsv";

const COL_PANEL_BG: D2D1_COLOR_F = rgb(0xF7, 0xF7, 0xF7);
const COL_PANEL_HEAD: D2D1_COLOR_F = rgb(0xE4, 0xE8, 0xEE);
const COL_PANEL_SEL: D2D1_COLOR_F = rgb(0xCC, 0xE4, 0xFF);
const COL_PANEL_FILE: D2D1_COLOR_F = rgb(0x1F, 0x5F, 0xA8);

pub(super) enum ResultKind {
    /// いま開いているファイルの中の一致
    InDoc { ranges: Vec<(u64, u64)> },
    /// GREP の結果
    Grep { root: PathBuf, res: GrepResult },
}

pub(super) struct Results {
    pub kind: ResultKind,
    /// 選んでいる行
    pub sel: usize,
    /// パネルの先頭に表示している行
    pub top: usize,
    pub summary: String,
}

impl Results {
    fn len(&self) -> usize {
        match &self.kind {
            ResultKind::InDoc { ranges } => ranges.len(),
            ResultKind::Grep { res, .. } => res.hits.len(),
        }
    }
}

/// 実行中の全件検索・GREP・すべて置換
pub(super) struct ResultsJob {
    pub id: u64,
    pub started: Instant,
    pub what: JobKind,
}

pub(super) enum JobKind {
    FindAll { ctl: Arc<SearchControl>, total: u64, replace: bool },
    Grep { prog: Arc<GrepProgress> },
}

impl ResultsJob {
    pub fn cancel(&self) {
        match &self.what {
            JobKind::FindAll { ctl, .. } => ctl.cancel.store(true, Ordering::Relaxed),
            JobKind::Grep { prog } => prog.cancel(),
        }
    }
}

pub(super) enum JobDone {
    FindAll { id: u64, query: String, found: FindAll, elapsed: Duration },
    Replace { id: u64, found: FindAll, elapsed: Duration },
    Grep { id: u64, query: String, root: PathBuf, res: std::result::Result<GrepResult, String>, elapsed: Duration },
}

fn post(hwnd: isize, msg: u32, done: JobDone) {
    let ptr = Box::into_raw(Box::new(done));
    if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(ptr as isize)) }.is_err() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// 置換後の文字列の \n \t \\ を展開する（正規表現モードのとき）
fn unescape(s: &str, nl: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push_str(nl),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

impl App {
    // ---- パネルの大きさ ----

    pub(super) fn panel_h(&self) -> f32 {
        if self.results.is_none() && !matches!(self.rjob.as_ref().map(|j| &j.what), Some(JobKind::Grep { .. })) {
            return 0.0;
        }
        let h = self.client_size().1 - self.status_h();
        let rows = self.results.as_ref().map_or(1, |r| r.len()).clamp(1, 12) as f32 + 1.0;
        (rows * self.line_h + 4.0).min(h * 0.4).max(self.line_h * 2.0 + 4.0)
    }

    fn panel_rows(&self) -> usize {
        (((self.panel_h() - 4.0) / self.line_h).floor() as usize).saturating_sub(1).max(1)
    }

    pub(super) fn in_panel(&self, y: f32) -> bool {
        let top = self.client_size().1 - self.status_h() - self.panel_h();
        self.panel_h() > 0.0 && y >= top && y < top + self.panel_h()
    }

    // ---- 全件検索 ----

    /// いまの検索語でファイル内をすべて探し、一覧に出す
    pub(super) fn find_all(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        if self.last_find.is_empty() {
            self.start_prompt(Prompt::Find);
            return;
        }
        let query = match Query::new(&doc, &self.last_find, self.opts) {
            Ok(q) => q,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        self.start_find_all(doc, query, None);
    }

    fn start_find_all(&mut self, doc: Arc<Document>, query: Query, replacement: Option<String>) {
        self.cancel_jobs();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        let replace = replacement.is_some();
        self.rjob = Some(ResultsJob { id, started: Instant::now(), what: JobKind::FindAll { ctl: ctl.clone(), total: doc.len(), replace } });
        let hwnd = self.hwnd.0 as isize;
        let text = self.last_find.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let limit = if replace { REPLACE_ALL_LIMIT } else { FIND_ALL_LIMIT };
            let found = query.find_all(&doc, replacement.as_deref(), limit, &ctl);
            drop(doc);
            if replace {
                post(hwnd, WM_REPLACE_FOUND, JobDone::Replace { id, found, elapsed: t.elapsed() });
            } else {
                post(hwnd, WM_RESULTS_DONE, JobDone::FindAll { id, query: text, found, elapsed: t.elapsed() });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    pub(super) fn cancel_jobs(&mut self) {
        self.cancel_search();
        if let Some(j) = self.rjob.take() {
            j.cancel();
        }
    }

    pub(super) fn on_job_done(&mut self, done: JobDone) {
        let id = match &done {
            JobDone::FindAll { id, .. } | JobDone::Replace { id, .. } | JobDone::Grep { id, .. } => *id,
        };
        if self.rjob.as_ref().map(|j| j.id) != Some(id) {
            return; // 中止した、または古い結果
        }
        self.rjob = None;
        match done {
            JobDone::FindAll { query, found, elapsed, .. } => {
                if found.cancelled {
                    self.message = "検索を中止しました".into();
                } else if found.ranges.is_empty() {
                    self.message = format!("「{query}」は見つかりませんでした");
                    self.results = None;
                } else {
                    let more = if found.truncated { format!("（{} 件で打ち切り）", group(FIND_ALL_LIMIT as u64)) } else { String::new() };
                    let summary = format!("「{query}」 {} 件{more}  {:.2} 秒", group(found.ranges.len() as u64), elapsed.as_secs_f64());
                    self.message = summary.clone();
                    self.results = Some(Results { kind: ResultKind::InDoc { ranges: found.ranges }, sel: 0, top: 0, summary });
                    self.jump_result(0);
                }
            }
            JobDone::Replace { found, elapsed, .. } => self.apply_replace_all(found, elapsed),
            JobDone::Grep { query, root, res, elapsed, .. } => match res {
                Err(e) => self.message = e,
                Ok(res) => {
                    let mut notes = Vec::new();
                    if res.truncated {
                        notes.push(format!("{} 件で打ち切り", group(GREP_LIMIT as u64)));
                    }
                    if res.cancelled {
                        notes.push("中止".into());
                    }
                    if res.skipped_binary > 0 {
                        notes.push(format!("バイナリ {} 件を除外", res.skipped_binary));
                    }
                    if res.errors > 0 {
                        notes.push(format!("開けないファイル {} 件", res.errors));
                    }
                    let notes = if notes.is_empty() { String::new() } else { format!("（{}）", notes.join("、")) };
                    let summary = format!(
                        "GREP「{query}」 {} 件 / {} ファイル中 {} ファイル{notes}  {:.2} 秒",
                        group(res.hits.len() as u64),
                        group(res.files.len() as u64),
                        group(res.matched_files as u64),
                        elapsed.as_secs_f64()
                    );
                    self.message = summary.clone();
                    self.results = Some(Results { kind: ResultKind::Grep { root, res }, sel: 0, top: 0, summary });
                }
            },
        }
        self.update_scrollbars();
        self.invalidate();
    }

    // ---- 結果への移動 ----

    pub(super) fn jump_result(&mut self, i: usize) -> Option<Deferred> {
        let r = self.results.as_mut()?;
        if r.len() == 0 {
            return None;
        }
        let i = i.min(r.len() - 1);
        r.sel = i;
        let rows = self.panel_rows();
        let r = self.results.as_mut().unwrap();
        if i < r.top {
            r.top = i;
        } else if i >= r.top + rows {
            r.top = i + 1 - rows;
        }
        match &r.kind {
            ResultKind::InDoc { ranges } => {
                let (s, e) = ranges[i];
                self.goto_range(s, e);
                None
            }
            ResultKind::Grep { res, .. } => {
                let h = &res.hits[i];
                let path = res.files[h.file].clone();
                let (line, start, end) = (h.line, h.start, h.end);
                let same = self.doc.as_ref().and_then(|d| d.path()).is_some_and(|p| p == path);
                if same {
                    self.goto_range_or_line(line, start, end);
                    None
                } else {
                    self.guarded(Pending::OpenAt(path, line, start, end))
                }
            }
        }
    }

    pub(super) fn next_result(&mut self, forward: bool) -> Option<Deferred> {
        let r = self.results.as_ref()?;
        let n = r.len();
        if n == 0 {
            return None;
        }
        let i = if forward { (r.sel + 1).min(n - 1) } else { r.sel.saturating_sub(1) };
        if i == r.sel && n > 1 {
            self.message = if forward { "最後の結果です" } else { "最初の結果です" }.into();
        }
        self.jump_result(i)
    }

    /// 一致の範囲を選んで画面に出す
    pub(super) fn goto_range(&mut self, s: u64, e: u64) {
        self.hit = Some((s, e));
        self.sel = Some((s, e));
        if let Some(line) = self.doc.as_ref().and_then(|d| d.line_of_offset(s)) {
            let vis = self.visible_lines() as u64;
            if line < self.top || line + 1 >= self.top + vis {
                self.scroll_to(line.saturating_sub(vis / 3));
            }
        }
        self.ensure_visible(s);
        self.invalidate();
    }

    /// GREP の結果へ：ファイルが変わっていなければ範囲、だめなら行へ
    pub(super) fn goto_range_or_line(&mut self, line: u64, s: u64, e: u64) {
        let Some(doc) = self.doc.clone() else { return };
        match doc.line_of_offset(s) {
            Some(l) if l == line && e <= doc.len() => self.goto_range(s, e),
            _ => match doc.line_range(line) {
                Some((ls, le)) => self.goto_range(ls, le),
                // 索引がまだなら、できしだい移動する
                None => self.pending_goto = Some((line, s, e)),
            },
        }
    }

    // ---- パネルのマウス操作 ----

    pub(super) fn panel_click(&mut self, y: f32) -> Option<Deferred> {
        let top = self.client_size().1 - self.status_h() - self.panel_h();
        let row = ((y - top - 2.0) / self.line_h).floor() as i64 - 1;
        if row < 0 {
            return None; // 見出しの行
        }
        let r = self.results.as_ref()?;
        let i = r.top + row as usize;
        if i < r.len() { self.jump_result(i) } else { None }
    }

    pub(super) fn panel_scroll(&mut self, rows: i64) {
        let visible = self.panel_rows();
        if let Some(r) = self.results.as_mut() {
            let max = r.len().saturating_sub(visible) as i64;
            r.top = (r.top as i64 + rows).clamp(0, max) as usize;
            self.invalidate();
        }
    }

    pub(super) fn close_results(&mut self) {
        if self.results.take().is_some() {
            self.update_scrollbars();
            self.invalidate();
        }
    }

    // ---- 描画 ----

    pub(super) unsafe fn draw_panel(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, top: f32) -> Result<()> {
        let h = self.panel_h();
        if h <= 0.0 {
            return Ok(());
        }
        unsafe {
            brush.SetColor(&COL_PANEL_BG);
            rt.FillRectangle(&rect(0.0, top, w, top + h), brush);
            brush.SetColor(&COL_PANEL_HEAD);
            rt.FillRectangle(&rect(0.0, top, w, top + self.line_h + 2.0), brush);
            brush.SetColor(&COL_TEXT);
            let head = match (&self.results, &self.rjob) {
                (_, Some(job)) if matches!(job.what, JobKind::Grep { .. }) => self.job_status(job),
                (Some(r), _) => format!("{}    F8: 次  Shift+F8: 前  Esc: 閉じる", r.summary),
                _ => String::new(),
            };
            self.draw_str(rt, brush, &head, 8.0, top + 1.0, w - 8.0)?;
            let Some(r) = &self.results else { return Ok(()) };
            let doc = self.doc.as_ref();
            for row in 0..self.panel_rows() {
                let i = r.top + row;
                if i >= r.len() {
                    break;
                }
                let y = top + 2.0 + (row + 1) as f32 * self.line_h;
                if i == r.sel {
                    brush.SetColor(&COL_PANEL_SEL);
                    rt.FillRectangle(&rect(0.0, y, w, y + self.line_h), brush);
                }
                match &r.kind {
                    ResultKind::InDoc { ranges } => {
                        let (s, _) = ranges[i];
                        let (num, text) = match doc.and_then(|d| d.line_of_offset(s).map(|l| (d, l))) {
                            Some((d, l)) => (group(l + 1), d.lines(l, 1).into_iter().next().map(|x| x.text).unwrap_or_default()),
                            None => ("?".into(), String::new()),
                        };
                        brush.SetColor(&COL_GUTTER_TEXT);
                        self.draw_str(rt, brush, &format!("{num:>12}"), 4.0, y, self.char_w * 14.0)?;
                        brush.SetColor(&COL_TEXT);
                        let text: String = text.chars().take(400).collect();
                        self.draw_str(rt, brush, &text, self.char_w * 15.0, y, w - 4.0)?;
                    }
                    ResultKind::Grep { root, res } => {
                        let hit = &res.hits[i];
                        let path = &res.files[hit.file];
                        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
                        let loc = format!("{rel}({})", group(hit.line + 1));
                        brush.SetColor(&COL_PANEL_FILE);
                        let lw = self.text_width(&loc.encode_utf16().collect::<Vec<_>>())?.min(w * 0.45);
                        self.draw_str(rt, brush, &loc, 8.0, y, 8.0 + lw)?;
                        brush.SetColor(&COL_TEXT);
                        self.draw_str(rt, brush, &format!(": {}", hit.text), 8.0 + lw, y, w - 4.0)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// 実行中の処理の表示
    pub(super) fn job_status(&self, job: &ResultsJob) -> String {
        let secs = job.started.elapsed().as_secs_f64();
        match &job.what {
            JobKind::FindAll { ctl, total, replace } => {
                let pct = ctl.scanned.load(Ordering::Relaxed) as f64 * 100.0 / (*total).max(1) as f64;
                let what = if *replace { "置換の対象を検索中" } else { "すべて検索中" };
                format!("{what}「{}」 {:.0}%  {secs:.1} 秒  (Esc で中止)", self.last_find, pct.min(100.0))
            }
            JobKind::Grep { prog } => format!(
                "GREP 中「{}」 {} / {} ファイル  {} 件  {} 読み込み  {secs:.1} 秒  (Esc で中止)",
                self.last_find,
                group(prog.files_done.load(Ordering::Relaxed) as u64),
                group(prog.files_total.load(Ordering::Relaxed) as u64),
                group(prog.hits.load(Ordering::Relaxed) as u64),
                human_size(prog.ctl.scanned.load(Ordering::Relaxed)),
            ),
        }
    }

    // ---- 置換 ----

    fn replacement_text(&self) -> String {
        if self.opts.regex { unescape(&self.last_replace, self.newline_str()) } else { self.last_replace.clone() }
    }

    /// いまの一致を置換して次を探す（`replace` が false なら置換せずに次へ）
    pub(super) fn replace_next(&mut self, replace: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let repl = self.replacement_text();
        if replace {
            if let Some((s, e)) = self.hit {
                if !self.check_editable() {
                    return;
                }
                if !self.opts.regex && !doc.can_encode(&repl) {
                    self.message = format!("置換後の文字列は {} で表せません", doc.encoding().name());
                    return self.invalidate();
                }
                let q = match Query::new(&doc, &self.last_find, self.opts) {
                    Ok(q) => q,
                    Err(e) => {
                        self.message = e;
                        return self.invalidate();
                    }
                };
                if let Some(bytes) = q.replacement_at(&doc, s, e, &repl) {
                    let n = bytes.len() as u64;
                    drop(doc);
                    let sel = self.sel.unwrap_or((s, e));
                    let r = self.with_editor(|ed| ed.replace_bytes(s, e, bytes, EditKind::Other, sel));
                    match r {
                        Some(Ok(_)) => {
                            // 置換した直後（カーソル位置）から次を探す
                            self.hit = None;
                            self.sel = Some((s + n, s + n));
                            self.after_edit();
                        }
                        Some(Err(e)) => self.message = e.to_string(),
                        None => {}
                    }
                }
            }
        }
        self.find(Direction::Forward);
    }

    /// すべて置換（対象を別スレッドで探し、見つかったら 1 回の編集として適用する）
    pub(super) fn replace_all(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        if self.last_find.is_empty() || !self.check_editable() {
            return;
        }
        let repl = self.replacement_text();
        if !self.opts.regex && !doc.can_encode(&repl) {
            self.message = format!("置換後の文字列は {} で表せません", doc.encoding().name());
            return self.invalidate();
        }
        let query = match Query::new(&doc, &self.last_find, self.opts) {
            Ok(q) => q,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        self.start_find_all(doc, query, Some(repl));
    }

    fn apply_replace_all(&mut self, found: FindAll, elapsed: Duration) {
        if found.cancelled {
            self.message = "置換を中止しました".into();
            return;
        }
        if found.truncated {
            self.message = format!("一致が多すぎます（{} 件を超えました）。検索語を絞ってください", group(REPLACE_ALL_LIMIT as u64));
            return;
        }
        if found.ranges.is_empty() {
            self.message = format!("「{}」は見つかりませんでした", self.last_find);
            return;
        }
        let t = Instant::now();
        let sel = self.sel.unwrap_or((0, 0));
        let r = self.with_editor(|ed| ed.replace_all(&found.ranges, &found.replacements, sel));
        match r {
            Some(Ok(n)) => {
                self.hit = None;
                let c = self.caret().min(self.doc.as_ref().map_or(0, |d| d.len()));
                self.sel = Some((c, c));
                self.after_edit();
                self.message = format!(
                    "{} 件置換しました（検索 {:.2} 秒 + 適用 {:.2} 秒）。Ctrl+Z でまとめて元に戻せます",
                    group(n as u64),
                    elapsed.as_secs_f64(),
                    t.elapsed().as_secs_f64()
                );
            }
            Some(Err(e)) => self.message = e.to_string(),
            None => {}
        }
    }

    // ---- GREP ----

    pub(super) fn start_grep(&mut self) {
        let root = PathBuf::from(self.grep_folder.trim().trim_matches('"'));
        let opts = GrepOptions {
            search: self.opts,
            include: grep::parse_include(&self.grep_files),
            recursive: true,
            max_hits: GREP_LIMIT,
        };
        self.cancel_jobs();
        let id = self.next_job;
        self.next_job += 1;
        let prog = Arc::new(GrepProgress::default());
        self.rjob = Some(ResultsJob { id, started: Instant::now(), what: JobKind::Grep { prog: prog.clone() } });
        let hwnd = self.hwnd.0 as isize;
        let query = self.last_find.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let res = grep::grep(&root, &query, &opts, &prog);
            post(hwnd, WM_RESULTS_DONE, JobDone::Grep { id, query, root, res, elapsed: t.elapsed() });
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// GREP のフォルダの初期値：前回の値 → いまのファイルのフォルダ → 作業フォルダ
    pub(super) fn default_grep_folder(&self) -> String {
        if !self.grep_folder.is_empty() {
            return self.grep_folder.clone();
        }
        self.doc
            .as_ref()
            .and_then(|d| d.path().and_then(|p| p.parent()).map(|p| p.display().to_string()))
            .or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string()))
            .unwrap_or_default()
    }
}
