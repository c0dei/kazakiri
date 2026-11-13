//! 差分表示（検索 → 他のタブと比較）。いまのタブ（左）と、もう 1 つのタブ（右）を、行ごとに左右へ並べて見る。
//!
//! 違う行に色を付け、片方にしかない行は反対側を灰色で空ける。↑↓ PageUp/PageDown Home/End でスクロール、
//! F8 / Shift+F8 で次／前の違い、Esc で閉じる。読み取り専用（編集はもとのタブで）。
//! 行の対応づけは別スレッドで求める（kazakiri_core::diff）。

use kazakiri_core::diff::{self, Kind, Row};

use super::*;

pub(super) const WM_DIFF_DONE: u32 = WM_APP + 8;
/// 比較できる行数の上限（片方）
const MAX_LINES: u64 = 3_000_000;

pub(super) struct DiffView {
    a: Arc<Document>,
    b: Arc<Document>,
    a_title: String,
    b_title: String,
    rows: Vec<Row>,
    /// 違いの塊の先頭の行（rows の位置）
    changes: Vec<usize>,
    top: usize,
    /// 分割表示（同じファイルを左右 2 つの窓で見る）。右の窓の先頭行と、操作する窓（0 = 左、1 = 右）
    split: bool,
    top_b: usize,
    focus: usize,
}

pub(super) struct DiffDone {
    a: Arc<Document>,
    b: Arc<Document>,
    a_title: String,
    b_title: String,
    rows: std::result::Result<Vec<Row>, String>,
}

