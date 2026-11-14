//! 設定ダイアログ（ファイル → 設定）。
//!
//! モードレスの 1 枚のウィンドウ。左に分類の一覧と検索欄（EmEditor の「すべての設定のプロパティ」のように、
//! 検索語に当てはまる項目を含む分類だけに絞る）、右に選んだ分類のページを出す。チェックボックスは押すとすぐ反映され、
//! `settings.ini` に保存される（表示メニューと同じ操作を呼ぶだけ）。フォント・大きさ・タブ幅の入力欄は、
//! 欄から離れたとき・閉じたとき・Enter で反映する。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::Dialogs::{ChooseFontW, CHOOSEFONTW, CF_INITTOLOGFONTSTRUCT, CF_SCREENFONTS};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};

use super::find_dialog::{make_child, rgb_ref, wide};
use super::*;

/// ダイアログのウィンドウ（0 ならまだない）。メッセージループの `IsDialogMessage` が見る
pub(super) static SET_DLG: AtomicIsize = AtomicIsize::new(0);
static DARK: AtomicBool = AtomicBool::new(false);
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
static FIRST: AtomicBool = AtomicBool::new(true);

const CLASS: PCWSTR = w!("KazakiriSettingsDialog");
const EN_KILLFOCUS_CODE: u32 = 0x0200;

const ID_DARK: i32 = 201;
const ID_MICA: i32 = 202;
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
const CATS: [&str; 7] = ["基本", "スクロール", "外観", "表示", "検索・GREP", "Markdown", "キーボード"];
const EN_CHANGE_CODE: u32 = 0x0300;
const LBN_SELCHANGE_CODE: u32 = 1;

struct Ctrls {
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
    unsafe {
        SendMessageW(ctl(id), BM_SETCHECK, Some(WPARAM(if on { BST_CHECKED.0 } else { BST_UNCHECKED.0 } as usize)), None);
    }
}

