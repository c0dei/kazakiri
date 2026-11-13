//! タブ（複数ファイル）。
//!
//! いま表示しているタブの状態は `App` のフィールドにそのまま置き、
//! 表示していないタブの状態だけを `Tab` に入れて `App::tabs` に持つ。
//! 切り替えるときに出し入れするので、描画や編集の処理はタブを意識しなくてよい。
//! `tabs[active]` は中身が空の置き場所（表示中の状態は `App` 側にある）。

use std::path::{Path, PathBuf};

use super::edit::Pending;
use super::results::{JobKind, ResultKind};
use super::*;

/// タブの中の余白（左）と、閉じるボタンの幅
const PAD: f32 = 12.0;
const CLOSE_W: f32 = 22.0;
const MIN_W: f32 = 90.0;
/// タブに出す名前の最大文字数
const MAX_TITLE: usize = 32;

/// 表示していないタブの状態
#[derive(Default)]
pub(super) struct Tab {
    doc: Option<Arc<Document>>,
    editor: Option<Editor>,
    top: u64,
    x_off: f32,
    sel: Option<(u64, u64)>,
    hit: Option<(u64, u64)>,
    goal_x: Option<f32>,
    csv: Option<CsvView>,
    follow: bool,
    stick_bottom: bool,
    pending_goto: Option<(u64, u64, u64)>,
    bookmarks: std::collections::BTreeSet<u64>,
    bm_lines: u64,
    /// 終了の確認で「保存しない」を選んだ（終了の手続きの間だけ使う）
    discard: bool,
}

