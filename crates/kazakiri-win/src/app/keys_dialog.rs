//! キー割り当ての設定ウィンドウ（ファイル → キー割り当て）。
//!
//! コマンドの一覧から選んで、割り当てたいキーを押すだけで変えられる（keys.ini を手で書かなくてよい）。
//! OK で keys.ini に保存して、すぐ反映する（メニューに出るキーの表示は、再起動後に変わる）。
//! すでに別のコマンドが使っているキーを割り当てると、そちらを外す。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::core::PWSTR;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};

use super::find_dialog::{make_child, rgb_ref, wide};
use super::*;
use crate::keymap::{self, Binding};

pub(super) static KEYS_DLG: AtomicIsize = AtomicIsize::new(0);
static DARK: AtomicBool = AtomicBool::new(false);
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
static FIRST: AtomicBool = AtomicBool::new(true);

const CLASS: PCWSTR = w!("KazakiriKeysDialog");

const ID_LIST: i32 = 301;
const ID_HOTKEY: i32 = 302;
const ID_SET: i32 = 303;
const ID_ADD: i32 = 304;
const ID_CLEAR: i32 = 305;
const ID_DEFAULT: i32 = 306;
const ID_ALL_DEFAULT: i32 = 307;
const ID_NOTE: i32 = 308;
const ID_PROMPT: i32 = 309;
const ID_OK: i32 = 310;
const ID_CANCEL: i32 = 311;

struct State {
    all: Vec<(i32, HWND)>,
    bindings: Vec<Binding>,
    /// 一覧に出すコマンド名（メニューの文言）
    labels: Vec<String>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn ctl(id: i32) -> HWND {
    STATE.with(|s| s.borrow().as_ref().and_then(|st| st.all.iter().find(|(i, _)| *i == id).map(|(_, h)| *h)).unwrap_or_default())
}

fn set_note(s: &str) {
    let w = wide(s);
    unsafe {
        let _ = SetWindowTextW(ctl(ID_NOTE), PCWSTR(w.as_ptr()));
    }
}

fn keys_text(b: &Binding) -> String {
    if b.keys.is_empty() {
        "（なし）".to_string()
    } else {
        b.keys.iter().map(|k| keymap::display(k)).collect::<Vec<_>>().join(" / ")
    }
}

/// 一覧の 1 行を、いまの割り当てに合わせる
fn refresh_row(i: usize) {
    let Some(text) = STATE.with(|s| s.borrow().as_ref().and_then(|st| st.bindings.get(i).map(keys_text))) else { return };
    let w = wide(&text);
    let item = LVITEMW { iSubItem: 1, pszText: PWSTR(w.as_ptr() as *mut _), ..Default::default() };
    unsafe {
        SendMessageW(ctl(ID_LIST), LVM_SETITEMTEXTW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
    }
}

fn selected() -> Option<usize> {
    let r = unsafe { SendMessageW(ctl(ID_LIST), LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_SELECTED as isize))) }.0;
    (r >= 0).then_some(r as usize)
}

/// メニューの文言から、一覧に出すコマンド名を作る
fn command_label(menu: HMENU, id: u32, fallback: &str) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetMenuStringW(menu, id, Some(&mut buf), MF_BYCOMMAND) } as usize;
    if n == 0 {
        return fallback.to_string();
    }
    let s = String::from_utf16_lossy(&buf[..n]);
    // 「保存(&S)」の「(&S)」と、「&F」のような記号を取り除く
    let s = s.split('\t').next().unwrap_or("");
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("(&") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find(')').map_or("", |j| &rest[i + j + 1..]);
    }
    out.push_str(rest);
    out.replace('&', "")
}

/// ファイルから読み直して、一覧を作り直す
fn reload() {
    let menu = with_app(|app| app.menus.bar).unwrap_or_default();
    let bindings = keymap::bindings();
    let labels: Vec<String> = bindings.iter().map(|b| command_label(menu, b.id, b.name)).collect();
    let list = ctl(ID_LIST);
    unsafe {
        SendMessageW(list, LVM_DELETEALLITEMS, None, None);
        for (i, l) in labels.iter().enumerate() {
            let w = wide(l);
            let item = LVITEMW { mask: LVIF_TEXT, iItem: i as i32, pszText: PWSTR(w.as_ptr() as *mut _), ..Default::default() };
            SendMessageW(list, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const _ as isize)));
        }
    }
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.bindings = bindings;
            st.labels = labels;
        }
    });
    let n = STATE.with(|s| s.borrow().as_ref().map_or(0, |st| st.bindings.len()));
    for i in 0..n {
        refresh_row(i);
    }
    set_note("コマンドを選んで、「キー」の欄を押してから割り当てたいキーを押し、「設定」を押してください。");
}

