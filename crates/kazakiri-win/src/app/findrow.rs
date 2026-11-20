//! 検索・置換の行（本文の上に固定。標準の入力欄とボタン）。
//! 行へ移動・列フィルタ・時刻やログレベルの絞り込みなどの入力も、同じ行に出す（題・入力欄・実行・閉じる、その下に説明）。カーソル移動・範囲選択・元に戻す（Ctrl+Z）・日本語入力は標準の入力欄が受ける。
//!
//! モードバーの下に出して、本文を隠さない。1 行目は検索（入力欄・Aa・.*・単語・前へ・次へ・閉じる）、置換のときは 2 行目（入力欄・置換・すべて置換）。
//! 入力の中身（検索語・置換語）は、これまでどおり `App` の `input` / `input2` が持ち、標準の入力欄とは、変わったときに写し合う。
//! 日本語入力は標準の入力欄が受ける。Enter・Esc・Tab・F3・Ctrl+… は、入力欄のサブクラスで受けて、アプリの操作につなぐ。
//! 色は、標準の部品がダークにならないので、`WM_CTLCOLOR*` と `SetWindowTheme`（入力欄は `DarkMode_CFD`、ボタンは `DarkMode_Explorer`）で合わせる。

use std::cell::Cell;

use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};

use super::find_dialog::{make_child, rgb_ref, wide};
use super::*;

const ID_LABEL: i32 = 9100;
const ID_FIND: i32 = 9101;
const ID_CASE: i32 = 9102;
const ID_REGEX: i32 = 9103;
const ID_WORD: i32 = 9104;
const ID_PREV: i32 = 9105;
const ID_NEXT: i32 = 9106;
const ID_CLOSE: i32 = 9107;
const ID_RLABEL: i32 = 9110;
const ID_REPL: i32 = 9111;
const ID_ONE: i32 = 9112;
const ID_ALL: i32 = 9113;
const ID_SEL: i32 = 9114;
const ID_GO: i32 = 9115;

/// 1 行の高さ（DIP）
pub(super) const ROW_H: f32 = 36.0;

pub(super) fn is_findrow_id(id: i32) -> bool {
    (ID_LABEL..=ID_GO).contains(&id)
}

/// 作った部品
pub(super) struct FindRow {
    all: Vec<(i32, HWND)>,
}

impl FindRow {
    fn h(&self, id: i32) -> HWND {
        self.all.iter().find(|(i, _)| *i == id).map(|(_, h)| *h).unwrap_or_default()
    }
}

/// 色（`WM_CTLCOLOR*` は、描画の最中に App を借りたまま届くことがあるので、App を借りずに答えられるよう、ここに持つ）
#[derive(Clone, Copy)]
struct Colors {
    row: HBRUSH,
    edit: HBRUSH,
    text: COLORREF,
    row_c: COLORREF,
    edit_c: COLORREF,
    /// ボタンの枠、選択中の枠（アクセントカラー）、選択中・押したときの塗り
    line: COLORREF,
    accent: COLORREF,
    on_fill: COLORREF,
}

thread_local! {
    static COLORS: Cell<Option<Colors>> = const { Cell::new(None) };
    /// 切り替えボタン（Aa・.*・単語）の、いまの状態
    static FLAGS: Cell<[bool; 3]> = const { Cell::new([false; 3]) };
}

