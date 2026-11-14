//! 設定ダイアログ（ファイル → 設定）。
//!
//! モードレスの 1 枚のウィンドウ。左に分類の一覧と検索欄（EmEditor の「すべての設定のプロパティ」のように、
//! 検索語に当てはまる項目を含む分類だけに絞る）、右に選んだ分類のページを出す。チェックボックスは押すとすぐ反映され、
//! `settings.ini` に保存される（表示メニューと同じ操作を呼ぶだけ）。フォント・大きさ・タブ幅の入力欄は、
//! 欄から離れたとき・閉じたとき・Enter で反映する。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};

use super::find_dialog::{make_child, pick_folder_titled, rgb_ref, wide};
use super::*;

/// ダイアログのウィンドウ（0 ならまだない）。メッセージループの `IsDialogMessage` が見る
pub(super) static SET_DLG: AtomicIsize = AtomicIsize::new(0);
static DARK: AtomicBool = AtomicBool::new(false);
/// ダークとライトの背景・入力欄のブラシ（[ライト, ダーク]）
static BG_BRUSH: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
static EDIT_BRUSH: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
/// 本文のフォントと、見出し用の大きなフォント（HFONT）
static UI_FONT: AtomicIsize = AtomicIsize::new(0);
static FIRST: AtomicBool = AtomicBool::new(true);

const CLASS: PCWSTR = w!("KazakiriSettingsDialog");
const EN_KILLFOCUS_CODE: u32 = 0x0200;

const ID_LINENUM: i32 = 214;
/// テーマの選び方（システム・ライト・ダーク）と、アクセントカラーの見本の ID（先頭から連番）
const ID_THEME_BASE: i32 = 250;
const ID_ACCENT_BASE: i32 = 260;
/// 大きさの − ＋ ボタン
const ID_SIZE_DEC: i32 = 270;
const ID_SIZE_INC: i32 = 271;
const ID_UISIZE_DEC: i32 = 272;
const ID_UISIZE_INC: i32 = 273;
const ID_TRAY: i32 = 226;
const ID_MODEBAR: i32 = 227;
const ID_FONT: i32 = 204;
const ID_SIZE: i32 = 206;
const ID_UIFONT: i32 = 207;
const ID_UISIZE: i32 = 209;
const ID_WRAP: i32 = 210;
const ID_IME: i32 = 211;
const ID_TAB: i32 = 212;
const ID_WS: i32 = 220;
const ID_LOG: i32 = 221;
const ID_SYN: i32 = 222;
const ID_TOOLBAR: i32 = 223;
const ID_FKEYS: i32 = 224;
const ID_BACKUP: i32 = 230;
const ID_WHEEL: i32 = 213;
const ID_MDAUTO: i32 = 231;
const ID_TITLE: i32 = 18;
const ID_SEARCH: i32 = 300;
const ID_CATS: i32 = 301;
const ID_BKSAVE: i32 = 232;
const ID_BKDIR: i32 = 233;
const ID_BKDIR_PICK: i32 = 234;
const ID_BKNUM: i32 = 235;
const ID_AUTOMIN: i32 = 236;
const ID_KEYS: i32 = 240;
const ID_CLOSE: i32 = 241;

/// チェックボックスの ID と、同じ働きの表示メニューのコマンド
const CHECKS: &[(i32, &str, u32)] = &[
    (ID_WRAP, "長い行を折り返して表示(&W)", menu::ID_WRAP),
    (ID_IME, "IME の変換中の文字をエディタ内に表示(&M)", menu::ID_IME_INLINE),
    (ID_WS, "空白・タブ・改行を表示(&S)", menu::ID_WHITESPACE),
    (ID_LOG, "ログの ERROR / WARN などに色を付ける(&L)", menu::ID_LOG_COLORS),
    (ID_SYN, "構文の色分け（JSON・SQL・XML など）(&X)", menu::ID_SYNTAX),
    (ID_TOOLBAR, "ツールバーを表示(&B)", menu::ID_TOOLBAR),
    (ID_MODEBAR, "モードバー（CSV の区切りなどのボタンの行）を表示(&E)", menu::ID_MODEBAR),
    (ID_FKEYS, "機能ボタンの列を表示（ステータスバーの上）(&U)", menu::ID_FKEYS),
    (ID_BACKUP, "GREP 置換の前にバックアップ (.bak) を作る(&B)", menu::ID_GREP_BACKUP),
];

/// 分類（左の一覧）
const CATS: [&str; 9] = ["基本", "スクロール", "外観", "表示", "検索・GREP", "バックアップ", "自動保存", "Markdown", "キーボード"];
const EN_CHANGE_CODE: u32 = 0x0300;
const LBN_SELCHANGE_CODE: u32 = 1;
const CBN_SELCHANGE_CODE: u32 = 1;
const CBN_KILLFOCUS_CODE: u32 = 4;
/// フォント名の一覧の先頭（空欄 = 自動）
const AUTO_FONT: &str = "（自動）";

/// 設定の 1 行（カード）。`toggle` があれば、右端にスイッチを出す
struct Row {
    page: usize,
    /// ページの上端からの位置（DIP）
    y: i32,
    title: &'static str,
    desc: &'static str,
    toggle: Option<i32>,
    /// 行の高さ（DIP）
    h: i32,
    /// フォントの見え方を見せる行
    preview: bool,
    /// カードのない小見出し
    heading: bool,
    /// 選択肢（先頭の ID, 種類 0 = 切り替え・1 = 色の見本, 個数）
    choice: Option<(i32, u8, usize)>,
}

/// アクセントカラーの見本（先頭の「Windows の設定の色」のあとに並べる）
const ACCENTS: [u32; 5] = [0x0067C0, 0x107C10, 0xCA5010, 0xE3008C, 0x8764B8];

/// コントロールの置き方：ウィンドウの大きさに合わせて置き直す（`relayout`）
#[derive(Clone, Copy)]
enum Anchor {
    /// カードの右端から（右端までの距離）
    CardRight(i32),
    /// カードの幅いっぱい
    CardWidth,
    /// 下端の手前まで伸ばす
    ListHeight,
    /// 右下の隅から（右・下までの距離）
    Corner(i32, i32),
}