/// 同じキーを使っている、ほかのコマンドから外す。外したコマンドの名前を返す
fn steal(key: &str, except: usize) -> Vec<String> {
    let norm = keymap::display(key).to_ascii_lowercase();
    let mut taken = Vec::new();
    let mut rows = Vec::new();
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            for (i, b) in st.bindings.iter_mut().enumerate() {
                if i == except {
                    continue;
                }
                let before = b.keys.len();
                b.keys.retain(|k| keymap::display(k).to_ascii_lowercase() != norm);
                if b.keys.len() != before {
                    taken.push(st.labels[i].clone());
                    rows.push(i);
                }
            }
        }
    });
    for i in rows {
        refresh_row(i);
    }
    taken
}

fn apply_key(replace: bool) {
    let Some(i) = selected() else { return set_note("先に一覧からコマンドを選んでください。") };
    let hk = unsafe { SendMessageW(ctl(ID_HOTKEY), HKM_GETHOTKEY, None, None) }.0 as u32;
    let (vk_code, mods) = ((hk & 0xFF) as u16, (hk >> 8) & 0xFF);
    if vk_code == 0 {
        return set_note("「キー」の欄を押して、割り当てたいキーを押してください。");
    }
    if mods & 4 != 0 {
        return set_note("Alt を使うキーは割り当てられません。");
    }
    let key = match keymap::key_from_input(vk_code, mods & 2 != 0, mods & 1 != 0) {
        Ok(k) => k,
        Err(e) => return set_note(&format!("{e}（文字のキーは Ctrl と一緒に押してください）")),
    };
    let taken = steal(&key, i);
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let b = &mut st.bindings[i];
            if replace {
                b.keys = vec![key.clone()];
            } else if !b.keys.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
                b.keys.push(key.clone());
            }
        }
    });
    refresh_row(i);
    set_note(&if taken.is_empty() { format!("{} を割り当てました。", keymap::display(&key)) } else { format!("{} を割り当てました（「{}」から外しました）。", keymap::display(&key), taken.join("」「")) });
}

fn clear_selected() {
    let Some(i) = selected() else { return set_note("先に一覧からコマンドを選んでください。") };
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.bindings[i].keys.clear();
        }
    });
    refresh_row(i);
    set_note("割り当てを外しました。");
}

fn reset_selected() {
    let Some(i) = selected() else { return set_note("先に一覧からコマンドを選んでください。") };
    let defaults = STATE.with(|s| s.borrow().as_ref().map(|st| st.bindings[i].defaults.clone())).unwrap_or_default();
    let mut taken = Vec::new();
    for k in &defaults {
        taken.extend(steal(k, i));
    }
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.bindings[i].keys = defaults;
        }
    });
    refresh_row(i);
    set_note(&if taken.is_empty() { "既定に戻しました。".to_string() } else { format!("既定に戻しました（「{}」から外しました）。", taken.join("」「")) });
}

fn reset_all() {
    let n = STATE.with(|s| {
        let mut b = s.borrow_mut();
        let st = b.as_mut()?;
        for x in st.bindings.iter_mut() {
            x.keys = x.defaults.clone();
        }
        Some(st.bindings.len())
    });
    for i in 0..n.unwrap_or(0) {
        refresh_row(i);
    }
    set_note("すべて既定に戻しました（OK を押すと保存されます）。");
}

/// OK：keys.ini に書いて、すぐ反映する
fn save(dlg: HWND) {
    let text = STATE.with(|s| s.borrow().as_ref().map(|st| keymap::to_ini(&st.bindings))).unwrap_or_default();
    let Some(path) = keymap::config_path() else { return set_note("設定の保存先（APPDATA）が見つかりません。") };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        return set_note(&format!("保存できませんでした: {e}"));
    }
    with_app(|app| {
        app.keymap = keymap::Keymap::load();
        app.message = "キー割り当てを保存しました（メニューに出るキーの表示は、再起動後に変わります）".into();
        app.invalidate();
    });
    unsafe {
        let _ = ShowWindow(dlg, SW_HIDE);
    }
}