/// ボタン（自前描画の標準ボタン）を描く。角は設定（corner）で四角／角丸。モードバーのボタンと同じ見た目
pub(super) unsafe fn draw_button(di: &DRAWITEMSTRUCT) {
    let Some(c) = COLORS.with(|c| c.get()) else { return };
    unsafe {
        let hdc = di.hDC;
        let id = di.CtlID as i32;
        let on = match id {
            ID_CASE => FLAGS.with(|f| f.get()[0]),
            ID_REGEX => FLAGS.with(|f| f.get()[1]),
            ID_WORD => FLAGS.with(|f| f.get()[2]),
            _ => false,
        };
        let pressed = di.itemState.0 & ODS_SELECTED.0 != 0;
        let rc = di.rcItem;
        FillRect(hdc, &rc, c.row);
        let fill = CreateSolidBrush(if on || pressed { c.on_fill } else { c.row_c });
        let pen = CreatePen(PS_SOLID, 1, if on { c.accent } else { c.line });
        let (old_b, old_p) = (SelectObject(hdc, fill.into()), SelectObject(hdc, pen.into()));
        if super::util::corner_round() {
            let r = (8.0 * GetDpiForWindow(di.hwndItem).max(96) as f32 / 96.0) as i32;
            let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, r, r);
        } else {
            let _ = Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom);
        }
        SelectObject(hdc, old_b);
        SelectObject(hdc, old_p);
        let _ = DeleteObject(fill.into());
        let _ = DeleteObject(pen.into());
        // 題
        let mut buf = [0u16; 64];
        let n = GetWindowTextW(di.hwndItem, &mut buf) as usize;
        let old_f = SelectObject(hdc, HGDIOBJ(SendMessageW(di.hwndItem, WM_GETFONT, None, None).0 as *mut _));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, c.text);
        let mut r = rc;
        DrawTextW(hdc, &mut buf[..n], &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old_f);
        if di.itemState.0 & ODS_FOCUS.0 != 0 && di.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
            let f = RECT { left: rc.left + 3, top: rc.top + 3, right: rc.right - 3, bottom: rc.bottom - 3 };
            let _ = DrawFocusRect(hdc, &f);
        }
    }
}

/// `wndproc` の先頭から、App を借りる前に呼ぶ。検索・置換の行の部品の色を答える
pub(super) unsafe fn ctl_color(main: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN) {
        return None;
    }
    let c = COLORS.with(|c| c.get())?;
    unsafe {
        if GetParent(HWND(lparam.0 as *mut _)).ok() != Some(main) {
            return None;
        }
        let hdc = HDC(wparam.0 as *mut _);
        SetTextColor(hdc, c.text);
        if msg == WM_CTLCOLOREDIT {
            SetBkColor(hdc, c.edit_c);
            Some(LRESULT(c.edit.0 as isize))
        } else {
            SetBkMode(hdc, OPAQUE);
            SetBkColor(hdc, c.row_c);
            Some(LRESULT(c.row.0 as isize))
        }
    }
}

impl App {
    /// 検索・置換の行の高さ（DIP。出していなければ 0）
    pub(super) fn find_h(&self) -> f32 {
        match self.prompt {
            Some(Prompt::Find) => ROW_H,
            Some(Prompt::Replace) => ROW_H * 2.0,
            None | Some(Prompt::Palette) => 0.0,
            // 入力の行 + （列フィルタは）チップの行 + 説明
            Some(_) => ROW_H + self.filter_chip_row_h() + self.prompt_hint_h(),
        }
    }