#[derive(Clone, Copy)]
struct Place {
    id: i32,
    anchor: Anchor,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// ダイアログの配色（Windows 11 の設定アプリに合わせた、背景・カード・文字・アクセント）
struct Pal {
    bg: D2D1_COLOR_F,
    card: D2D1_COLOR_F,
    cardb: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    dim: D2D1_COLOR_F,
    knob: D2D1_COLOR_F,
}

const PAL_DARK: Pal = Pal { bg: rgb(0x20, 0x20, 0x20), card: rgb(0x2b, 0x2b, 0x2b), cardb: rgb(0x38, 0x38, 0x38), text: rgb(0xf2, 0xf2, 0xf2), dim: rgb(0x9a, 0x9a, 0x9a), knob: rgb(0x11, 0x11, 0x11) };
const PAL_LIGHT: Pal = Pal { bg: rgb(0xf3, 0xf3, 0xf3), card: rgb(0xfb, 0xfb, 0xfb), cardb: rgb(0xe5, 0xe5, 0xe5), text: rgb(0x1a, 0x1a, 0x1a), dim: rgb(0x6b, 0x6b, 0x6b), knob: rgb(0xff, 0xff, 0xff) };

/// いまのアクセントカラー（設定で選んだ色、なければ Windows の設定の色）
fn accent_now() -> D2D1_COLOR_F {
    super::theme::accent(DARK.load(Ordering::Relaxed))
}

/// 選択肢の各部品の位置（ダイアログ内。DIP）。(x0, y0, x1, y1)
fn choice_rects(row_y: i32, row_h: i32, card_w: i32, kind: u8, n: usize) -> Vec<(i32, i32, i32, i32)> {
    let right = PAGE_X + card_w - 16;
    let top = PAGE_Y + row_y;
    let (w, h, gap) = if kind == 0 { (78, 32, 0) } else { (28, 28, 10) };
    let total = n as i32 * w + (n as i32 - 1).max(0) * gap;
    (0..n as i32).map(|i| (right - total + i * (w + gap), top + (row_h - h) / 2, right - total + i * (w + gap) + w, top + (row_h - h) / 2 + h)).collect()
}

/// アクセントカラーの見本 i（0 は Windows の設定の色）
fn swatch_color(i: usize) -> D2D1_COLOR_F {
    if i == 0 {
        return super::theme::system_accent();
    }
    let c = ACCENTS[(i - 1).min(ACCENTS.len() - 1)];
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// ウィンドウの大きさに合わせて、カードの幅とコントロールの位置を置き直す
fn relayout(dlg: HWND) {
    unsafe {
        let scale = GetDpiForWindow(dlg).max(96) as f32 / 96.0;
        let mut rc = RECT::default();
        let _ = GetClientRect(dlg, &mut rc);
        let (cw, ch) = ((rc.right as f32 / scale) as i32, (rc.bottom as f32 / scale) as i32);
        let new_w = (cw - PAGE_X - 24).max(440);
        let (places, rows, ids) = CTRLS.with(|c| {
            let mut c = c.borrow_mut();
            let Some(ct) = c.as_mut() else { return (Vec::new(), Vec::new(), Vec::new()) };
            ct.card_w = new_w;
            (ct.places.clone(), ct.rows.iter().map(|r| (r.y, r.h, r.toggle, r.choice)).collect::<Vec<_>>(), ct.all.iter().map(|(i, _)| *i).collect::<Vec<_>>())
        });
        if ids.is_empty() {
            return;
        }
        let p = |v: i32| (v as f32 * scale).round() as i32;
        let put = |id: i32, x: i32, y: i32, w: i32, h: i32| {
            let _ = SetWindowPos(ctl(id), None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
        };
        for pl in places {
            match pl.anchor {
                Anchor::CardRight(off) => put(pl.id, PAGE_X + new_w - off, pl.y, pl.w, pl.h),
                Anchor::CardWidth => put(pl.id, pl.x, pl.y, new_w, pl.h),
                Anchor::ListHeight => put(pl.id, pl.x, pl.y, pl.w, (ch - pl.y - 14).max(100)),
                Anchor::Corner(dx, dy) => put(pl.id, cw - dx, ch - dy, pl.w, pl.h),
            }
        }
        for (y, h, toggle, choice) in rows {
            if let Some(id) = toggle {
                put(id, PAGE_X + 4, PAGE_Y + y + 4, new_w - 8, h - 8);
            }
            if let Some((first, kind, n)) = choice {
                for (i, (x0, y0, x1, y1)) in choice_rects(y, h, new_w, kind, n).into_iter().enumerate() {
                    put(first + i as i32, x0, y0, x1 - x0, y1 - y0);
                }
            }
        }
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// インストールされているフォントの名前（日本語名があればそれ）
fn font_families() -> Vec<String> {
    unsafe {
        let Ok(dw) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else { return Vec::new() };
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return Vec::new();
        }
        let Some(coll) = coll else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..coll.GetFontFamilyCount() {
            let Ok(fam) = coll.GetFontFamily(i) else { continue };
            let Ok(names) = fam.GetFamilyNames() else { continue };
            let (mut idx, mut exists) = (0u32, BOOL(0));
            let _ = names.FindLocaleName(w!("ja-jp"), &mut idx, &mut exists);
            let idx = if exists.as_bool() { idx } else { 0 };
            let Ok(len) = names.GetStringLength(idx) else { continue };
            let mut buf = vec![0u16; len as usize + 1];
            if names.GetString(idx, &mut buf).is_ok() {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
        }
        out.sort_by_key(|s| s.to_lowercase());
        out.dedup();
        out
    }
}

fn pal() -> &'static Pal {
    if DARK.load(Ordering::Relaxed) { &PAL_DARK } else { &PAL_LIGHT }
}

/// 1 行の高さ・行の間・カードの幅（DIP）と、ページの左上（ダイアログ内の座標）
const ROW_H: i32 = 56;
const ROW_GAP: i32 = 6;
const CARD_W: i32 = 496;
const PAGE_X: i32 = 184;
const PAGE_Y: i32 = 56;

struct Ctrls {
    /// いまのカードの幅（DIP）
    card_w: i32,
    places: Vec<Place>,
    /// 選択肢のいま選んでいるもの（先頭の ID → 番号）
    sel: HashMap<i32, usize>,
    rows: Vec<Row>,
    /// スイッチの状態（ID → オン）
    toggles: HashMap<i32, bool>,
    all: Vec<(i32, HWND)>,
    /// 分類ごとの部品：(ID, 分類の番号, 検索用の文字列)
    pages: Vec<(i32, usize, String)>,
    /// 一覧に出している分類
    visible: Vec<usize>,
    cur: usize,
}

thread_local! {
    static CTRLS: RefCell<Option<Ctrls>> = const { RefCell::new(None) };
}

fn ctl(id: i32) -> HWND {
    CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.all.iter().find(|(i, _)| *i == id).map(|(_, h)| *h)).unwrap_or_default())
}

fn text_of(id: i32) -> String {
    let mut buf = vec![0u16; 512];
    let n = unsafe { GetWindowTextW(ctl(id), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn set_text(id: i32, s: &str) {
    let w = wide(s);
    unsafe {
        let _ = SetWindowTextW(ctl(id), PCWSTR(w.as_ptr()));
    }
}

fn set_checked(id: i32, on: bool) {
    CTRLS.with(|c| {
        if let Some(ct) = c.borrow_mut().as_mut() {
            ct.toggles.insert(id, on);
        }
    });
}

fn is_checked(id: i32) -> bool {
    CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.toggles.get(&id).copied())).unwrap_or(false)
}

fn is_toggle(id: i32) -> bool {
    CTRLS.with(|c| c.borrow().as_ref().is_some_and(|ct| ct.rows.iter().any(|r| r.toggle == Some(id))))
}

fn flip_checked(id: i32) {
    set_checked(id, !is_checked(id));
}

/// アプリの今の設定を、ダイアログの各部品に写す
fn sync() {
    let Some(s) = with_app(|app| app.settings.clone()) else { return };
    set_checked(ID_LINENUM, s.line_numbers);
    set_checked(ID_WRAP, s.wrap);
    set_checked(ID_IME, s.ime_inline);
    set_checked(ID_WS, s.whitespace);
    set_checked(ID_LOG, s.log_colors);
    set_checked(ID_SYN, s.syntax);
    set_checked(ID_TOOLBAR, s.toolbar);
    set_checked(ID_FKEYS, s.fkeys);
    set_checked(ID_TRAY, s.tray);
    set_checked(ID_MODEBAR, s.modebar);
    set_checked(ID_BACKUP, s.grep_backup);
    set_checked(ID_MDAUTO, s.md_auto);
    set_checked(ID_BKSAVE, s.backup_save);
    set_checked(ID_BKNUM, s.backup_numbered);
    if unsafe { GetFocus() } != ctl(ID_BKDIR) {
        set_text(ID_BKDIR, &s.backup_dir);
    }
    if unsafe { GetFocus() } != ctl(ID_AUTOMIN) {
        set_text(ID_AUTOMIN, &s.autosave_min.to_string());
    }
    if unsafe { GetFocus() } != ctl(ID_WHEEL) {
        set_text(ID_WHEEL, &s.wheel_lines.to_string());
    }
    // 編集中の欄は触らない
    let focus = unsafe { GetFocus() };
    if focus != ctl(ID_FONT) {
        set_text(ID_FONT, &s.font);
    }
    if focus != ctl(ID_SIZE) {
        set_text(ID_SIZE, &format!("{}", s.font_size));
    }
    if focus != ctl(ID_UIFONT) {
        set_text(ID_UIFONT, &s.ui_font);
    }
    if focus != ctl(ID_UISIZE) {
        set_text(ID_UISIZE, &format!("{}", s.ui_font_size));
    }
    if focus != ctl(ID_TAB) {
        set_text(ID_TAB, &s.tab_width.to_string());
    }
    // 選択肢：テーマの選び方、アクセントカラー
    let accent_idx = match s.accent {
        None => Some(0),
        Some(c) => ACCENTS.iter().position(|a| *a == c).map(|i| i + 1),
    };
    CTRLS.with(|c| {
        if let Some(ct) = c.borrow_mut().as_mut() {
            ct.sel.insert(ID_THEME_BASE, s.theme_mode.min(2) as usize);
            match accent_idx {
                Some(i) => ct.sel.insert(ID_ACCENT_BASE, i),
                None => ct.sel.remove(&ID_ACCENT_BASE),
            };
        }
    });
    set_theme(s.dark);
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if !dlg.0.is_null() {
        unsafe {
            let _ = InvalidateRect(Some(dlg), None, true);
        }
    }
}

fn set_theme(dark: bool) {
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if dlg.0.is_null() {
        return;
    }
    let changed = DARK.swap(dark, Ordering::Relaxed) != dark;
    if !changed {
        return;
    }
    unsafe {
        let all: Vec<(i32, HWND)> = CTRLS.with(|c| c.borrow().as_ref().map(|ct| ct.all.clone()).unwrap_or_default());
        for (id, h) in all {
            // フォントの選択（コンボボックス）は、専用のテーマ名
            let name = if id == ID_FONT || id == ID_UIFONT { w!("DarkMode_CFD") } else { w!("DarkMode_Explorer") };
            let _ = SetWindowTheme(h, if dark { name } else { PCWSTR::null() }, PCWSTR::null());
        }
        let v = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(dlg, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// 分類のページを出す（ほかの分類の部品は隠す）
fn show_page(cat: usize) {
    let pages: Vec<(i32, usize)> = CTRLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(ct) = c.as_mut() else { return Vec::new() };
        ct.cur = cat;
        ct.pages.iter().map(|(i, p, _)| (*i, *p)).collect()
    });
    unsafe {
        for (id, page) in pages {
            let _ = ShowWindow(ctl(id), if page == cat { SW_SHOW } else { SW_HIDE });
        }
    }
    set_text(ID_TITLE, CATS[cat]);
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if !dlg.0.is_null() {
        unsafe {
            let _ = InvalidateRect(Some(dlg), None, true);
        }
    }
}

/// 検索語に当てはまる項目を持つ分類だけを、左の一覧に出す
fn fill_cats(query: &str) {
    let q = query.trim().to_lowercase();
    let (visible, cur) = CTRLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(ct) = c.as_mut() else { return (Vec::new(), 0) };
        ct.visible = (0..CATS.len()).filter(|&i| q.is_empty() || CATS[i].to_lowercase().contains(&q) || ct.pages.iter().any(|(_, p, t)| *p == i && t.contains(&q))).collect();
        (ct.visible.clone(), ct.cur)
    });
    let list = ctl(ID_CATS);
    unsafe {
        SendMessageW(list, LB_RESETCONTENT, None, None);
        for &i in &visible {
            let w = wide(CATS[i]);
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(w.as_ptr() as isize)));
        }
        let pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
        if let Some(&cat) = visible.get(pos) {
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(pos)), None);
            show_page(cat);
        }
    }
}