/// アプリの今の設定を、ダイアログの各部品に写す
fn sync() {
    let Some(s) = with_app(|app| app.settings.clone()) else { return };
    set_checked(ID_DARK, s.dark);
    set_checked(ID_MICA, s.mica);
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
    with_app(|app| {
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
        if matches!(id, ID_FONT | ID_SIZE | ID_TAB | ID_UIFONT | ID_UISIZE | ID_WHEEL) {
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
                let on = SendMessageW(ctl(ID_TRAY), BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                with_app(|app| app.set_tray(on));
            }
            ID_MDAUTO => {
                let on = SendMessageW(ctl(ID_MDAUTO), BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                with_app(|app| {
                    app.settings.md_auto = on;
                    app.settings.save();
                });
            }
            ID_MICA => {
                let on = SendMessageW(ctl(ID_MICA), BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                with_app(|app| app.set_mica(on));
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
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
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
        let check = BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0;
        let push = BS_PUSHBUTTON as u32 | WS_TABSTOP.0;
        let edit = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
        let mut all: Vec<(i32, HWND)> = Vec::new();
        let mut pages: Vec<(i32, usize, String)> = Vec::new();
        // 分類のページの左上（ダイアログ内の座標）
        let (ox, oy) = (184, 46);
        // page: 分類の番号（usize::MAX は、どの分類でも出す部品）。x, y はページの左上から
        let mut put = |page: usize, id: i32, class: PCWSTR, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, h: i32| {
            let hw = make_child(dlg, class, text, style, ex, id, font);
            let p = |v: i32| (v as f32 * scale).round() as i32;
            let (x, y) = if page == usize::MAX { (x, y) } else { (x + ox, y + oy) };
            let _ = SetWindowPos(hw, None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
            all.push((id, hw));
            if page != usize::MAX {
                pages.push((id, page, text.replace('&', "").to_lowercase()));
            }
        };
        const ALL: usize = usize::MAX;
        put(ALL, ID_SEARCH, w!("EDIT"), "", edit, WS_EX_CLIENTEDGE.0, 12, 12, 156, 24);
        put(ALL, ID_CATS, w!("LISTBOX"), "", (LBS_NOTIFY as u32) | WS_TABSTOP.0 | WS_VSCROLL.0, WS_EX_CLIENTEDGE.0, 12, 44, 156, 380);
        put(ALL, ID_TITLE, w!("STATIC"), "", 0, 0, 184, 14, 480, 22);
        // 基本
        put(0, ID_WRAP, w!("BUTTON"), CHECKS[1].1, check, 0, 0, 0, 440, 22);
        put(0, ID_IME, w!("BUTTON"), CHECKS[2].1, check, 0, 0, 26, 440, 22);
        put(0, 12, w!("STATIC"), "タブの幅（半角の文字数）", 0, 0, 0, 60, 180, 18);
        put(0, ID_TAB, w!("EDIT"), "", edit | ES_NUMBER as u32, WS_EX_CLIENTEDGE.0, 190, 56, 50, 24);
        // スクロール
        put(1, 15, w!("STATIC"), "マウスホイール 1 回で動かす行数", 0, 0, 0, 4, 230, 18);
        put(1, ID_WHEEL, w!("EDIT"), "", edit | ES_NUMBER as u32, WS_EX_CLIENTEDGE.0, 240, 0, 50, 24);
        // 外観
        put(2, ID_DARK, w!("BUTTON"), CHECKS[0].1, check, 0, 0, 0, 440, 22);
        put(2, ID_MICA, w!("BUTTON"), "Mica（Windows 11 のタイトルバーを半透明に）(&C)", check, 0, 0, 26, 440, 22);
        put(2, 10, w!("STATIC"), "本文のフォント（空欄で自動）", 0, 0, 0, 62, 220, 18);
        put(2, 11, w!("STATIC"), "大きさ (pt)", 0, 0, 312, 62, 100, 18);
        put(2, ID_FONT, w!("EDIT"), "", edit, WS_EX_CLIENTEDGE.0, 0, 82, 220, 24);
        put(2, ID_FONT_PICK, w!("BUTTON"), "選択...(&F)", push, 0, 228, 82, 72, 24);
        put(2, ID_SIZE, w!("EDIT"), "", edit | ES_NUMBER as u32, WS_EX_CLIENTEDGE.0, 312, 82, 60, 24);
        put(2, 13, w!("STATIC"), "画面（メニュー・タブなど）のフォント（空欄で自動）", 0, 0, 0, 120, 340, 18);
        put(2, 14, w!("STATIC"), "大きさ (pt)", 0, 0, 312, 140, 100, 18);
        put(2, ID_UIFONT, w!("EDIT"), "", edit, WS_EX_CLIENTEDGE.0, 0, 142, 220, 24);
        put(2, ID_UIFONT_PICK, w!("BUTTON"), "選択...(&G)", push, 0, 228, 142, 72, 24);
        put(2, ID_UISIZE, w!("EDIT"), "", edit | ES_NUMBER as u32, WS_EX_CLIENTEDGE.0, 312, 162, 60, 24);
        // 表示
        put(3, ID_WS, w!("BUTTON"), CHECKS[3].1, check, 0, 0, 0, 440, 22);
        put(3, ID_LOG, w!("BUTTON"), CHECKS[4].1, check, 0, 0, 26, 440, 22);
        put(3, ID_SYN, w!("BUTTON"), CHECKS[5].1, check, 0, 0, 52, 440, 22);
        put(3, ID_TOOLBAR, w!("BUTTON"), CHECKS[6].1, check, 0, 0, 78, 440, 22);
        put(3, ID_MODEBAR, w!("BUTTON"), "モードバー（CSV の区切りなどのボタンの行）を表示(&E)", check, 0, 0, 104, 440, 22);
        put(3, ID_FKEYS, w!("BUTTON"), CHECKS[8].1, check, 0, 0, 130, 440, 22);
        put(3, ID_TRAY, w!("BUTTON"), "通知領域（タスクトレイ）にアイコンを表示（クリックで新規作成・開くなど）(&T)", check, 0, 0, 156, 440, 22);
        // 検索・GREP
        put(4, ID_BACKUP, w!("BUTTON"), CHECKS[9].1, check, 0, 0, 0, 440, 22);
        // Markdown
        put(5, ID_MDAUTO, w!("BUTTON"), "Markdown ファイル（.md）を開いたとき、右にプレビューを並べる(&A)", check, 0, 0, 0, 460, 22);
        put(5, 16, w!("STATIC"), "プレビューは、表示メニューやモードバーのボタン（Ctrl+Shift+V：右に並べる、Ctrl+Shift+G：デザインビュー）でも切り替えられます。", 0, 0, 0, 34, 460, 40);
        // キーボード
        put(6, ID_KEYS, w!("BUTTON"), "キー割り当て...(&K)", push, 0, 0, 0, 200, 28);
        put(6, 17, w!("STATIC"), "コマンドごとのキーを、一覧から選んで変えられます。", 0, 0, 0, 38, 460, 18);
        put(ALL, ID_CLOSE, w!("BUTTON"), "閉じる", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0, 0, 598, 436, 90, 28);
        let (bg, edit_bg) = (CreateSolidBrush(rgb_ref(&super::theme::DARK.gutter_bg)), CreateSolidBrush(rgb_ref(&super::theme::DARK.bg)));
        BG_BRUSH.store(bg.0 as isize, Ordering::Relaxed);
        EDIT_BRUSH.store(edit_bg.0 as isize, Ordering::Relaxed);
        CTRLS.with(|c| *c.borrow_mut() = Some(Ctrls { all, pages, visible: Vec::new(), cur: 0 }));
        fill_cats("");
        SET_DLG.store(dlg.0 as isize, Ordering::Relaxed);
        // クライアント領域を 472 × 630 に
        let style = WINDOW_STYLE(GetWindowLongW(dlg, GWL_STYLE) as u32);
        let p = |v: i32| (v as f32 * scale).round() as i32;
        let mut rc = RECT { left: 0, top: 0, right: p(700), bottom: p(476) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let _ = SetWindowPos(dlg, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe extern "system" fn dlg_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                on_command(hwnd, id, code);
                LRESULT(0)
            }
            WM_CLOSE => {
                commit_edits();
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                let hdc = HDC(wparam.0 as *mut _);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                if DARK.load(Ordering::Relaxed) {
                    FillRect(hdc, &rc, HBRUSH(BG_BRUSH.load(Ordering::Relaxed) as *mut _));
                } else {
                    FillRect(hdc, &rc, GetSysColorBrush(COLOR_BTNFACE));
                }
                LRESULT(1)
            }
            WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                if !DARK.load(Ordering::Relaxed) {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let hdc = HDC(wparam.0 as *mut _);
                let t = &super::theme::DARK;
                let editable = msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX;
                SetTextColor(hdc, rgb_ref(&t.text));
                SetBkColor(hdc, rgb_ref(if editable { &t.bg } else { &t.gutter_bg }));
                LRESULT(if editable { EDIT_BRUSH.load(Ordering::Relaxed) } else { BG_BRUSH.load(Ordering::Relaxed) })
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
