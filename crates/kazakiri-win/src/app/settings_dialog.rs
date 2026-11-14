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
use windows::Win32::UI::Controls::Dialogs::{ChooseFontW, CHOOSEFONTW, CF_INITTOLOGFONTSTRUCT, CF_SCREENFONTS};
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

const ID_DARK: i32 = 201;
const ID_TRAY: i32 = 226;
const ID_MODEBAR: i32 = 227;
const ID_FONT: i32 = 204;
const ID_FONT_PICK: i32 = 205;
const ID_SIZE: i32 = 206;
const ID_UIFONT: i32 = 207;
const ID_UIFONT_PICK: i32 = 208;
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
    (ID_DARK, "ダークテーマ(&D)", menu::ID_DARK),
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
}

/// ダイアログの配色（Windows 11 の設定アプリに合わせた、背景・カード・文字・アクセント）
struct Pal {
    bg: D2D1_COLOR_F,
    card: D2D1_COLOR_F,
    cardb: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    dim: D2D1_COLOR_F,
    acc: D2D1_COLOR_F,
    knob: D2D1_COLOR_F,
}

const PAL_DARK: Pal = Pal { bg: rgb(0x20, 0x20, 0x20), card: rgb(0x2b, 0x2b, 0x2b), cardb: rgb(0x38, 0x38, 0x38), text: rgb(0xf2, 0xf2, 0xf2), dim: rgb(0x9a, 0x9a, 0x9a), acc: rgb(0x60, 0xcd, 0xff), knob: rgb(0x11, 0x11, 0x11) };
const PAL_LIGHT: Pal = Pal { bg: rgb(0xf3, 0xf3, 0xf3), card: rgb(0xfb, 0xfb, 0xfb), cardb: rgb(0xe5, 0xe5, 0xe5), text: rgb(0x1a, 0x1a, 0x1a), dim: rgb(0x6b, 0x6b, 0x6b), acc: rgb(0x00, 0x67, 0xc0), knob: rgb(0xff, 0xff, 0xff) };

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
    set_checked(ID_DARK, s.dark);
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
        let all: Vec<HWND> = CTRLS.with(|c| c.borrow().as_ref().map(|ct| ct.all.iter().map(|(_, h)| *h).collect()).unwrap_or_default());
        for h in all {
            let _ = SetWindowTheme(h, if dark { w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
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

/// フォント選択ダイアログ
unsafe fn pick_font(dlg: HWND, ui: bool) {
    unsafe {
        let (id_font, id_size) = if ui { (ID_UIFONT, ID_UISIZE) } else { (ID_FONT, ID_SIZE) };
        let name = text_of(id_font);
        let size = text_of(id_size).trim().parse::<f32>().unwrap_or(14.0);
        let mut lf = LOGFONTW { lfHeight: -(size.round() as i32 * 96 / 72), lfCharSet: windows::Win32::Graphics::Gdi::SHIFTJIS_CHARSET, ..Default::default() };
        for (i, c) in name.encode_utf16().take(31).enumerate() {
            lf.lfFaceName[i] = c;
        }
        let mut cf = CHOOSEFONTW {
            lStructSize: std::mem::size_of::<CHOOSEFONTW>() as u32,
            hwndOwner: dlg,
            lpLogFont: &mut lf,
            Flags: CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT,
            ..Default::default()
        };
        if ChooseFontW(&mut cf).as_bool() {
            let n = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
            set_text(id_font, &String::from_utf16_lossy(&lf.lfFaceName[..n]));
            set_text(id_size, &format!("{}", (cf.iPointSize / 10).max(1)));
            commit_edits();
        }
    }
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
        if matches!(id, ID_FONT | ID_SIZE | ID_TAB | ID_UIFONT | ID_UISIZE | ID_WHEEL | ID_BKDIR | ID_AUTOMIN) {
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
            ID_FONT_PICK => return pick_font(dlg, false),
            ID_UIFONT_PICK => return pick_font(dlg, true),
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
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
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
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), 0, dpi).is_ok() {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        };
        let push = BS_PUSHBUTTON as u32 | WS_TABSTOP.0;
        let edit = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
        let mut all: Vec<(i32, HWND)> = Vec::new();
        let mut pages: Vec<(i32, usize, String)> = Vec::new();
        let mut rows: Vec<Row> = Vec::new();
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
        // 1 行（カード）を足す。戻り値は、その行の上端（ダイアログ内の座標）。検索用に、題と説明も覚える
        let card = |rows: &mut Vec<Row>, page: usize, idx: i32, title: &'static str, desc: &'static str, toggle: Option<i32>| -> i32 {
            let y = idx * (ROW_H + ROW_GAP);
            rows.push(Row { page, y, title, desc, toggle, h: ROW_H, preview: false });
            PAGE_Y + y
        };
        // 入力欄の縦の位置（カードの中で縦中央）
        let mid = |top: i32, h: i32| top + (ROW_H - h) / 2;
        let right = PAGE_X + CARD_W - 16;
        let flat = WS_BORDER.0;
        put(ALL, ID_SEARCH, w!("EDIT"), "", edit | flat, 0, 12, 14, 156, 26);
        put(ALL, ID_CATS, w!("LISTBOX"), "", (LBS_NOTIFY as u32) | (LBS_OWNERDRAWFIXED as u32) | (LBS_HASSTRINGS as u32) | WS_TABSTOP.0, 0, 12, 52, 164, 420);
        put(ALL, ID_TITLE, w!("STATIC"), "", 0, 0, PAGE_X, 12, 480, 34);
        // 基本
        card(&mut rows, 0, 0, "長い行を折り返して表示", "画面の幅で折り返す（文字単位）", Some(ID_WRAP));
        card(&mut rows, 0, 1, "IME の変換中の文字をエディタ内に表示", "オフなら IME 標準の窓", Some(ID_IME));
        let y = card(&mut rows, 0, 2, "タブの幅", "半角の文字数", None);
        put(0, ID_TAB, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        // スクロール
        let y = card(&mut rows, 1, 0, "マウスホイール", "1 回で動かす行数", None);
        put(1, ID_WHEEL, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        // 外観
        card(&mut rows, 2, 0, "ダークテーマ", "タイトルバー・スクロールバー・メニューも暗くなる", Some(ID_DARK));
        let y = card(&mut rows, 2, 1, "本文のフォント", "空欄で自動。右は大きさ (pt)", None);
        put(2, ID_FONT, w!("EDIT"), "", edit | flat, 0, right - 60 - 8 - 72 - 8 - 150, mid(y, 26), 150, 26);
        put(2, ID_FONT_PICK, w!("BUTTON"), "選択...", push, 0, right - 60 - 8 - 72, mid(y, 28), 72, 28);
        put(2, ID_SIZE, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        let y = card(&mut rows, 2, 2, "画面のフォント", "メニュー・タブなど。空欄で自動", None);
        put(2, ID_UIFONT, w!("EDIT"), "", edit | flat, 0, right - 60 - 8 - 72 - 8 - 150, mid(y, 26), 150, 26);
        put(2, ID_UIFONT_PICK, w!("BUTTON"), "選択...", push, 0, right - 60 - 8 - 72, mid(y, 28), 72, 28);
        put(2, ID_UISIZE, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        // フォントの見え方（設定を変えると、すぐ変わる）
        rows.push(Row { page: 2, y: 3 * (ROW_H + ROW_GAP), title: "プレビュー", desc: "", toggle: None, h: 92, preview: true });
        // 表示
        card(&mut rows, 3, 0, "空白・タブ・改行を表示", "印で見えるようにする（Ctrl+Shift+W）", Some(ID_WS));
        card(&mut rows, 3, 1, "ログの色分け", "ERROR / WARN などに色を付ける", Some(ID_LOG));
        card(&mut rows, 3, 2, "構文の色分け", "JSON・SQL・XML など", Some(ID_SYN));
        card(&mut rows, 3, 3, "ツールバー", "画面の上のボタンの行", Some(ID_TOOLBAR));
        card(&mut rows, 3, 4, "モードバー", "CSV・Markdown のときの操作の行", Some(ID_MODEBAR));
        card(&mut rows, 3, 5, "機能ボタンの列", "ステータスバーの上の「検索 Ctrl+F …」", Some(ID_FKEYS));
        card(&mut rows, 3, 6, "通知領域のアイコン", "クリックで新規作成・開くなど", Some(ID_TRAY));
        // 検索・GREP
        card(&mut rows, 4, 0, "GREP 置換の前にバックアップ", ".bak を作る", Some(ID_BACKUP));
        // バックアップ
        card(&mut rows, 5, 0, "上書き保存の前にバックアップ", "元のファイルを残す", Some(ID_BKSAVE));
        let y = card(&mut rows, 5, 1, "バックアップを置くフォルダ", "空欄で、元のファイルと同じ", None);
        put(5, ID_BKDIR, w!("EDIT"), "", edit | flat, 0, right - 80 - 8 - 190, mid(y, 26), 190, 26);
        put(5, ID_BKDIR_PICK, w!("BUTTON"), "参照...", push, 0, right - 80, mid(y, 28), 80, 28);
        card(&mut rows, 5, 2, "古いバックアップを残す", ".bak2、.bak3 … と番号を増やす（オフなら .bak を上書き）", Some(ID_BKNUM));
        put(5, 41, w!("STATIC"), "バックアップは、名前を付けて保存・変換して保存を含む、既存のファイルを書き換える保存の直前に作ります。", 0, 0, PAGE_X, PAGE_Y + 3 * (ROW_H + ROW_GAP) + 4, CARD_W, 40);
        // 自動保存
        let y = card(&mut rows, 6, 0, "自動保存の間隔", "分。0 で自動保存しない", None);
        put(6, ID_AUTOMIN, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        put(6, 43, w!("STATIC"), "未保存の編集を、『名前.autosave』に書き出します（元のファイルは書き換えません）。保存するか、通常どおり閉じると消え、異常終了したときだけ残ります。バックアップのフォルダを指定していれば、そこに書き出します。", 0, 0, PAGE_X, PAGE_Y + (ROW_H + ROW_GAP) + 4, CARD_W, 80);
        // Markdown
        card(&mut rows, 7, 0, "開いたときにプレビューを並べる", "Markdown ファイル（.md）を開いたとき、右に並べる", Some(ID_MDAUTO));
        put(7, 16, w!("STATIC"), "プレビューは、表示メニューやモードバーのボタン（Ctrl+Shift+V：右に並べる、Ctrl+Shift+G：デザインビュー）でも切り替えられます。", 0, 0, PAGE_X, PAGE_Y + (ROW_H + ROW_GAP) + 4, CARD_W, 40);
        // キーボード
        let y = card(&mut rows, 8, 0, "キー割り当て", "コマンドごとのキーを、一覧から選んで変える", None);
        put(8, ID_KEYS, w!("BUTTON"), "変更...", push, 0, right - 90, mid(y, 28), 90, 28);
        put(ALL, ID_CLOSE, w!("BUTTON"), "閉じる", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0, 0, 620, 520, 90, 30);
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
        // スイッチの行ごとに、カードの上に透明なボタンを重ねる（Tab で止まり、Space で切り替わる。クリックもこのボタンが受ける）
        for r in &rows {
            if let Some(id) = r.toggle {
                let hw = make_child(dlg, w!("BUTTON"), "", BS_OWNERDRAW as u32 | WS_TABSTOP.0, 0, id, font);
                let (x, y) = (PAGE_X + 4, PAGE_Y + r.y + 4);
                let _ = SetWindowPos(hw, None, p(x), p(y), p(CARD_W - 8), p(ROW_H - 8), SWP_NOZORDER | SWP_NOACTIVATE);
                all.push((id, hw));
                pages.push((id, r.page, String::new()));
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
        CTRLS.with(|c| *c.borrow_mut() = Some(Ctrls { rows, toggles: HashMap::new(), all, pages, visible: Vec::new(), cur: 0 }));
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
            let acc = CreateSolidBrush(rgb_ref(&p.acc));
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
        SetTextColor(hdc, rgb_ref(if selected { &p.acc } else { &p.dim }));
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
        let (rows, toggles) = CTRLS.with(|c| {
            c.borrow().as_ref().map_or((Vec::new(), HashMap::new()), |ct| {
                (ct.rows.iter().filter(|r| r.page == ct.cur).map(|r| (r.y, r.title, r.desc, r.toggle, r.h, r.preview)).collect::<Vec<_>>(), ct.toggles.clone())
            })
        });
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
                        for &(y, _, _, toggle, rh, _) in &rows {
                            let (x0, y0) = (px(PAGE_X), px(PAGE_Y + y));
                            let (x1, y1) = (x0 + px(CARD_W), y0 + px(rh));
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
                                    brush.SetColor(&pal.acc);
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
        for &(y, title, desc, toggle, rh, preview) in &rows {
            if preview {
                draw_preview(hdc, px(PAGE_X + 16) as i32, px(PAGE_Y + y) as i32, px(rh) as i32, scale, pal);
                continue;
            }
            let x0 = px(PAGE_X + 16) as i32;
            let y0 = px(PAGE_Y + y) as i32;
            let (ty, dy) = if desc.is_empty() { (y0 + (px(ROW_H) as i32 - lh) / 2, 0) } else { (y0 + px(10) as i32, y0 + px(10) as i32 + lh + px(2) as i32) };
            // スイッチの行はスイッチの手前まで、入力欄のある行は入力欄の手前まで
            let limit = if toggle.is_some() { px(PAGE_X + CARD_W - 76) as i32 } else { x0 + px(CARD_W - 32 - 300) as i32 };
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
                let brush = CreateSolidBrush(rgb_ref(if focused { &p.acc } else { &p.card }));
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