/// フォント名・大きさ・タブ幅の欄を設定へ反映する
fn commit_edits() {
    let font = text_of(ID_FONT).trim().to_string();
    let size = text_of(ID_SIZE).trim().parse::<f32>().ok();
    let tab = text_of(ID_TAB).trim().parse::<usize>().ok();
    let ui_font = text_of(ID_UIFONT).trim().to_string();
    let ui_size = text_of(ID_UISIZE).trim().parse::<f32>().ok();
    let wheel = text_of(ID_WHEEL).trim().parse::<usize>().ok();
    let bk_dir = text_of(ID_BKDIR).trim().to_string();
    let auto_min = text_of(ID_AUTOMIN).trim().parse::<usize>().ok().map(|n| n.min(1440));
    with_app(|app| {
        if bk_dir != app.settings.backup_dir {
            app.settings.backup_dir = bk_dir.clone();
            app.settings.save();
        }
        if let Some(n) = auto_min.filter(|&n| n != app.settings.autosave_min) {
            app.settings.autosave_min = n;
            app.settings.save();
            app.restart_autosave_timer();
        }
        if let Some(n) = wheel.map(|n| n.clamp(1, 30)).filter(|&n| n != app.settings.wheel_lines) {
            app.settings.wheel_lines = n;
            app.settings.save();
        }
        let s = &app.settings;
        let (fs, tw, us) = (s.font_size, s.tab_width, s.ui_font_size);
        app.set_text_settings(&font, size.unwrap_or(fs), tab.unwrap_or(tw));
        app.set_ui_font(&ui_font, ui_size.unwrap_or(us));
    });
    sync();
}

/// 大きさを `delta` だけ変える（− ＋ ボタン）
fn step_size(id_size: i32, delta: f32, min: f32, max: f32) {
    let cur = text_of(id_size).trim().parse::<f32>().unwrap_or(14.0);
    set_text(id_size, &format!("{}", (cur + delta).clamp(min, max)));
    commit_edits();
}