fn set_theme(dark: bool) {
    let dlg = HWND(KEYS_DLG.load(Ordering::Relaxed) as *mut _);
    if dlg.0.is_null() {
        return;
    }
    DARK.store(dark, Ordering::Relaxed);
    unsafe {
        let all: Vec<HWND> = STATE.with(|s| s.borrow().as_ref().map(|st| st.all.iter().map(|(_, h)| *h).collect()).unwrap_or_default());
        for h in all {
            let _ = SetWindowTheme(h, if dark { w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
        }
        let t = &super::theme::DARK;
        let list = ctl(ID_LIST);
        let (bg, fg) = if dark { (rgb_ref(&t.bg), rgb_ref(&t.text)) } else { (COLORREF(0x00FF_FFFF), COLORREF(0)) };
        SendMessageW(list, LVM_SETBKCOLOR, None, Some(LPARAM(bg.0 as isize)));
        SendMessageW(list, LVM_SETTEXTBKCOLOR, None, Some(LPARAM(bg.0 as isize)));
        SendMessageW(list, LVM_SETTEXTCOLOR, None, Some(LPARAM(fg.0 as isize)));
        let v = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(dlg, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// ダイアログを（なければ作って）開く
pub(super) unsafe fn show(owner: HWND) {
    unsafe {
        if KEYS_DLG.load(Ordering::Relaxed) == 0 {
            create(owner);
        }
        let dlg = HWND(KEYS_DLG.load(Ordering::Relaxed) as *mut _);
        if dlg.0.is_null() {
            return;
        }
        reload();
        set_theme(with_app(|app| app.settings.dark).unwrap_or(false));
        if FIRST.swap(false, Ordering::Relaxed) {
            let mut r = RECT::default();
            let _ = GetWindowRect(owner, &mut r);
            let _ = SetWindowPos(dlg, None, r.left + 70, r.top + 50, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
    }
}

unsafe fn create(owner: HWND) {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES | ICC_HOTKEY_CLASS };
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
            w!("キー割り当て"),
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
        let p = |v: i32| (v as f32 * scale).round() as i32;
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), 0, dpi).is_ok() {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        };
        let push = BS_PUSHBUTTON as u32 | WS_TABSTOP.0;
        let mut all: Vec<(i32, HWND)> = Vec::new();
        let mut put = |id: i32, class: PCWSTR, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, h: i32| {
            let hw = make_child(dlg, class, text, style, ex, id, font);
            let _ = SetWindowPos(hw, None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
            all.push((id, hw));
        };
        put(ID_LIST, w!("SysListView32"), "", LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | WS_TABSTOP.0, WS_EX_CLIENTEDGE.0, 12, 12, 536, 300);
        put(ID_PROMPT, w!("STATIC"), "キー（ここを押してから、割り当てたいキーを押す）", 0, 0, 12, 322, 400, 18);
        put(ID_HOTKEY, w!("msctls_hotkey32"), "", WS_TABSTOP.0, WS_EX_CLIENTEDGE.0, 12, 342, 200, 26);
        put(ID_SET, w!("BUTTON"), "設定(&S)", push, 0, 222, 342, 76, 26);
        put(ID_ADD, w!("BUTTON"), "追加(&A)", push, 0, 304, 342, 76, 26);
        put(ID_CLEAR, w!("BUTTON"), "外す(&R)", push, 0, 386, 342, 76, 26);
        put(ID_DEFAULT, w!("BUTTON"), "既定に(&D)", push, 0, 468, 342, 80, 26);
        put(ID_NOTE, w!("STATIC"), "", 0, 0, 12, 378, 536, 36);
        put(ID_ALL_DEFAULT, w!("BUTTON"), "すべて既定に戻す(&Z)", push, 0, 12, 424, 150, 28);
        put(ID_OK, w!("BUTTON"), "OK", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0, 0, 372, 424, 84, 28);
        put(ID_CANCEL, w!("BUTTON"), "キャンセル", push, 0, 464, 424, 84, 28);
        // 一覧の列
        let list = all.iter().find(|(i, _)| *i == ID_LIST).map(|(_, h)| *h).unwrap_or_default();
        SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, None, Some(LPARAM(LVS_EX_FULLROWSELECT as isize)));
        for (i, (name, w)) in [("コマンド", 250), ("キー", 260)].iter().enumerate() {
            let t = wide(name);
            let col = LVCOLUMNW { mask: LVCF_TEXT | LVCF_WIDTH, cx: p(*w), pszText: PWSTR(t.as_ptr() as *mut _), ..Default::default() };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        let (bg, edit_bg) = (CreateSolidBrush(rgb_ref(&super::theme::DARK.gutter_bg)), CreateSolidBrush(rgb_ref(&super::theme::DARK.bg)));
        BG_BRUSH.store(bg.0 as isize, Ordering::Relaxed);
        EDIT_BRUSH.store(edit_bg.0 as isize, Ordering::Relaxed);
        STATE.with(|s| *s.borrow_mut() = Some(State { all, bindings: Vec::new(), labels: Vec::new() }));
        KEYS_DLG.store(dlg.0 as isize, Ordering::Relaxed);
        let style = WINDOW_STYLE(GetWindowLongW(dlg, GWL_STYLE) as u32);
        let mut rc = RECT { left: 0, top: 0, right: p(560), bottom: p(464) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let _ = SetWindowPos(dlg, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe fn on_command(dlg: HWND, id: i32, code: u32) {
    if code != BN_CLICKED && id != IDOK.0 && id != IDCANCEL.0 {
        return;
    }
    match id {
        ID_SET => apply_key(true),
        ID_ADD => apply_key(false),
        ID_CLEAR => clear_selected(),
        ID_DEFAULT => reset_selected(),
        ID_ALL_DEFAULT => reset_all(),
        ID_OK | 1 => save(dlg),
        ID_CANCEL | 2 => unsafe {
            let _ = ShowWindow(dlg, SW_HIDE);
        },
        _ => {}
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