    /// 入力の行（検索・置換以外）の題・説明・実行ボタンの題。`cols` が false のときは、列名を入れない（高さの見積もり用。呼ばれる回数が多いので）
    pub(super) fn prompt_texts(&self, cols: bool) -> (String, String, &'static str) {
        let (t, h, go): (&str, String, &'static str) = match self.prompt {
            Some(Prompt::GotoLine) => ("行へ移動", "行番号を入力して Enter".into(), "移動"),
            Some(Prompt::LineFilter) => ("行フィルター", format!("検索語を入力して Enter。一致した行だけを新しいタブに出します（{}{}）", if self.opts.case_sensitive { "大文字小文字を区別" } else { "大文字小文字を区別しない" }, if self.opts.regex { "・正規表現" } else { "" }), "抽出"),
            Some(Prompt::TimeFilter) => ("時刻の範囲で絞り込み", "開始 ～ 終了（例: 2026-11-18 12:00 ～ 13:30、日付だけならその日全体）を入力して Enter。範囲に入る行を一覧に出します。「ERROR @ 2026-11-18 12:00 ～ 13:00」のようにレベルと組み合わせられます".into(), "絞り込み"),
            Some(Prompt::TimeExtract) => ("時刻の範囲を抽出", "開始 ～ 終了（例: 2026-11-18 12:00 ～ 13:30）を入力して Enter。範囲に入る行を新しいタブに出します".into(), "抽出"),
            Some(Prompt::MacroRepeat) => ("マクロを繰り返し再生", "回数を入力して Enter（0 は、検索で見つからなくなるまで。Esc キーで再生を中止できます）".into(), "再生"),
            Some(Prompt::LevelFilter) => ("ログレベルで絞り込み", "レベル（例: ERROR,WARN / >=WARN）を入力して Enter。該当する行（レベルのない続きの行も）を一覧に出します。「ERROR @ 2026-11-18 12:00 ～ 13:00」のように時刻の範囲と組み合わせられます".into(), "絞り込み"),
            Some(Prompt::LevelExtract) => ("ログレベルで抽出", "レベル（例: ERROR,WARN / >=WARN）を入力して Enter。該当する行を新しいタブに出します".into(), "抽出"),
            Some(Prompt::TimeGoto) => ("時刻へ移動", "時刻（例: 2026-11-18 12:30:00）を入力して Enter。時刻順のログとして、その時刻以降の最初の行へ移ります".into(), "移動"),
            Some(Prompt::Sequence) => ("連番を挿入", "開始値,増分,桁数（例: 1,1,3 → 001, 002…）を入力して Enter".into(), "挿入"),
            Some(Prompt::Split) => ("ファイルを分割", "1 ファイルの行数、またはサイズ（例: 1000000 / 512MB / 2GB）を入力して Enter".into(), "分割"),
            Some(Prompt::ColumnFilter) => {
                let names: Vec<String> = if cols { self.csv_column_names(12).iter().enumerate().map(|(i, n)| format!("{}:{n}", i + 1)).collect() } else { Vec::new() };
                let cols = if names.is_empty() { String::new() } else { format!("　列 {}", names.join("  ")) };
                ("列フィルタ", format!("「列番号 演算子 値」の形で、演算子は下のチップでも選べます（例: 3 = ERROR / 4 >= 100）。Enter で絞り込み{cols}"), "絞り込み")
            }
            _ => ("", String::new(), ""),
        };
        (t.to_string(), h, go)
    }

    /// 説明の高さ（DIP）。文字の幅と窓の幅から行数（1〜4）を決める。`find_h` から何度も呼ばれる（`client_size` の中からも）ので、条件が変わったときだけ測る
    fn prompt_hint_h(&self) -> f32 {
        let Some(prompt) = self.prompt else { return 0.0 };
        let hint = self.prompt_texts(false).1;
        let width = self.client_w_dip();
        let key = (prompt, hint.len(), (width * 10.0) as i32);
        if let Some((k, h)) = self.hint_h_cache.get() {
            if k == key {
                return h;
            }
        }
        let units: Vec<u16> = hint.encode_utf16().collect();
        let avail = (width - 24.0).max(1.0);
        // 折り返しの分、少し余裕を見る。列フィルタは、列名が付くので少なくとも 2 行分
        let lines = unsafe { self.ui_text_width(&units) }.map_or(2, |w| ((w * 1.04 / avail).ceil() as usize).clamp(1, 4));
        let lines = if prompt == Prompt::ColumnFilter { lines.max(2) } else { lines };
        let h = lines as f32 * self.ui_line_h + 10.0;
        self.hint_h_cache.set(Some((key, h)));
        h
    }