unsafe fn on_command(dlg: HWND, id: i32, code: u32) {
    unsafe {
        let owner = GetWindow(dlg, GW_OWNER).unwrap_or_default();
        if id == ID_SEARCH {
            if code == EN_CHANGE_CODE {
                fill_cats(&text_of(ID_SEARCH));
            }
            return;
        }
        if id == ID_CATS {
            if code == LBN_SELCHANGE_CODE {
                let sel = SendMessageW(ctl(ID_CATS), LB_GETCURSEL, None, None).0;
                let cat = CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| usize::try_from(sel).ok().and_then(|i| ct.visible.get(i).copied())));
                if let Some(cat) = cat {
                    show_page(cat);
                }
            }
            return;
        }
        // 検索欄で Enter を押しても、ダイアログは閉じない
        if id == IDOK.0 && GetFocus() == ctl(ID_SEARCH) {
            return;
        }
        // フォント名のコンボボックス：一覧から選んだら、その名前にして反映する（「（自動）」は空欄）
        if matches!(id, ID_FONT | ID_UIFONT) {
            if code == CBN_SELCHANGE_CODE {
                let list = ctl(id);
                let sel = SendMessageW(list, CB_GETCURSEL, None, None).0;
                if sel >= 0 {
                    let len = SendMessageW(list, CB_GETLBTEXTLEN, Some(WPARAM(sel as usize)), None).0.max(0) as usize;
                    let mut buf = vec![0u16; len + 1];
                    SendMessageW(list, CB_GETLBTEXT, Some(WPARAM(sel as usize)), Some(LPARAM(buf.as_mut_ptr() as isize)));
                    let name = String::from_utf16_lossy(&buf[..len]);
                    set_text(id, if name == AUTO_FONT { "" } else { &name });
                    commit_edits();
                }
            } else if code == EN_KILLFOCUS_CODE || code == CBN_KILLFOCUS_CODE {
                commit_edits();
            }
            return;
        }
        if matches!(id, ID_SIZE | ID_TAB | ID_UISIZE | ID_WHEEL | ID_BKDIR | ID_AUTOMIN) {
            if code == EN_KILLFOCUS_CODE {
                commit_edits();
            }
            return;
        }
        if code != BN_CLICKED && id != IDOK.0 && id != IDCANCEL.0 {
            return;
        }
        if id == IDOK.0 || id == IDCANCEL.0 || id == ID_CLOSE {
            commit_edits();
            let _ = ShowWindow(dlg, SW_HIDE);
            return;
        }
        match id {
            ID_SIZE_DEC => return step_size(ID_SIZE, -1.0, settings::MIN_FONT, settings::MAX_FONT),
            ID_SIZE_INC => return step_size(ID_SIZE, 1.0, settings::MIN_FONT, settings::MAX_FONT),
            ID_UISIZE_DEC => return step_size(ID_UISIZE, -1.0, settings::MIN_FONT, 40.0),
            ID_UISIZE_INC => return step_size(ID_UISIZE, 1.0, settings::MIN_FONT, 40.0),
            i if (ID_THEME_BASE..ID_THEME_BASE + 3).contains(&i) => {
                with_app(|app| app.set_theme_mode((i - ID_THEME_BASE) as u8));
            }
            i if (ID_ACCENT_BASE..ID_ACCENT_BASE + 1 + ACCENTS.len() as i32).contains(&i) => {
                let n = (i - ID_ACCENT_BASE) as usize;
                with_app(|app| app.set_accent(if n == 0 { None } else { Some(ACCENTS[n - 1]) }));
            }
            ID_LINENUM => {
                let on = is_checked(ID_LINENUM);
                with_app(|app| {
                    app.settings.line_numbers = on;
                    app.settings.save();
                    app.layout_cache.borrow_mut().clear();
                    app.invalidate();
                });
            }
            ID_TRAY => {
                let on = is_checked(ID_TRAY);
                with_app(|app| app.set_tray(on));
            }
            ID_BKDIR_PICK => {
                if let Some(d) = pick_folder_titled(dlg, "バックアップを置くフォルダを選んでください") {
                    set_text(ID_BKDIR, &d);
                    commit_edits();
                }
            }
            ID_BKSAVE | ID_BKNUM => {
                let on = is_checked(id);
                with_app(|app| {
                    if id == ID_BKSAVE {
                        app.settings.backup_save = on;
                    } else {
                        app.settings.backup_numbered = on;
                    }
                    app.settings.save();
                });
            }
            ID_MDAUTO => {
                let on = is_checked(ID_MDAUTO);
                with_app(|app| {
                    app.settings.md_auto = on;
                    app.settings.save();
                });
            }
            ID_KEYS => {
                let d = with_app(|app| app.command(menu::ID_KEYS)).flatten();
                if let Some(d) = d {
                    run_deferred(owner, d);
                }
                let _ = ShowWindow(dlg, SW_HIDE);
            }
            _ => {
                if let Some(&(_, _, cmd)) = CHECKS.iter().find(|(i, _, _)| *i == id) {
                    let d = with_app(|app| app.command(cmd)).flatten();
                    if let Some(d) = d {
                        run_deferred(owner, d);
                    }
                }
            }
        }
        sync();
        let _ = InvalidateRect(Some(owner), None, false);
    }
}