/// 行の文字列（タブは空白に、制御文字は空白に。長い行は 600 バイトまで）
fn line_text(doc: &Document, l: u64) -> String {
    let Some((s, e)) = doc.line_range(l) else { return String::new() };
    let raw = doc.read(s, e.min(s + 600));
    let t = doc.decode(&raw);
    t.trim_end_matches('\r').chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn diff_color(kind: Kind, left: bool, dark: bool) -> Option<D2D1_COLOR_F> {
    let (add, del, chg) = if dark { ((0x1E, 0x4A, 0x2A), (0x5A, 0x24, 0x24), (0x4A, 0x42, 0x1A)) } else { ((0xD4, 0xF4, 0xD4), (0xFF, 0xD6, 0xD6), (0xFF, 0xF0, 0xB8)) };
    let c = match (kind, left) {
        (Kind::Same, _) => return None,
        (Kind::Del, true) => del,
        (Kind::Ins, false) => add,
        (Kind::Chg, _) => chg,
        // 反対側（空き）は呼び出し側で灰色にする
        _ => return None,
    };
    Some(rgb(c.0, c.1, c.2))
}

impl App {
    /// 比較できるタブ（いまのタブ以外）
    pub(super) fn diff_candidates(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| i != self.active && self.tab_doc(i).is_some()).collect()
    }

    /// 他のタブが 1 つだけならそれと比べ、複数なら選ぶメニューを出す
    pub(super) fn diff_command(&mut self) -> Option<Deferred> {
        let c = self.diff_candidates();
        match c.len() {
            0 => {
                self.message = "比較するには、もう 1 つのファイルをタブで開いてください".into();
                self.invalidate();
                None
            }
            1 => {
                self.start_diff(c[0]);
                None
            }
            _ => Some(Deferred::DiffMenu),
        }
    }

    /// 選ぶメニュー（使い終わったら呼び出し側が DestroyMenu する）
    pub(super) unsafe fn build_diff_menu(&self) -> Option<HMENU> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            for i in self.diff_candidates() {
                let w = wide(&format!("{} と比較", self.tab_title(i).replace('&', "&&")));
                let _ = AppendMenuW(m, MF_STRING, (menu::ID_DIFF_TAB_BASE + i as u32) as usize, PCWSTR(w.as_ptr()));
            }
            Some(m)
        }
    }

    pub(super) fn start_diff(&mut self, other: usize) {
        let (Some(a), Some(b)) = (self.doc.clone(), self.tab_doc(other)) else { return };
        if a.line_count().0 > MAX_LINES || b.line_count().0 > MAX_LINES {
            self.message = "行数が多すぎて比較できません（300 万行まで）".into();
            return self.invalidate();
        }
        let a_title = self.tab_title(self.active);
        let b_title = self.tab_title(other);
        self.message = "差分を計算しています…".into();
        self.invalidate();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let (ha, hb) = (diff::line_hashes(&a), diff::line_hashes(&b));
            let rows = diff::diff(&ha, &hb).ok_or_else(|| format!("違いが多すぎて比較できません（{} 行以上）", diff::MAX_D));
            let done = Box::into_raw(Box::new(DiffDone { a, b, a_title, b_title, rows }));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_DIFF_DONE, WPARAM(0), LPARAM(done as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(done) });
            }
        });
    }

    pub(super) fn on_diff_done(&mut self, d: DiffDone) {
        let rows = match d.rows {
            Ok(r) => r,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        let mut changes = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            if r.kind != Kind::Same && (i == 0 || rows[i - 1].kind == Kind::Same) {
                changes.push(i);
            }
        }
        if changes.is_empty() {
            self.message = "2 つのファイルは同じです".into();
            return self.invalidate();
        }
        let first = changes[0].saturating_sub(2);
        self.message = format!("差分: {} か所の違い。F8 で次、Shift+F8 で前、Esc で閉じる", changes.len());
        self.diffview = Some(DiffView { a: d.a, b: d.b, a_title: d.a_title, b_title: d.b_title, rows, changes, top: first, split: false, top_b: 0, focus: 0 });
        self.invalidate();
    }

    /// 分割表示：同じファイルを左右 2 つの窓で見る（もう一度で閉じる）
    /// 分割表示（同じファイルを左右 2 窓）を開いているか
    pub(super) fn is_split_view(&self) -> bool {
        self.diffview.as_ref().is_some_and(|v| v.split)
    }

    pub(super) fn toggle_split_view(&mut self) {
        if self.diffview.as_ref().is_some_and(|v| v.split) {
            self.diffview = None;
            self.message = "分割表示を閉じました".into();
            return self.invalidate();
        }
        let Some(doc) = self.doc.clone() else { return };
        let title = self.tab_title(self.active);
        let top = self.top as usize;
        self.diffview = Some(DiffView { a: doc.clone(), b: doc, a_title: title.clone(), b_title: title, rows: Vec::new(), changes: Vec::new(), top, split: true, top_b: top, focus: 1 });
        self.message = "分割表示: 窓をクリックして選び、↑↓ PageUp/PageDown Home/End・ホイールで動かす。Esc で閉じる".into();
        self.invalidate();
    }

    /// 窓を選ぶ（x は本文の座標）
    pub(super) fn diff_focus_at(&mut self, x: f32) {
        let w = self.client_size().0;
        if let Some(v) = &mut self.diffview {
            if v.split {
                v.focus = usize::from(x >= w / 2.0);
                self.invalidate();
            }
        }
    }

    /// ホイール：分割表示ではマウスのある窓を動かす
    pub(super) fn diff_wheel(&mut self, x: f32, delta: i64) {
        self.diff_focus_at(x);
        self.diff_scroll(delta);
    }

    fn diff_total(&self) -> usize {
        match &self.diffview {
            Some(v) if v.split => v.a.line_count().0 as usize,
            Some(v) => v.rows.len(),
            None => 0,
        }
    }

    fn diff_rows_visible(&self) -> usize {
        (((self.client_size().1 - self.status_h() - self.line_h - 6.0) / self.line_h).floor() as usize).max(1)
    }

    pub(super) fn diff_scroll(&mut self, delta: i64) {
        let vis = self.diff_rows_visible();
        let total = self.diff_total();
        if let Some(v) = &mut self.diffview {
            let max = total.saturating_sub(vis) as i64;
            let slot = if v.split && v.focus == 1 { &mut v.top_b } else { &mut v.top };
            *slot = (*slot as i64 + delta).clamp(0, max) as usize;
            self.invalidate();
        }
    }

    /// 次（前）の違いへ
    fn diff_jump(&mut self, forward: bool) {
        let vis = self.diff_rows_visible();
        let Some(v) = &mut self.diffview else { return };
        let here = v.top + 2;
        let target = if forward { v.changes.iter().find(|&&c| c > here).or(v.changes.first()) } else { v.changes.iter().rev().find(|&&c| c < here).or(v.changes.last()) };
        if let Some(&c) = target {
            v.top = c.saturating_sub(2).min(v.rows.len().saturating_sub(vis));
            let n = v.changes.iter().position(|&x| x == c).unwrap_or(0) + 1;
            self.message = format!("違い {n} / {}", v.changes.len());
        }
        self.invalidate();
    }

    /// 差分表示中のキー。処理したら true（false なら通常の処理へ）
    pub(super) fn diff_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        if self.diffview.is_none() {
            return false;
        }
        let page = self.diff_rows_visible() as i64;
        match vk {
            VK_ESCAPE => {
                self.diffview = None;
                self.message = "差分表示を閉じました".into();
                self.invalidate();
            }
            VK_UP => self.diff_scroll(-1),
            VK_DOWN => self.diff_scroll(1),
            VK_PRIOR => self.diff_scroll(-page),
            VK_NEXT => self.diff_scroll(page),
            VK_HOME => self.diff_scroll(i64::MIN / 2),
            VK_END => self.diff_scroll(i64::MAX / 2),
            VK_F8 if self.diffview.as_ref().is_some_and(|v| !v.split) => self.diff_jump(!shift),
            // Ctrl 付きのキー（タブの切り替えなど）は通常どおり
            _ if ctrl => return false,
            _ => {}
        }
        true
    }

    // ---- 描画 ----

    pub(super) unsafe fn draw_diffview(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, body_h: f32) -> Result<()> {
        let Some(v) = &self.diffview else { return Ok(()) };
        let t = self.theme;
        let dark = self.settings.dark;
        let head = self.line_h + 6.0;
        let pw = ((w - 2.0) / 2.0).floor();
        let digits = group(v.a.line_count().0.max(v.b.line_count().0)).len().max(3);
        let num_w = self.char_w * (digits as f32 + 1.5);
        unsafe {
            // 見出し
            for (i, title) in [&v.a_title, &v.b_title].iter().enumerate() {
                let x0 = i as f32 * (pw + 2.0);
                brush.SetColor(&t.gutter_bg);
                rt.FillRectangle(&rect(x0, 0.0, x0 + pw, head), brush);
                brush.SetColor(&t.text);
                self.draw_str(rt, brush, &format!("{}  {title}", if i == 0 { "A" } else { "B" }), x0 + 8.0, 3.0, x0 + pw - 4.0)?;
            }
            brush.SetColor(&t.dim);
            rt.FillRectangle(&rect(pw, 0.0, pw + 2.0, body_h), brush);
            let vis = ((body_h - head) / self.line_h).ceil() as usize;
            if v.split {
                // 選んでいる窓の見出しに印
                brush.SetColor(&theme::accent(dark));
                let fx = v.focus as f32 * (pw + 2.0);
                rt.FillRectangle(&rect(fx, head - 2.0, fx + pw, head), brush);
                for (side, doc) in [&v.a, &v.b].into_iter().enumerate() {
                    let x0 = side as f32 * (pw + 2.0);
                    let top = if side == 0 { v.top } else { v.top_b };
                    rt.PushAxisAlignedClip(&rect(x0, head, x0 + pw, body_h), D2D1_ANTIALIAS_MODE_ALIASED);
                    for n in 0..vis {
                        let l = (top + n) as u64;
                        if l >= doc.line_count().0 {
                            break;
                        }
                        let y = head + n as f32 * self.line_h;
                        brush.SetColor(&t.gutter_text);
                        self.draw_str(rt, brush, &format!("{:>width$}", l + 1, width = digits), x0 + self.char_w * 0.5, y, x0 + num_w)?;
                        brush.SetColor(&t.text);
                        self.draw_str(rt, brush, &line_text(doc, l), x0 + num_w + 4.0, y, x0 + pw)?;
                    }
                    rt.PopAxisAlignedClip();
                }
                return Ok(());
            }
            for (n, row) in v.rows.iter().skip(v.top).take(vis).enumerate() {
                let y = head + n as f32 * self.line_h;
                for (side, (doc, line)) in [(&v.a, row.a), (&v.b, row.b)].into_iter().enumerate() {
                    let x0 = side as f32 * (pw + 2.0);
                    let left = side == 0;
                    rt.PushAxisAlignedClip(&rect(x0, y, x0 + pw, y + self.line_h), D2D1_ANTIALIAS_MODE_ALIASED);
                    match (line, diff_color(row.kind, left, dark)) {
                        (None, _) => {
                            brush.SetColor(&t.panel_bg);
                            rt.FillRectangle(&rect(x0, y, x0 + pw, y + self.line_h), brush);
                        }
                        (Some(_), Some(c)) => {
                            brush.SetColor(&c);
                            rt.FillRectangle(&rect(x0, y, x0 + pw, y + self.line_h), brush);
                        }
                        _ => {}
                    }
                    if let Some(l) = line {
                        brush.SetColor(&t.gutter_text);
                        self.draw_str(rt, brush, &format!("{:>width$}", l + 1, width = digits), x0 + self.char_w * 0.5, y, x0 + num_w)?;
                        brush.SetColor(&t.text);
                        self.draw_str(rt, brush, &line_text(doc, l), x0 + num_w + 4.0, y, x0 + pw)?;
                    }
                    rt.PopAxisAlignedClip();
                }
            }
        }
        Ok(())
    }
}