    /// 入力欄の中身の取得
    unsafe fn edit_text(h: HWND) -> String {
        let mut buf = vec![0u16; 2048];
        let n = unsafe { GetWindowTextW(h, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
    }

    unsafe fn ensure_findrow(&self) {
        if self.findrow.borrow().is_some() {
            return;
        }
        unsafe {
            if self.findrow_font.get().is_invalid() {
                self.findrow_font.set(self.ui_hfont());
            }
            let font = self.findrow_font.get();
            // 入力欄は枠なし（枠は背景に自前で描く。角の四角／角丸を設定で切り替えるため）。ボタンは自前描画（標準のボタンは角が OS 任せ）
            let edit = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
            let toggle = BS_OWNERDRAW as u32 | WS_TABSTOP.0;
            let push = BS_OWNERDRAW as u32 | WS_TABSTOP.0;
            let mk = |class: PCWSTR, text: &str, style: u32, id: i32| (id, make_child(self.hwnd, class, text, style, 0, id, font));
            let all = vec![
                mk(w!("STATIC"), "検索", 0x200 /* SS_CENTERIMAGE */, ID_LABEL),
                mk(w!("EDIT"), "", edit, ID_FIND),
                mk(w!("BUTTON"), "Aa", toggle, ID_CASE),
                mk(w!("BUTTON"), ".*", toggle, ID_REGEX),
                mk(w!("BUTTON"), "単語", toggle, ID_WORD),
                mk(w!("BUTTON"), "↑ 前へ", push, ID_PREV),
                mk(w!("BUTTON"), "↓ 次へ", push, ID_NEXT),
                mk(w!("BUTTON"), "✕", push, ID_CLOSE),
                mk(w!("STATIC"), "置換", 0x200 /* SS_CENTERIMAGE */, ID_RLABEL),
                mk(w!("EDIT"), "", edit, ID_REPL),
                mk(w!("BUTTON"), "置換", push, ID_ONE),
                mk(w!("BUTTON"), "すべて置換", push, ID_ALL),
                mk(w!("BUTTON"), "選択範囲を置換", push, ID_SEL),
                mk(w!("BUTTON"), "実行", push, ID_GO),
            ];
            for (id, h) in &all {
                if *id == ID_FIND {
                    let _ = SetWindowSubclass(*h, Some(edit_proc), 1, 0);
                } else if *id == ID_REPL {
                    let _ = SetWindowSubclass(*h, Some(edit_proc), 1, 1);
                } else if (ID_CASE..=ID_CLOSE).contains(id) || (ID_ONE..=ID_GO).contains(id) {
                    let _ = SetWindowSubclass(*h, Some(button_proc), 2, 0);
                }
            }
            *self.findrow.borrow_mut() = Some(FindRow { all });
            self.findrow_theme.set(None);
        }
    }

    /// 色のブラシを、いまのテーマに合わせて作り直す（変わったときだけ）
    unsafe fn update_findrow_colors(&self) {
        let (row_c, edit_c, text) = (rgb_ref(&self.theme.toolbar_bg), if self.settings.dark { COLORREF(0x001f_1f1f) } else { COLORREF(0x00ff_ffff) }, rgb_ref(&self.theme.text));
        let acc = theme::accent(self.settings.dark);
        let (line, accent) = (rgb_ref(&self.theme.dim), rgb_ref(&acc));
        // 選択中の塗り：アクセントカラーを、行の色に 22% 混ぜる
        let mixc = |a: f32, b: f32| ((a * 0.22 + b * 0.78) * 255.0).round() as u32;
        let on_fill = COLORREF(mixc(acc.r, self.theme.toolbar_bg.r) | (mixc(acc.g, self.theme.toolbar_bg.g) << 8) | (mixc(acc.b, self.theme.toolbar_bg.b) << 16));
        unsafe {
            let old = COLORS.with(|c| c.get());
            if old.is_some_and(|o| o.row_c == row_c && o.edit_c == edit_c && o.text == text && o.line == line && o.accent == accent && o.on_fill == on_fill) {
                return;
            }
            if let Some(o) = old {
                let _ = DeleteObject(o.row.into());
                let _ = DeleteObject(o.edit.into());
            }
            COLORS.with(|c| c.set(Some(Colors { row: CreateSolidBrush(row_c), edit: CreateSolidBrush(edit_c), text, row_c, edit_c, line, accent, on_fill })));
        }
    }

    /// 検索・置換の行を、いまの状態に合わせて出す・隠す・置き直す・中身を写す。描画のたびと、バーを開いたときに呼ぶ
    pub(super) fn layout_findrow(&self) {
        let want = self.find_h() > 0.0;
        unsafe {
            if want {
                self.ensure_findrow();
            }
            let guard = self.findrow.borrow();
            let Some(fr) = guard.as_ref() else { return };
            if !want {
                if self.findrow_shown.get() {
                    for (_, h) in &fr.all {
                        let _ = ShowWindow(*h, SW_HIDE);
                    }
                    self.findrow_shown.set(false);
                    self.findrow_placed.set([0; 5]);
                }
                return;
            }
            self.update_findrow_colors();
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let replace = self.prompt == Some(Prompt::Replace);
            // 検索・置換以外の入力（行へ移動など）：題の幅の分だけ入力欄を右へ
            let generic = !self.is_find_bar();
            let (title, _, go) = self.prompt_texts(false);
            let label_w = if generic { self.ui_text_width(&title.encode_utf16().collect::<Vec<u16>>()).unwrap_or(80.0) + 4.0 } else { 36.0 };
            let y0 = ((self.tab_h() + self.tb_h()) * scale).round() as i32;
            let placed = [rc.right, y0, i32::from(replace) | (i32::from(generic) << 1) | (((label_w * 10.0) as i32) << 2), (scale * 100.0) as i32, (self.find_h() * 10.0) as i32];
            let first = !self.findrow_shown.get();
            if first || self.findrow_placed.get() != placed {
                self.findrow_placed.set(placed);
                let s = |v: f32| (v * scale).round() as i32;
                let (row_h, ih, pad) = (s(ROW_H), s(26.0), s(5.0));
                let place = |id: i32, x: f32, w: f32, row: i32| {
                    let _ = SetWindowPos(fr.h(id), None, s(x), y0 + row * row_h + pad, s(w), ih, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                };
                let w = rc.right as f32 / scale;
                let edit_w = if generic { (w * 0.4).clamp(200.0, 480.0) } else { (w * 0.28).clamp(160.0, 340.0) };
                let ex = if generic { 12.0 + label_w + 8.0 } else { 52.0 };
                // 窓が狭いときは、右のボタン（実行・✕、検索行ならさらに切り替えと前へ・次へ）が画面の外に出ないよう、入力欄を縮める
                let reserve = if generic { 84.0 + 4.0 + 8.0 + 30.0 + 8.0 } else { 322.0 + 8.0 };
                let edit_w = edit_w.min((w - ex - 8.0 - reserve).max(80.0));
                place(ID_LABEL, 12.0, label_w, 0);
                // 入力欄：枠（背景に自前で描く）の内側に、文字の高さで置く
                // 標準の入力欄は文字を上から描くので、高さは実際の文字の高さ（GDI）にして、枠の中央に置く
                let th = {
                    let hdc = GetDC(Some(self.hwnd));
                    let old = SelectObject(hdc, HGDIOBJ(self.findrow_font.get().0));
                    let mut tm = TEXTMETRICW::default();
                    let _ = GetTextMetricsW(hdc, &mut tm);
                    SelectObject(hdc, old);
                    ReleaseDC(Some(self.hwnd), hdc);
                    if tm.tmHeight > 0 { tm.tmHeight as f32 / scale } else { self.ui_line_h }
                };
                let place_edit = |id: i32, row: i32| {
                    let ty = 5.0 + (26.0 - th) / 2.0;
                    let _ = SetWindowPos(fr.h(id), None, s(ex + 6.0), y0 + row * row_h + s(ty), s(edit_w - 12.0), s(th), SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                };
                place_edit(ID_FIND, 0);
                let mut x = ex + edit_w + 8.0;
                if generic {
                    for id in [ID_CASE, ID_REGEX, ID_WORD, ID_PREV, ID_NEXT] {
                        let _ = ShowWindow(fr.h(id), SW_HIDE);
                    }
                    place(ID_GO, x, 84.0, 0);
                    x += 84.0 + 4.0;
                } else {
                    let _ = ShowWindow(fr.h(ID_GO), SW_HIDE);
                    for (id, bw) in [(ID_CASE, 34.0), (ID_REGEX, 34.0), (ID_WORD, 50.0)] {
                        place(id, x, bw, 0);
                        x += bw + 4.0;
                    }
                    x += 6.0;
                    for (id, bw) in [(ID_PREV, 70.0), (ID_NEXT, 70.0)] {
                        place(id, x, bw, 0);
                        x += bw + 4.0;
                    }
                }
                place(ID_CLOSE, (w - 8.0 - 30.0).max(x), 30.0, 0);
                if replace {
                    place(ID_RLABEL, 12.0, 36.0, 1);
                    place_edit(ID_REPL, 1);
                    place(ID_ONE, ex + edit_w + 8.0, 56.0, 1);
                    place(ID_ALL, ex + edit_w + 8.0 + 56.0 + 4.0, 92.0, 1);
                    place(ID_SEL, ex + edit_w + 8.0 + 56.0 + 4.0 + 92.0 + 4.0, 116.0, 1);
                } else {
                    for id in [ID_RLABEL, ID_REPL, ID_ONE, ID_ALL, ID_SEL] {
                        let _ = ShowWindow(fr.h(id), SW_HIDE);
                    }
                }
                self.findrow_frames.set([(ex, 5.0, ex + edit_w, 31.0), (ex, 5.0 + ROW_H, ex + edit_w, 31.0 + ROW_H)]);
                self.findrow_shown.set(true);
            }
            // 題と実行ボタンの文字（入力の種類で変わる）
            for (id, text) in [(ID_LABEL, if generic { title.as_str() } else { "検索" }), (ID_GO, go)] {
                let h = fr.h(id);
                if Self::edit_text(h) != text {
                    let t = wide(text);
                    let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
                    let _ = InvalidateRect(Some(h), None, true);
                }
            }
            // 中身を写す（入力欄は、違うときだけ。入力中の文字をこちらで書き換えて、カーソルが動かないように）
            for (id, text) in [(ID_FIND, &self.input), (ID_REPL, &self.input2)] {
                let h = fr.h(id);
                if Self::edit_text(h) != *text {
                    let t = wide(text);
                    let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
                }
            }
            let flags = [self.opts.case_sensitive, self.opts.regex, self.opts.whole_word];
            if FLAGS.with(|f| f.get()) != flags {
                FLAGS.with(|f| f.set(flags));
                for id in [ID_CASE, ID_REGEX, ID_WORD] {
                    let _ = InvalidateRect(Some(fr.h(id)), None, false);
                }
            }
            // 開いた直後（Ctrl+F など）：入力欄に入って、全選択
            let focus = self.findrow_focus.take();
            if focus != 0 {
                let target = if replace && self.prompt_focus == 1 { fr.h(ID_REPL) } else { fr.h(ID_FIND) };
                let _ = SetFocus(Some(target));
                // 1 = 全選択、2 = 末尾にカーソル（列フィルタの「3 : 」の続きを打つときなど）
                if focus == 2 || self.prompt == Some(Prompt::ColumnFilter) {
                    SendMessageW(target, EM_SETSEL, Some(WPARAM(i32::MAX as usize)), Some(LPARAM(i32::MAX as isize)));
                } else {
                    SendMessageW(target, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
        }
    }

    /// 開く・入力欄へ戻るときに、入力欄に入って全選択するよう頼む
    pub(super) fn focus_findrow(&self) {
        self.panel_focus.set(false);
        self.findrow_focus.set(1);
        self.layout_findrow();
    }

    /// 入力欄に入って、末尾にカーソルを置く（演算子のチップを押したあと）
    pub(super) fn focus_findrow_end(&self) {
        self.findrow_focus.set(2);
        self.layout_findrow();
    }

    /// 検索・置換の行を閉じる
    pub(super) fn close_find_bar(&mut self) {
        self.prompt = None;
        self.comp = None;
        self.layout_findrow();
        self.invalidate();
    }

    /// 検索・置換の行の部品からの通知（WM_COMMAND）
    pub(super) fn findrow_command(&mut self, id: i32, code: u32) -> Option<Deferred> {
        // 自前描画のボタンは、素早い 2 回目のクリックが「ダブルクリック」（BN_DOUBLECLICKED = 5）として届く。
        // これを無視すると、連打したときに 2 回に 1 回が落ちる。ふつうのクリックと同じに扱う
        let code = if code == 5 && !matches!(id, ID_FIND | ID_REPL) { 0 } else { code };
        match (id, code) {
            // 入力欄の文字が変わった：検索語・置換語に写す
            (ID_FIND | ID_REPL, 0x0300) => {
                let h = self.findrow.borrow().as_ref().map(|f| f.h(id)).unwrap_or_default();
                let text = unsafe { Self::edit_text(h) };
                if id == ID_FIND {
                    self.input = text;
                } else {
                    self.input2 = text;
                }
                self.find_bar_changed();
            }
            // 実行ボタン（行へ移動・絞り込みなど）
            (ID_GO, 0) => {
                let d = self.submit_prompt();
                // 閉じたら本文へ、開いたままなら入力欄へ戻る
                let back = if self.prompt.is_none() { self.hwnd } else { self.findrow.borrow().as_ref().map(|f| f.h(ID_FIND)).unwrap_or(self.hwnd) };
                unsafe {
                    let _ = SetFocus(Some(back));
                }
                return d;
            }
            // 入力欄の枠の色（入力中はアクセントカラー）のために、フォーカスの出入りで描き直す
            (ID_FIND | ID_REPL, 0x0100) => {
                self.findrow_focus_id.set(id);
                self.invalidate();
                return None;
            }
            (ID_FIND | ID_REPL, 0x0200) => {
                if self.findrow_focus_id.get() == id {
                    self.findrow_focus_id.set(0);
                }
                self.invalidate();
                return None;
            }
            (ID_CASE, 0) => self.find_bar_do(FindAct::Case),
            (ID_REGEX, 0) => self.find_bar_do(FindAct::Regex),
            (ID_WORD, 0) => self.find_bar_do(FindAct::Word),
            (ID_PREV, 0) => self.find_bar_do(FindAct::Prev),
            (ID_NEXT, 0) => self.find_bar_do(FindAct::Next),
            (ID_ONE, 0) => self.find_bar_do(FindAct::ReplOne),
            (ID_ALL, 0) => self.find_bar_do(FindAct::ReplAll),
            (ID_SEL, 0) => self.find_bar_do(FindAct::ReplSel),
            (ID_CLOSE, 0) => {
                self.close_find_bar();
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
                return None;
            }
            _ => return None,
        }
        // ボタンを押したあとは、入力欄に戻る（続けて Enter で次を探せるように）
        if code == 0 && id != ID_FIND && id != ID_REPL {
            let h = self.findrow.borrow().as_ref().map(|f| f.h(ID_FIND)).unwrap_or_default();
            if self.is_find_bar() && !h.0.is_null() {
                unsafe {
                    let _ = SetFocus(Some(h));
                }
            }
        }
        None
    }

    /// 検索・置換の行の背景（ツールバーと同じ色。下に細い線）。部品はこの上に重なる
    pub(super) unsafe fn draw_findrow_bg(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) {
        let h = self.find_h();
        if h <= 0.0 {
            return;
        }
        unsafe {
            brush.SetColor(&self.theme.toolbar_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, h), brush);
            brush.SetColor(&self.theme.dim);
            rt.FillRectangle(&rect(0.0, h - 1.0, w, h), brush);
            // 入力欄の枠（四角／角丸は設定）。入力中の欄は、アクセントカラーの枠
            let rows = if self.prompt == Some(Prompt::Replace) { 2 } else { 1 };
            // 検索・置換以外：チップ（列フィルタ）と、その下に説明
            if !self.is_find_bar() {
                if let Ok(b) = rt.CreateSolidColorBrush(&self.theme.gutter_text, None) {
                    let _ = self.draw_filter_chips(rt, &b);
                    let hint: Vec<u16> = self.prompt_texts(true).1.encode_utf16().collect();
                    let top = ROW_H + self.filter_chip_row_h();
                    // 説明は折り返して描く（共有の書式は折り返さない設定なので、レイアウトごとに切り替える）
                    if let Ok(layout) = self.dw.CreateTextLayout(&hint, &self.ui_format, (w - 24.0).max(1.0), (h - top - 2.0).max(1.0)) {
                        let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
                        rt.DrawTextLayout(Vector2 { X: 12.0, Y: top + 1.0 }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                    }
                }
            }
            for (i, (x0, y0, x1, y1)) in self.findrow_frames.get().into_iter().enumerate().take(rows) {
                let id = if i == 0 { ID_FIND } else { ID_REPL };
                // 入力中の欄：フォーカスの通知（EN_SETFOCUS / EN_KILLFOCUS）で覚えた値を使う（描画の瞬間の GetFocus には頼らない）
                let focused = self.findrow_focus_id.get() == id;
                let edge = if focused { theme::accent(self.settings.dark) } else { self.theme.dim };
                // 枠は、専用のブラシで描く（共有のブラシの色を何度も替える描き方だと、色が崩れることがあった）
                if let Ok(b) = rt.CreateSolidColorBrush(&edge, None) {
                    rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, 4.0), &b);
                }
                if let Ok(b) = rt.CreateSolidColorBrush(&self.theme.bg, None) {
                    rt.FillRoundedRectangle(&rrect(x0 + 1.0, y0 + 1.0, x1 - 1.0, y1 - 1.0, 3.0), &b);
                }
            }
        }
    }
}

/// ボタンのサブクラス：素早い 2 回目のクリック（WM_LBUTTONDBLCLK）を、ふつうの押下（WM_LBUTTONDOWN）として扱う。
/// 自前描画のボタンはダブルクリックを押下として描かないので、連打すると 2 回目のハイライトが出なかった
unsafe extern "system" fn button_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe { DefSubclassProc(hwnd, if msg == WM_LBUTTONDBLCLK { WM_LBUTTONDOWN } else { msg }, wparam, lparam) }
}

/// 入力欄のサブクラス：Enter・Esc・Tab・F3・Ctrl+… を、アプリの操作につなぐ。`data` は 0 = 検索欄、1 = 置換欄
unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        let main = GetParent(hwnd).unwrap_or_default();
        match msg {
            // Enter・Tab・Esc の文字（ビープ音を鳴らさない）
            WM_CHAR if matches!(wparam.0, 0x0D | 0x09 | 0x1B) => return LRESULT(0),
            // Ctrl+Backspace（標準の入力欄では、四角い記号が入ってしまう）：前の 1 語を消す。元に戻せる
            WM_CHAR if wparam.0 == 0x7F => {
                let mut sel = (0u32, 0u32);
                SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut sel.0 as *mut u32 as usize)), Some(LPARAM(&mut sel.1 as *mut u32 as isize)));
                if sel.0 == sel.1 {
                    let units: Vec<u16> = App::edit_text(hwnd).encode_utf16().collect();
                    let end = (sel.1 as usize).min(units.len());
                    let mut start = end;
                    while start > 0 && units[start - 1] == 0x20 {
                        start -= 1;
                    }
                    while start > 0 && units[start - 1] != 0x20 {
                        start -= 1;
                    }
                    SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(end as isize)));
                }
                SendMessageW(hwnd, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(w!("").as_ptr() as isize)));
                return LRESULT(0);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = VIRTUAL_KEY(wparam.0 as u16);
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let alt = msg == WM_SYSKEYDOWN || GetKeyState(VK_MENU.0 as i32) < 0;
                if msg == WM_KEYDOWN && !ctrl && !alt {
                    match vk {
                        VK_RETURN => {
                            // 検索・置換以外の入力は、実行（行へ移動・絞り込みなど）
                            if with_app(|a| !a.is_find_bar()) == Some(true) {
                                let d = with_app(|a| a.submit_prompt()).flatten();
                                // 閉じたときだけ本文へ（開いたままなら入力欄に残る）
                                if with_app(|a| a.prompt.is_none()) == Some(true) {
                                    let _ = SetFocus(Some(main));
                                }
                                if let Some(d) = d {
                                    run_deferred(main, d);
                                }
                                return LRESULT(0);
                            }
                            let act = if data == 1 {
                                FindAct::ReplOne
                            } else if shift {
                                FindAct::Prev
                            } else {
                                FindAct::Next
                            };
                            with_app(|a| a.find_bar_do(act));
                            return LRESULT(0);
                        }
                        VK_ESCAPE => {
                            with_app(|a| a.close_find_bar());
                            let _ = SetFocus(Some(main));
                            return LRESULT(0);
                        }
                        VK_TAB => {
                            // 検索欄と置換欄を行き来する
                            let other = with_app(|a| {
                                a.findrow.borrow().as_ref().map(|f| f.h(if data == 0 { ID_REPL } else { ID_FIND })).filter(|h| IsWindowVisible(*h).as_bool())
                            })
                            .flatten();
                            if let Some(h) = other {
                                let _ = SetFocus(Some(h));
                                SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                            }
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                }
                // ファンクションキーや Ctrl・Alt の組み合わせは、アプリの操作へ渡す（入力欄が自分で使うものは除く）
                let function = (VK_F1.0..=VK_F12.0).contains(&vk.0);
                let native = matches!(vk, VK_A | VK_C | VK_V | VK_X | VK_Z | VK_BACK | VK_DELETE | VK_LEFT | VK_RIGHT | VK_HOME | VK_END | VK_INSERT) && !alt;
                if (function || ((ctrl || alt) && !native)) && !matches!(vk, VK_CONTROL | VK_MENU | VK_SHIFT) {
                    let _ = PostMessageW(Some(main), msg, wparam, lparam);
                    return LRESULT(0);
                }
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}