/// ダイアログを（なければ作って）開く
pub(super) unsafe fn show(owner: HWND) {
    unsafe {
        if SET_DLG.load(Ordering::Relaxed) == 0 {
            create(owner);
        }
        let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
        if dlg.0.is_null() {
            return;
        }
        DARK.store(!with_app(|app| app.settings.dark).unwrap_or(false), Ordering::Relaxed);
        sync();
        if FIRST.swap(false, Ordering::Relaxed) {
            let mut r = RECT::default();
            let _ = GetWindowRect(owner, &mut r);
            let _ = SetWindowPos(dlg, None, r.left + 80, r.top + 60, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
    }
}

unsafe fn create(owner: HWND) {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(dlg_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let Ok(dlg) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("設定"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            300,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let dpi = GetDpiForWindow(dlg).max(96);
        let scale = dpi as f32 / 96.0;
        // タイトルバーに、アプリのアイコン
        for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let size = GetSystemMetricsForDpi(metric, dpi);
            if let Ok(icon) = LoadImageW(Some(instance.into()), PCWSTR(std::ptr::without_provenance::<u16>(1)), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(dlg, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), 0, dpi).is_ok() {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        };
        let push = BS_PUSHBUTTON as u32 | WS_TABSTOP.0;
        let edit = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
        let combo = (CBS_DROPDOWN as u32) | (CBS_AUTOHSCROLL as u32) | WS_VSCROLL.0 | WS_TABSTOP.0;
        let mut all: Vec<(i32, HWND)> = Vec::new();
        let mut pages: Vec<(i32, usize, String)> = Vec::new();
        let mut rows: Vec<Row> = Vec::new();
        let mut places: Vec<Place> = Vec::new();
        let p = |v: i32| (v as f32 * scale).round() as i32;
        // page: 分類の番号（usize::MAX は、どの分類でも出す部品）。x, y はダイアログの左上から
        let mut put = |page: usize, id: i32, class: PCWSTR, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, h: i32| {
            let hw = make_child(dlg, class, text, style, ex, id, font);
            let _ = SetWindowPos(hw, None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
            all.push((id, hw));
            if page != usize::MAX {
                pages.push((id, page, text.replace('&', "").to_lowercase()));
            }
        };
        const ALL: usize = usize::MAX;
        // ページごとの、次の行の上端（ページの上端からの位置。DIP）
        let mut cur = [0i32; 9];
        // 1 行（カード）を足す。戻り値は、その行の上端（ダイアログ内の座標）。検索用に、題と説明も覚える
        let card = |rows: &mut Vec<Row>, cur: &mut [i32; 9], page: usize, title: &'static str, desc: &'static str, toggle: Option<i32>, choice: Option<(i32, u8, usize)>| -> i32 {
            let y = cur[page];
            rows.push(Row { page, y, title, desc, toggle, h: ROW_H, preview: false, heading: false, choice });
            cur[page] += ROW_H + ROW_GAP;
            PAGE_Y + y
        };
        // 小見出し
        let heading = |rows: &mut Vec<Row>, cur: &mut [i32; 9], page: usize, title: &'static str| {
            let y = cur[page];
            rows.push(Row { page, y, title, desc: "", toggle: None, h: 26, preview: false, heading: true, choice: None });
            cur[page] += 32;
        };
        // 入力欄の縦の位置（カードの中で縦中央）
        let mid = |top: i32, h: i32| top + (ROW_H - h) / 2;
        let right = PAGE_X + CARD_W - 16;
        // カードの右端までの距離（右に寄せる部品の置き方）
        let cr = |x: i32| Anchor::CardRight(PAGE_X + CARD_W - x);
        let flat = WS_BORDER.0;
        put(ALL, ID_SEARCH, w!("EDIT"), "", edit | flat, 0, 12, 14, 156, 26);
        put(ALL, ID_CATS, w!("LISTBOX"), "", (LBS_NOTIFY as u32) | (LBS_OWNERDRAWFIXED as u32) | (LBS_HASSTRINGS as u32) | WS_TABSTOP.0, 0, 12, 52, 164, 420);
        places.push(Place { id: ID_CATS, anchor: Anchor::ListHeight, x: 12, y: 52, w: 164, h: 420 });
        put(ALL, ID_TITLE, w!("STATIC"), "", 0, 0, PAGE_X, 12, 480, 34);
        // 基本
        card(&mut rows, &mut cur, 0, "長い行を折り返して表示", "画面の幅で折り返す（文字単位）", Some(ID_WRAP), None);
        card(&mut rows, &mut cur, 0, "IME の変換中の文字をエディタ内に表示", "オフなら IME 標準の窓", Some(ID_IME), None);
        card(&mut rows, &mut cur, 0, "行番号", "本文の左の行番号の欄を出す", Some(ID_LINENUM), None);
        let y = card(&mut rows, &mut cur, 0, "タブの幅", "半角の文字数", None, None);
        put(0, ID_TAB, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_TAB, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        // スクロール
        let y = card(&mut rows, &mut cur, 1, "マウスホイール", "1 回で動かす行数", None, None);
        put(1, ID_WHEEL, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_WHEEL, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        // 外観
        heading(&mut rows, &mut cur, 2, "テーマ");
        card(&mut rows, &mut cur, 2, "アプリのテーマ", "Windows の設定に合わせる、または固定する", None, Some((ID_THEME_BASE, 0, 3)));
        card(&mut rows, &mut cur, 2, "アクセントカラー", "選択・ボタン・フォーカスに使う色。左端は Windows の設定の色", None, Some((ID_ACCENT_BASE, 1, 1 + ACCENTS.len())));
        heading(&mut rows, &mut cur, 2, "フォント");
        // フォント名の欄（一覧から選ぶか、書く）と、大きさの − [数] ＋
        for (title, desc, id_font, id_dec, id_size, id_inc) in [("本文のフォント", "空欄で自動。等幅のフォントがおすすめ", ID_FONT, ID_SIZE_DEC, ID_SIZE, ID_SIZE_INC), ("画面のフォント", "メニュー・タブなど。空欄で自動", ID_UIFONT, ID_UISIZE_DEC, ID_UISIZE, ID_UISIZE_INC)] {
            let y = card(&mut rows, &mut cur, 2, title, desc, None, None);
            let (xi, xs, xd, xf) = (right - 28, right - 28 - 4 - 48, right - 28 - 4 - 48 - 4 - 28, right - 28 - 4 - 48 - 4 - 28 - 8 - 190);
            put(2, id_inc, w!("BUTTON"), "＋", push, 0, xi, mid(y, 28), 28, 28);
            put(2, id_size, w!("EDIT"), "", edit | ES_NUMBER as u32 | ES_CENTER as u32 | flat, 0, xs, mid(y, 26), 48, 26);
            put(2, id_dec, w!("BUTTON"), "－", push, 0, xd, mid(y, 28), 28, 28);
            put(2, id_font, w!("COMBOBOX"), "", combo, 0, xf, mid(y, 26), 190, 320);
            places.push(Place { id: id_inc, anchor: cr(xi), x: xi, y: mid(y, 28), w: 28, h: 28 });
            places.push(Place { id: id_size, anchor: cr(xs), x: xs, y: mid(y, 26), w: 48, h: 26 });
            places.push(Place { id: id_dec, anchor: cr(xd), x: xd, y: mid(y, 28), w: 28, h: 28 });
            places.push(Place { id: id_font, anchor: cr(xf), x: xf, y: mid(y, 26), w: 190, h: 320 });
        }
        // フォントの見え方（設定を変えると、すぐ変わる）
        let py = cur[2];
        rows.push(Row { page: 2, y: py, title: "プレビュー", desc: "", toggle: None, h: 92, preview: true, heading: false, choice: None });
        cur[2] += 98;
        // 表示
        heading(&mut rows, &mut cur, 3, "本文");
        card(&mut rows, &mut cur, 3, "空白・タブ・改行を表示", "印で見えるようにする（Ctrl+Shift+W）", Some(ID_WS), None);
        card(&mut rows, &mut cur, 3, "ログの色分け", "ERROR / WARN などに色を付ける", Some(ID_LOG), None);
        card(&mut rows, &mut cur, 3, "構文の色分け", "JSON・SQL・XML など", Some(ID_SYN), None);
        heading(&mut rows, &mut cur, 3, "画面の部品");
        card(&mut rows, &mut cur, 3, "ツールバー", "画面の上のボタンの行", Some(ID_TOOLBAR), None);
        card(&mut rows, &mut cur, 3, "モードバー", "CSV・Markdown のときの操作の行", Some(ID_MODEBAR), None);
        card(&mut rows, &mut cur, 3, "機能ボタンの列", "ステータスバーの上の「検索 Ctrl+F …」", Some(ID_FKEYS), None);
        card(&mut rows, &mut cur, 3, "通知領域のアイコン", "クリックで新規作成・開くなど", Some(ID_TRAY), None);
        // 検索・GREP
        card(&mut rows, &mut cur, 4, "GREP 置換の前にバックアップ", ".bak を作る", Some(ID_BACKUP), None);
        // バックアップ
        card(&mut rows, &mut cur, 5, "上書き保存の前にバックアップ", "元のファイルを残す", Some(ID_BKSAVE), None);
        let y = card(&mut rows, &mut cur, 5, "バックアップを置くフォルダ", "空欄で、元のファイルと同じ", None, None);
        put(5, ID_BKDIR, w!("EDIT"), "", edit | flat, 0, right - 80 - 8 - 190, mid(y, 26), 190, 26);
        put(5, ID_BKDIR_PICK, w!("BUTTON"), "参照...", push, 0, right - 80, mid(y, 28), 80, 28);
        places.push(Place { id: ID_BKDIR, anchor: cr(right - 80 - 8 - 190), x: right - 80 - 8 - 190, y: mid(y, 26), w: 190, h: 26 });
        places.push(Place { id: ID_BKDIR_PICK, anchor: cr(right - 80), x: right - 80, y: mid(y, 28), w: 80, h: 28 });
        card(&mut rows, &mut cur, 5, "古いバックアップを残す", ".bak2、.bak3 … と番号を増やす（オフなら .bak を上書き）", Some(ID_BKNUM), None);
        put(5, 41, w!("STATIC"), "バックアップは、名前を付けて保存・変換して保存を含む、既存のファイルを書き換える保存の直前に作ります。", 0, 0, PAGE_X, PAGE_Y + cur[5] + 4, CARD_W, 40);
        places.push(Place { id: 41, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[5] + 4, w: CARD_W, h: 40 });
        // 自動保存
        let y = card(&mut rows, &mut cur, 6, "自動保存の間隔", "分。0 で自動保存しない", None, None);
        put(6, ID_AUTOMIN, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_AUTOMIN, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        put(6, 43, w!("STATIC"), "未保存の編集を、『名前.autosave』に書き出します（元のファイルは書き換えません）。保存するか、通常どおり閉じると消え、異常終了したときだけ残ります。バックアップのフォルダを指定していれば、そこに書き出します。", 0, 0, PAGE_X, PAGE_Y + cur[6] + 4, CARD_W, 80);
        places.push(Place { id: 43, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[6] + 4, w: CARD_W, h: 80 });
        // Markdown
        card(&mut rows, &mut cur, 7, "開いたときにプレビューを並べる", "Markdown ファイル（.md）を開いたとき、右に並べる", Some(ID_MDAUTO), None);
        put(7, 16, w!("STATIC"), "プレビューは、表示メニューやモードバーのボタン（Ctrl+Shift+V：右に並べる、Ctrl+Shift+G：デザインビュー）でも切り替えられます。", 0, 0, PAGE_X, PAGE_Y + cur[7] + 4, CARD_W, 40);
        places.push(Place { id: 16, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[7] + 4, w: CARD_W, h: 40 });
        // キーボード
        let y = card(&mut rows, &mut cur, 8, "キー割り当て", "コマンドごとのキーを、一覧から選んで変える", None, None);
        put(8, ID_KEYS, w!("BUTTON"), "変更...", push, 0, right - 90, mid(y, 28), 90, 28);
        places.push(Place { id: ID_KEYS, anchor: cr(right - 90), x: right - 90, y: mid(y, 28), w: 90, h: 28 });
        put(ALL, ID_CLOSE, w!("BUTTON"), "閉じる", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0, 0, 620, 520, 90, 30);
        places.push(Place { id: ID_CLOSE, anchor: Anchor::Corner(100, 46), x: 620, y: 520, w: 90, h: 30 });
        // フォント名の一覧（先頭は「（自動）」）
        let fonts = font_families();
        for id_font in [ID_FONT, ID_UIFONT] {
            if let Some(&(_, h)) = all.iter().find(|(i, _)| *i == id_font) {
                let auto = wide(AUTO_FONT);
                SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(auto.as_ptr() as isize)));
                for name in &fonts {
                    let w = wide(name);
                    SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(w.as_ptr() as isize)));
                }
            }
        }
        // 本文より大きな見出しのフォント
        let mut big = font_lf(font);
        big.lfHeight = (big.lfHeight as f32 * 2.0) as i32;
        big.lfWeight = 600;
        big.lfFaceName = [0; 32];
        for (i, c) in "Yu Gothic UI".encode_utf16().enumerate() {
            big.lfFaceName[i] = c;
        }
        let big_font = CreateFontIndirectW(&big);
        if let Some(&(_, h)) = all.iter().find(|(i, _)| *i == ID_TITLE) {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(big_font.0 as usize)), Some(LPARAM(1)));
        }
        // スイッチの行・選択肢ごとに、カードの上に透明なボタンを重ねる（Tab で止まり、Space で押せる。クリックもこのボタンが受ける）
        for r in &rows {
            if let Some(id) = r.toggle {
                let hw = make_child(dlg, w!("BUTTON"), "", BS_OWNERDRAW as u32 | WS_TABSTOP.0, 0, id, font);
                let (x, y) = (PAGE_X + 4, PAGE_Y + r.y + 4);
                let _ = SetWindowPos(hw, None, p(x), p(y), p(CARD_W - 8), p(r.h - 8), SWP_NOZORDER | SWP_NOACTIVATE);
                all.push((id, hw));
                pages.push((id, r.page, String::new()));
            }
            if let Some((first, kind, n)) = r.choice {
                for (i, (x0, y0, x1, y1)) in choice_rects(r.y, r.h, CARD_W, kind, n).into_iter().enumerate() {
                    let id = first + i as i32;
                    let hw = make_child(dlg, w!("BUTTON"), "", BS_OWNERDRAW as u32 | WS_TABSTOP.0, 0, id, font);
                    let _ = SetWindowPos(hw, None, p(x0), p(y0), p(x1 - x0), p(y1 - y0), SWP_NOZORDER | SWP_NOACTIVATE);
                    all.push((id, hw));
                    pages.push((id, r.page, String::new()));
                }
            }
        }
        // 検索欄の薄い案内文
        if let Some(&(_, h)) = all.iter().find(|(i, _)| *i == ID_SEARCH) {
            let cue = wide("設定を検索");
            SendMessageW(h, 0x1501, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
        }
        UI_FONT.store(font.0 as isize, Ordering::Relaxed);
        // 検索で見つかるように、各行の題と説明も、その分類の文字列として覚える
        for r in &rows {
            pages.push((0, r.page, format!("{} {}", r.title, r.desc).to_lowercase()));
        }
        for (i, pal) in [&PAL_LIGHT, &PAL_DARK].into_iter().enumerate() {
            let edit_bg = if i == 1 { rgb(0x1f, 0x1f, 0x1f) } else { rgb(0xff, 0xff, 0xff) };
            BG_BRUSH[i].store(CreateSolidBrush(rgb_ref(&pal.bg)).0 as isize, Ordering::Relaxed);
            EDIT_BRUSH[i].store(CreateSolidBrush(rgb_ref(&edit_bg)).0 as isize, Ordering::Relaxed);
        }
        CTRLS.with(|c| *c.borrow_mut() = Some(Ctrls { card_w: CARD_W, places, sel: HashMap::new(), rows, toggles: HashMap::new(), all, pages, visible: Vec::new(), cur: 0 }));
        fill_cats("");
        SET_DLG.store(dlg.0 as isize, Ordering::Relaxed);
        SendMessageW(ctl(ID_CATS), LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(p(40) as isize)));
        // クライアント領域を 720 × 566 に
        let style = WINDOW_STYLE(GetWindowLongW(dlg, GWL_STYLE) as u32);
        let mut rc = RECT { left: 0, top: 0, right: p(720), bottom: p(566) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let _ = SetWindowPos(dlg, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// 設定ダイアログのフォントの LOGFONT
unsafe fn font_lf(font: HFONT) -> LOGFONTW {
    let mut lf = LOGFONTW::default();
    unsafe {
        GetObjectW(font.into(), std::mem::size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
    }
    lf
}

/// フォントの見え方（本文のフォントと大きさで、構文・ログの色の例を描く）
unsafe fn draw_preview(hdc: HDC, x: i32, y: i32, h: i32, scale: f32, pal: &Pal) {
    let Some((face, size)) = with_app(|a| (a.settings.font.clone(), a.settings.font_size)) else { return };
    let face = if face.is_empty() { "BIZ UDGothic".to_string() } else { face };
    let t = if DARK.load(Ordering::Relaxed) { &super::theme::DARK } else { &super::theme::LIGHT };
    unsafe {
        let wface = wide(&face);
        let mut name = [0u16; 32];
        for (i, c) in wface.iter().take(31).enumerate() {
            name[i] = *c;
        }
        let lf = LOGFONTW { lfHeight: -((size * scale).round() as i32), lfCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET, lfFaceName: name, ..Default::default() };
        let font = CreateFontIndirectW(&lf);
        let old = SelectObject(hdc, font.into());
        SetBkMode(hdc, TRANSPARENT);
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(hdc, &mut tm);
        let lh = tm.tmHeight.max(12) + 2;
        // 見出し（小さく）
        let y0 = y + (h - lh * 2) / 2;
        let runs: [&[(&str, &D2D1_COLOR_F)]; 2] = [
            &[("SELECT", &t.syn_kw), (" id, name ", &t.text), ("FROM", &t.syn_kw), (" users ", &t.text), ("WHERE", &t.syn_kw), (" status = ", &t.text), ("'active'", &t.syn_str), (";", &t.text)],
            &[("ERROR", &t.log_error), (" 接続できません　", &t.text), ("WARN", &t.log_warn), (" 再試行 1/3　", &t.text), ("12345", &t.syn_num)],
        ];
        let _ = pal;
        for (row, line) in runs.iter().enumerate() {
            let mut cx = x;
            for (text, color) in line.iter() {
                SetTextColor(hdc, rgb_ref(color));
                let mut u: Vec<u16> = text.encode_utf16().collect();
                let mut rc = RECT { left: cx, top: y0 + row as i32 * lh, right: cx + 2000, bottom: y0 + row as i32 * lh + lh };
                DrawTextW(hdc, &mut u, &mut rc, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP);
                let mut sz = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &u, &mut sz);
                cx += sz.cx;
            }
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(font.into());
    }
}

/// 左の分類の 1 行：アイコン・名前、選んでいる行は左にアクセントカラーの帯
unsafe fn draw_cat_item(di: &DRAWITEMSTRUCT) {
    const ICONS: [&str; 9] = ["\u{E713}", "\u{E8CB}", "\u{E790}", "\u{E890}", "\u{E721}", "\u{E74E}", "\u{E823}", "\u{E8A5}", "\u{E765}"];
    let p = pal();
    let hdc = di.hDC;
    let rc = di.rcItem;
    let Some(cat) = CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.visible.get(di.itemID as usize).copied())) else {
        unsafe {
            let b = CreateSolidBrush(rgb_ref(&p.bg));
            FillRect(hdc, &rc, b);
            let _ = DeleteObject(b.into());
        }
        return;
    };
    let scale = unsafe { GetDpiForWindow(di.hwndItem) }.max(96) as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let selected = di.itemState.0 & ODS_SELECTED.0 != 0;
    unsafe {
        let bg = CreateSolidBrush(rgb_ref(&p.bg));
        FillRect(hdc, &rc, bg);
        let _ = DeleteObject(bg.into());
        if selected {
            let sel = CreateSolidBrush(rgb_ref(&p.cardb));
            let pen = GetStockObject(NULL_PEN);
            let (ob, op) = (SelectObject(hdc, sel.into()), SelectObject(hdc, pen));
            let _ = RoundRect(hdc, rc.left + px(2.0), rc.top + px(2.0), rc.right - px(2.0), rc.bottom - px(2.0), px(8.0), px(8.0));
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(sel.into());
            let acc = CreateSolidBrush(rgb_ref(&accent_now()));
            let bar = RECT { left: rc.left + px(2.0), top: rc.top + px(11.0), right: rc.left + px(5.0), bottom: rc.bottom - px(11.0) };
            FillRect(hdc, &bar, acc);
            let _ = DeleteObject(acc.into());
        }
        SetBkMode(hdc, TRANSPARENT);
        // アイコン（Segoe Fluent Icons。なければ Segoe MDL2 Assets）
        let mut name = [0u16; 32];
        for (i, c) in "Segoe Fluent Icons".encode_utf16().enumerate() {
            name[i] = c;
        }
        let lf = LOGFONTW { lfHeight: -px(16.0), lfCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET, lfFaceName: name, ..Default::default() };
        let icon_font = CreateFontIndirectW(&lf);
        let old = SelectObject(hdc, icon_font.into());
        let acc = accent_now();
        SetTextColor(hdc, rgb_ref(if selected { &acc } else { &p.dim }));
        let mut g: Vec<u16> = ICONS[cat.min(8)].encode_utf16().collect();
        let mut r = RECT { left: rc.left + px(16.0), top: rc.top, right: rc.left + px(36.0), bottom: rc.bottom };
        DrawTextW(hdc, &mut g, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
        let _ = DeleteObject(icon_font.into());
        // 名前
        let font = HFONT(UI_FONT.load(Ordering::Relaxed) as *mut _);
        let old = SelectObject(hdc, font.into());
        SetTextColor(hdc, rgb_ref(&p.text));
        let mut t: Vec<u16> = CATS[cat].encode_utf16().collect();
        let mut r = RECT { left: rc.left + px(44.0), top: rc.top, right: rc.right - px(8.0), bottom: rc.bottom };
        DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(hdc, old);
    }
}

/// カード・スイッチ・文字を、背景として描く（子ウィンドウの部品は、この上に重なる）
unsafe fn paint_bg(hwnd: HWND, hdc: HDC) {
    thread_local! {
        static FACTORY: RefCell<Option<ID2D1Factory>> = const { RefCell::new(None) };
    }
    unsafe {
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let pal = pal();
        let (rows, toggles, card_w, sel) = CTRLS.with(|c| {
            c.borrow().as_ref().map_or((Vec::new(), HashMap::new(), CARD_W, HashMap::new()), |ct| {
                (ct.rows.iter().filter(|r| r.page == ct.cur).map(|r| (r.y, r.title, r.desc, r.toggle, r.h, r.preview, r.heading, r.choice)).collect::<Vec<_>>(), ct.toggles.clone(), ct.card_w, ct.sel.clone())
            })
        });
        let acc = accent_now();
        let factory = FACTORY.with(|f| {
            let mut f = f.borrow_mut();
            if f.is_none() {
                *f = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok();
            }
            f.clone()
        });
        let px = |v: i32| v as f32 * scale;
        let mut drawn = false;
        if let Some(factory) = factory {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            if let Ok(rt) = factory.CreateDCRenderTarget(&props) {
                if rt.BindDC(hdc, &rc).is_ok() {
                    rt.BeginDraw();
                    rt.Clear(Some(&pal.bg));
                    if let Ok(brush) = rt.CreateSolidColorBrush(&pal.card, None) {
                        for &(y, _, _, toggle, rh, _, heading, choice) in &rows {
                            if heading {
                                continue;
                            }
                            let (x0, y0) = (px(PAGE_X), px(PAGE_Y + y));
                            let (x1, y1) = (x0 + px(card_w), y0 + px(rh));
                            brush.SetColor(&pal.cardb);
                            rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, px(8)), &brush);
                            brush.SetColor(&pal.card);
                            rt.FillRoundedRectangle(&rrect(x0 + 1.0, y0 + 1.0, x1 - 1.0, y1 - 1.0, px(8) - 1.0), &brush);
                            if let Some(id) = toggle {
                                let on = toggles.get(&id).copied().unwrap_or(false);
                                let (tw, th) = (px(40), px(20));
                                let (tx1, ty0) = (x1 - px(16), (y0 + y1) / 2.0 - th / 2.0);
                                let tx0 = tx1 - tw;
                                if on {
                                    brush.SetColor(&acc);
                                    rt.FillRoundedRectangle(&rrect(tx0, ty0, tx1, ty0 + th, th / 2.0), &brush);
                                } else {
                                    brush.SetColor(&pal.dim);
                                    rt.DrawRoundedRectangle(&rrect(tx0 + 0.5, ty0 + 0.5, tx1 - 0.5, ty0 + th - 0.5, th / 2.0), &brush, 1.0, None);
                                }
                                let r = px(5);
                                let kx = if on { tx1 - px(10) - 1.0 } else { tx0 + px(10) + 1.0 };
                                brush.SetColor(if on { &pal.knob } else { &pal.dim });
                                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: kx, Y: ty0 + th / 2.0 }, radiusX: r, radiusY: r }, &brush);
                            }
                            // 選択肢：切り替え（システム・ライト・ダーク）と、色の見本
                            if let Some((first, kind, n)) = choice {
                                let selected = sel.get(&first).copied();
                                let rects = choice_rects(y, rh, card_w, kind, n);
                                if kind == 0 {
                                    if let (Some(a), Some(b)) = (rects.first(), rects.last()) {
                                        brush.SetColor(&pal.dim);
                                        rt.DrawRoundedRectangle(&rrect(px(a.0) + 0.5, px(a.1) + 0.5, px(b.2) - 0.5, px(b.3) - 0.5, px(6)), &brush, 1.0, None);
                                    }
                                }
                                for (i, &(a0, a1, a2, a3)) in rects.iter().enumerate() {
                                    let on = selected == Some(i);
                                    if kind == 0 {
                                        if on {
                                            brush.SetColor(&D2D1_COLOR_F { a: 0.28, ..acc });
                                            rt.FillRoundedRectangle(&rrect(px(a0) + 2.0, px(a1) + 2.0, px(a2) - 2.0, px(a3) - 2.0, px(5)), &brush);
                                            brush.SetColor(&acc);
                                            rt.DrawRoundedRectangle(&rrect(px(a0) + 2.5, px(a1) + 2.5, px(a2) - 2.5, px(a3) - 2.5, px(5)), &brush, 1.0, None);
                                        }
                                    } else {
                                        let (cx, cy, r) = ((px(a0) + px(a2)) / 2.0, (px(a1) + px(a3)) / 2.0, px(11));
                                        brush.SetColor(&swatch_color(i));
                                        rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, &brush);
                                        if on {
                                            brush.SetColor(&pal.text);
                                            rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r + px(3), radiusY: r + px(3) }, &brush, 1.5, None);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    drawn = rt.EndDraw(None, None).is_ok();
                }
            }
        }
        if !drawn {
            FillRect(hdc, &rc, HBRUSH(BG_BRUSH[DARK.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) as *mut _));
        }
        // 題と説明（GDI の文字）
        let font = HFONT(UI_FONT.load(Ordering::Relaxed) as *mut _);
        let old = SelectObject(hdc, font.into());
        SetBkMode(hdc, TRANSPARENT);
        let lh = {
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(hdc, &mut tm);
            tm.tmHeight.max(12)
        };
        for &(y, title, desc, toggle, rh, preview, heading, choice) in &rows {
            if heading {
                SetTextColor(hdc, rgb_ref(&pal.dim));
                let mut t: Vec<u16> = title.encode_utf16().collect();
                let mut r = RECT { left: px(PAGE_X + 4) as i32, top: px(PAGE_Y + y) as i32 + px(4) as i32, right: px(PAGE_X + card_w) as i32, bottom: px(PAGE_Y + y + 26) as i32 };
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX);
                continue;
            }
            if let Some((_, 0, n)) = choice {
                // 切り替えの文字
                for (i, (a0, a1, a2, a3)) in choice_rects(y, rh, card_w, 0, n).into_iter().enumerate() {
                    SetTextColor(hdc, rgb_ref(&pal.text));
                    let mut t: Vec<u16> = ["システム", "ライト", "ダーク"][i.min(2)].encode_utf16().collect();
                    let mut r = RECT { left: px(a0) as i32, top: px(a1) as i32, right: px(a2) as i32, bottom: px(a3) as i32 };
                    DrawTextW(hdc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
            if preview {
                draw_preview(hdc, px(PAGE_X + 16) as i32, px(PAGE_Y + y) as i32, px(rh) as i32, scale, pal);
                continue;
            }
            let x0 = px(PAGE_X + 16) as i32;
            let y0 = px(PAGE_Y + y) as i32;
            let (ty, dy) = if desc.is_empty() { (y0 + (px(ROW_H) as i32 - lh) / 2, 0) } else { (y0 + px(10) as i32, y0 + px(10) as i32 + lh + px(2) as i32) };
            // スイッチの行はスイッチの手前まで、入力欄のある行は入力欄の手前まで
            let limit = if toggle.is_some() { px(PAGE_X + card_w - 76) as i32 } else { x0 + px(card_w - 32 - 330) as i32 };
            SetTextColor(hdc, rgb_ref(&pal.text));
            let mut t: Vec<u16> = title.encode_utf16().collect();
            let mut r = RECT { left: x0, top: ty, right: limit.max(x0 + 40), bottom: ty + lh };
            DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            if !desc.is_empty() {
                SetTextColor(hdc, rgb_ref(&pal.dim));
                let mut d: Vec<u16> = desc.encode_utf16().collect();
                let mut r = RECT { left: x0, top: dy, right: limit.max(x0 + 40), bottom: dy + lh };
                DrawTextW(hdc, &mut d, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            }
        }
        SelectObject(hdc, old);
    }
}

unsafe extern "system" fn dlg_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if code == BN_CLICKED && is_toggle(id) {
                    flip_checked(id);
                }
                on_command(hwnd, id, code);
                LRESULT(0)
            }
            WM_SIZE => {
                relayout(hwnd);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                // 小さくしすぎて、カードが崩れないように
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let scale = dpi as f32 / 96.0;
                let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
                let mut rc = RECT { left: 0, top: 0, right: (640.0 * scale) as i32, bottom: (460.0 * scale) as i32 };
                let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
                mmi.ptMinTrackSize = POINT { x: rc.right - rc.left, y: rc.bottom - rc.top };
                LRESULT(0)
            }
            WM_CLOSE => {
                commit_edits();
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                paint_bg(hwnd, HDC(wparam.0 as *mut _));
                LRESULT(1)
            }
            WM_SETCURSOR => {
                // スイッチの上は手の形
                let child = HWND(wparam.0 as *mut _);
                let id = GetDlgCtrlID(child);
                if id != 0 && is_toggle(id) {
                    SetCursor(LoadCursorW(None, IDC_HAND).ok());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DRAWITEM => {
                // カードの上に重ねた透明なボタン：フォーカスがあるときだけ、枠を描く（ないときはカードの色で消す）
                let di = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if di.CtlType == ODT_LISTBOX {
                    draw_cat_item(di);
                    return LRESULT(1);
                }
                let p = pal();
                let focused = di.itemState.0 & ODS_FOCUS.0 != 0;
                let acc = accent_now();
                let brush = CreateSolidBrush(rgb_ref(if focused { &acc } else { &p.card }));
                FrameRect(di.hDC, &di.rcItem, brush);
                let _ = DeleteObject(brush.into());
                LRESULT(1)
            }
            WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let hdc = HDC(wparam.0 as *mut _);
                let (p, i) = (pal(), DARK.load(Ordering::Relaxed) as usize);
                if msg == WM_CTLCOLORLISTBOX {
                    SetTextColor(hdc, rgb_ref(&p.text));
                    SetBkColor(hdc, rgb_ref(&p.bg));
                    return LRESULT(BG_BRUSH[i].load(Ordering::Relaxed));
                }
                if msg == WM_CTLCOLOREDIT {
                    let bg = if i == 1 { rgb(0x1f, 0x1f, 0x1f) } else { rgb(0xff, 0xff, 0xff) };
                    SetTextColor(hdc, rgb_ref(&p.text));
                    SetBkColor(hdc, rgb_ref(&bg));
                    return LRESULT(EDIT_BRUSH[i].load(Ordering::Relaxed));
                }
                // 見出し・説明の文字：背景はそのまま見せる
                SetBkMode(hdc, TRANSPARENT);
                let is_title = HWND(lparam.0 as *mut _) == ctl(ID_TITLE);
                SetTextColor(hdc, rgb_ref(if is_title { &p.text } else { &p.dim }));
                // 静的な文字と、カードの上に重ねた透明なボタンは、下のカードを塗りつぶさない
                if msg == WM_CTLCOLORSTATIC || msg == WM_CTLCOLORBTN {
                    return LRESULT(GetStockObject(NULL_BRUSH).0 as isize);
                }
                LRESULT(BG_BRUSH[i].load(Ordering::Relaxed))
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