fn title_of(doc: Option<&Arc<Document>>) -> String {
    let name = doc
        .and_then(|d| d.path())
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "（無題）".into());
    if name.chars().count() > MAX_TITLE {
        let head: String = name.chars().take(MAX_TITLE - 1).collect();
        format!("{head}…")
    } else {
        name
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// タブバーの中の 1 つ分
pub(super) struct TabBox {
    pub x0: f32,
    pub x1: f32,
    /// 表示する名前（未保存なら先頭に * を付ける）
    pub title: String,
}

impl App {
    /// タブ `i` のファイル（表示中のタブは App 側にある）
    pub(super) fn tab_doc(&self, i: usize) -> Option<Arc<Document>> {
        if i == self.active { self.doc.clone() } else { self.tabs.get(i)?.doc.clone() }
    }

    pub(super) fn tab_title(&self, i: usize) -> String {
        title_of(self.tab_doc(i).as_ref())
    }

    // ---- 状態の出し入れ ----

    /// 表示中の状態を取り出す（`App` 側は空になる）
    fn stash(&mut self) -> Tab {
        self.opened_at = None;
        Tab {
            doc: self.doc.take(),
            editor: self.editor.take(),
            top: std::mem::take(&mut self.top),
            x_off: std::mem::take(&mut self.x_off),
            sel: self.sel.take(),
            hit: self.hit.take(),
            goal_x: self.goal_x.take(),
            csv: self.csv.take(),
            follow: std::mem::take(&mut self.follow),
            stick_bottom: std::mem::take(&mut self.stick_bottom),
            pending_goto: self.pending_goto.take(),
            bookmarks: std::mem::take(&mut self.bookmarks),
            bm_lines: std::mem::take(&mut self.bm_lines),
            discard: std::mem::take(&mut self.discard),
        }
    }

    fn load(&mut self, t: Tab) {
        self.doc = t.doc;
        self.editor = t.editor;
        self.top = t.top;
        self.x_off = t.x_off;
        self.sel = t.sel;
        self.hit = t.hit;
        self.goal_x = t.goal_x;
        self.csv = t.csv;
        self.follow = t.follow;
        self.stick_bottom = t.stick_bottom;
        self.pending_goto = t.pending_goto;
        self.bookmarks = t.bookmarks;
        self.bm_lines = t.bm_lines;
        self.discard = t.discard;
    }

    /// タブを離れる前の後始末：そのファイルに結び付いた処理や表示をやめる
    fn leave_tab(&mut self) {
        self.diffview = None;
        self.cancel_search();
        self.search = None;
        // GREP はファイルに結び付かないので続ける
        if !matches!(self.rjob.as_ref().map(|j| &j.what), Some(JobKind::Grep { .. } | JobKind::GrepReplace { .. })) {
            if let Some(j) = self.rjob.take() {
                j.cancel();
            }
        }
        if matches!(self.results.as_ref().map(|r| &r.kind), Some(ResultKind::InDoc { .. })) {
            self.results = None;
        }
        self.prompt = None;
        self.dragging = false;
        self.layout_cache.borrow_mut().clear();
    }

    /// タブに入った後の準備：タイマー、タイトル、スクロールバー
    fn enter_tab(&mut self) {
        self.layout_cache.borrow_mut().clear();
        unsafe {
            if self.doc.as_ref().is_some_and(|d| !d.progress().done) || self.pending_goto.is_some() {
                SetTimer(Some(self.hwnd), TIMER_INDEX, 100, None);
            }
            if self.follow {
                SetTimer(Some(self.hwnd), TIMER_FOLLOW, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_FOLLOW);
            }
        }
        self.update_title();
        self.update_scrollbars();
        self.update_ime_position();
        self.reveal_active_tab();
        self.invalidate();
    }

    fn busy_saving(&mut self) -> bool {
        if self.saving.is_some() {
            self.message = "保存が終わるまでお待ちください".into();
            self.invalidate();
            return true;
        }
        false
    }

    // ---- 切り替え・追加・削除 ----

    pub(super) fn switch_to(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() || self.busy_saving() {
            return;
        }
        self.leave_tab();
        let cur = self.stash();
        self.tabs[self.active] = cur;
        let next = std::mem::take(&mut self.tabs[i]);
        self.load(next);
        self.active = i;
        self.message.clear();
        self.enter_tab();
    }

    /// 次（`forward` でなければ前）のタブへ
    pub(super) fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n > 1 {
            self.switch_to((self.active + if forward { 1 } else { n - 1 }) % n);
        }
    }

    /// 表示中のタブの右隣に空のタブを作って、そこへ移る
    fn push_tab(&mut self) {
        self.leave_tab();
        let cur = self.stash();
        self.tabs[self.active] = cur;
        self.active += 1;
        self.tabs.insert(self.active, Tab::default());
    }

    /// 表示中のタブが「何もしていない無題」か（ファイルを開くときに使い回す）
    pub(super) fn is_blank(&self) -> bool {
        match &self.doc {
            None => true,
            Some(d) => d.path().is_none() && d.is_empty() && !self.is_modified(),
        }
    }

    fn tab_path(&self, i: usize) -> Option<PathBuf> {
        let doc = if i == self.active { self.doc.as_ref() } else { self.tabs[i].doc.as_ref() };
        doc.and_then(|d| d.path()).map(|p| p.to_path_buf())
    }

    pub(super) fn tab_modified(&self, i: usize) -> bool {
        if i == self.active { self.is_modified() } else { self.tabs[i].editor.as_ref().is_some_and(|e| e.is_modified()) }
    }

    /// ファイルを開く。すでに開いていればそのタブへ移り、そうでなければ新しいタブで開く
    pub(super) fn open(&mut self, path: PathBuf, forced: Option<&'static Encoding>) {
        if self.busy_saving() {
            return;
        }
        if let Some(i) = (0..self.tabs.len()).find(|&i| self.tab_path(i).is_some_and(|p| same_file(&p, &path))) {
            self.switch_to(i);
            return;
        }
        let fresh = !self.is_blank();
        if fresh {
            self.push_tab();
        }
        self.open_keep(path, forced, false);
        if fresh && self.doc.is_none() {
            // 開けなかったので、作ったタブを片付ける（理由のメッセージは残す）
            let msg = std::mem::take(&mut self.message);
            self.remove_active_tab();
            self.message = msg;
        }
        self.enter_tab();
    }

    /// 新しいタブ（無題）
    pub(super) fn new_tab(&mut self) {
        if self.busy_saving() {
            return;
        }
        if self.is_blank() && self.doc.is_some() {
            self.message = "すでに新規のタブです".into();
            return self.invalidate();
        }
        if self.doc.is_some() {
            self.push_tab();
        }
        self.new_doc();
        self.enter_tab();
    }

    /// 表示中のタブを取り除き、隣のタブを表示する（最後の 1 つなら無題にする）
    fn remove_active_tab(&mut self) {
        self.leave_tab();
        drop(self.stash());
        if self.tabs.len() == 1 {
            self.new_doc();
            self.message.clear();
            return;
        }
        self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len() - 1);
        let next = std::mem::take(&mut self.tabs[self.active]);
        self.load(next);
    }

    /// 表示中のタブを閉じる（未保存の確認は済んでいること）
    pub(super) fn close_active_tab(&mut self) {
        if self.busy_saving() {
            return;
        }
        self.remove_active_tab();
        self.enter_tab();
    }

    // ---- 終了 ----

    /// 終了の手続きを始める前に、「保存しない」の記録を消す
    pub(super) fn clear_discard(&mut self) {
        self.discard = false;
        for t in &mut self.tabs {
            t.discard = false;
        }
    }

    /// 終了の続き：表示中のタブは片付いたので、まだ確認していない未保存のタブを探す。
    /// あればそのタブを表示して確認を返す。なければ None（終了してよい）
    pub(super) fn continue_close(&mut self) -> Option<Deferred> {
        self.discard = true;
        let next = (0..self.tabs.len()).find(|&i| {
            let discard = if i == self.active { self.discard } else { self.tabs[i].discard };
            self.tab_modified(i) && !discard
        })?;
        self.switch_to(next);
        Some(Deferred::Confirm(Pending::Close))
    }

    // ---- タブバー ----

    pub(super) fn tab_h(&self) -> f32 {
        self.ui_line_h + 8.0
    }

    /// タブの並び（横スクロールを引く前の位置）
    pub(super) fn tab_boxes(&self) -> Vec<TabBox> {
        let mut x = 0.0;
        (0..self.tabs.len())
            .map(|i| {
                let doc = if i == self.active { self.doc.as_ref() } else { self.tabs[i].doc.as_ref() };
                let title = title_of(doc);
                let shown = if self.tab_modified(i) { format!("* {title}") } else { title };
                let units: Vec<u16> = shown.encode_utf16().collect();
                let tw = unsafe { self.ui_text_width(&units) }.unwrap_or(units.len() as f32 * self.ui_char_w);
                let w = (PAD + tw + CLOSE_W).max(MIN_W);
                let b = TabBox { x0: x, x1: x + w, title: shown };
                x += w;
                b
            })
            .collect()
    }

    /// 表示中のタブがバーの中に見えるように横へずらす
    fn reveal_active_tab(&mut self) {
        let boxes = self.tab_boxes();
        let Some(b) = boxes.get(self.active) else { return };
        let w = self.client_size().0;
        if b.x0 < self.tab_scroll {
            self.tab_scroll = b.x0;
        } else if b.x1 > self.tab_scroll + w {
            self.tab_scroll = b.x1 - w;
        }
        let total = boxes.last().map_or(0.0, |l| l.x1);
        self.tab_scroll = self.tab_scroll.clamp(0.0, (total - w).max(0.0));
    }

    /// タブバーの中の x 座標 → (タブの番号, 閉じるボタンの上か)
    fn tab_at(&self, x: f32) -> Option<(usize, bool)> {
        let x = x + self.tab_scroll;
        let boxes = self.tab_boxes();
        let i = boxes.iter().position(|b| x >= b.x0 && x < b.x1)?;
        Some((i, x >= boxes[i].x1 - CLOSE_W))
    }

    /// タブバーのクリック。`close` なら（中ボタンなど）閉じる
    pub(super) fn tab_click(&mut self, x: f32, close: bool) -> Option<Deferred> {
        match self.tab_at(x) {
            Some((i, on_close)) => {
                self.switch_to(i);
                if (close || on_close) && self.active == i {
                    return self.guarded(Pending::CloseTab);
                }
                None
            }
            None => None,
        }
    }

    /// タブを押した：ドラッグで並べ替えを始める（閉じるボタンの上は除く）
    pub(super) fn tab_drag_start(&mut self, x: f32) {
        if let Some((i, false)) = self.tab_at(x) {
            self.tab_drag = Some((i, false));
            unsafe {
                SetCapture(self.hwnd);
            }
        }
    }

    /// ドラッグ中のタブを、マウスの下にあるタブの位置へ動かす
    pub(super) fn tab_drag_move(&mut self, x: f32) {
        let Some((from, _)) = self.tab_drag else { return };
        // 端にいるときは、はみ出した分だけ近いタブに合わせる
        let total = self.tab_boxes().last().map_or(0.0, |b| b.x1);
        let probe = (x + self.tab_scroll).clamp(0.0, (total - 1.0).max(0.0)) - self.tab_scroll;
        let Some((to, _)) = self.tab_at(probe) else { return };
        if to == from {
            return;
        }
        // 表示中のタブの状態は App 側にあり、tabs[active] は空の入れ物なので、並べ替えは番号だけでよい
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.active = if self.active == from {
            to
        } else if from < self.active && to >= self.active {
            self.active - 1
        } else if from > self.active && to <= self.active {
            self.active + 1
        } else {
            self.active
        };
        self.tab_drag = Some((to, true));
        self.invalidate();
    }

    /// 開いているタブのファイルの（正規化した）パス
    pub(super) fn open_paths(&self) -> Vec<PathBuf> {
        (0..self.tabs.len())
            .filter_map(|i| {
                let doc = if i == self.active { self.doc.as_ref() } else { self.tabs[i].doc.as_ref() };
                doc.and_then(|d| d.path()).and_then(|p| std::fs::canonicalize(p).ok())
            })
            .collect()
    }

    /// 開いているファイルを覚えておく（終了時）
    pub(super) fn save_session(&self) {
        let mut paths = Vec::new();
        let mut active = 0;
        for i in 0..self.tabs.len() {
            let doc = if i == self.active { self.doc.as_ref() } else { self.tabs[i].doc.as_ref() };
            if let Some(p) = doc.and_then(|d| d.path()) {
                if i == self.active {
                    active = paths.len();
                }
                paths.push(p.to_path_buf());
            }
        }
        crate::session::save(&paths, active);
    }

    /// 前回開いていたファイルを開く
    pub(super) fn restore_session(&mut self) {
        let (paths, active) = crate::session::load();
        let mut opened = 0;
        let mut target = None;
        for (i, p) in paths.into_iter().enumerate() {
            if !p.is_file() {
                continue;
            }
            if i == active {
                target = Some(opened);
            }
            self.open(p, None);
            opened += 1;
        }
        if let Some(t) = target {
            self.switch_to(t);
        }
        self.message.clear();
    }

    /// タブバーの何もない所をダブルクリックしたら新しいタブ
    pub(super) fn tab_double_click(&mut self, x: f32) {
        if self.tab_at(x).is_none() {
            self.new_tab();
        }
    }

    pub(super) unsafe fn draw_tabs(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let h = self.tab_h();
        let t = self.theme;
        unsafe {
            brush.SetColor(&t.gutter_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, h), brush);
            // 本文との境目（表示中のタブの下だけは塗りつぶして、本文とつながって見えるようにする）
            brush.SetColor(&t.dim);
            rt.FillRectangle(&rect(0.0, h - 1.0, w, h), brush);
            for (i, b) in self.tab_boxes().iter().enumerate() {
                let (x0, x1) = (b.x0 - self.tab_scroll, b.x1 - self.tab_scroll);
                if x1 < 0.0 || x0 > w {
                    continue;
                }
                let active = i == self.active;
                if active {
                    // 角を丸めたタブ（下は本文とつなげる）と、アクセントカラーの短い印
                    brush.SetColor(&t.bg);
                    rt.FillRoundedRectangle(&rrect(x0 + 2.0, 3.0, x1 - 2.0, h + 8.0, 7.0), brush);
                    rt.FillRectangle(&rect(x0 + 2.0, h - 6.0, x1 - 2.0, h), brush);
                    brush.SetColor(&theme::accent(self.settings.dark));
                    rt.FillRoundedRectangle(&rrect((x0 + x1) / 2.0 - 10.0, h - 3.0, (x0 + x1) / 2.0 + 10.0, h, 1.5), brush);
                } else if i + 1 != self.active {
                    // タブの境目（選択中のタブのとなりでは描かない）
                    brush.SetColor(&t.dim);
                    rt.FillRectangle(&rect(x1 - 1.0, 7.0, x1, h - 7.0), brush);
                }
                brush.SetColor(if active { &t.text } else { &t.gutter_text });
                self.draw_ui_str(rt, brush, &b.title, x0 + PAD, 4.0, x1 - CLOSE_W)?;
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, "x", x1 - CLOSE_W + 6.0, 4.0, x1 - 2.0)?;
            }
        }
        Ok(())
    }
}
