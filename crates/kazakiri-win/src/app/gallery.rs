//! 標準部品ギャラリー（試験用の画面。`kazakiri.exe --gallery` で開く）。
//!
//! 設定画面と同じ配色の「カード」（単色で背景を塗る子ウィンドウ）の上に、標準部品を、方式を変えて並べる。
//! ライト／ダークで画面を撮って、部品ごとに、実機で成立する方式を決めるためのもの（`docs/STANDARD_CONTROLS.md`）。
//! 通常の起動では使わない。

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::*;

use super::find_dialog::{make_child, rgb_ref, wide};
use super::*;

static GDARK: AtomicBool = AtomicBool::new(false);
/// 配色ごとの [背景, カード] のブラシ（0 = ライト、1 = ダーク）
static BR_BG: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
static BR_CARD: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
static UI_FONT: AtomicIsize = AtomicIsize::new(0);

const ID_TOGGLE: i32 = 1;
/// NM_CUSTOMDRAW で文字色を渡す部品の ID
const ID_CUSTOMDRAW: i32 = 103;

fn dark() -> bool {
    GDARK.load(Ordering::Relaxed)
}
fn bg() -> COLORREF {
    if dark() { COLORREF(0x0020_2020) } else { COLORREF(0x00f3_f3f3) }
}
fn card() -> COLORREF {
    if dark() { COLORREF(0x002b_2b2b) } else { COLORREF(0x00fb_fbfb) }
}
fn text() -> COLORREF {
    if dark() { COLORREF(0x00f2_f2f2) } else { COLORREF(0x001a_1a1a) }
}
fn brush(a: &[AtomicIsize; 2]) -> HBRUSH {
    HBRUSH(a[dark() as usize].load(Ordering::Relaxed) as *mut _)
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES | ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES | ICC_UPDOWN_CLASS };
        let _ = InitCommonControlsEx(&icc);
        GDARK.store(theme::system_prefers_dark(), Ordering::Relaxed);
        for (i, (b, c)) in [(0x00f3_f3f3u32, 0x00fb_fbfbu32), (0x0020_2020, 0x002b_2b2b)].into_iter().enumerate() {
            BR_BG[i].store(CreateSolidBrush(COLORREF(b)).0 as isize, Ordering::Relaxed);
            BR_CARD[i].store(CreateSolidBrush(COLORREF(c)).0 as isize, Ordering::Relaxed);
        }
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let main_class = w!("KazakiriGallery");
        let card_class = w!("KazakiriGalleryCard");
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(main_proc), hInstance: instance.into(), hCursor: cursor, lpszClassName: main_class, ..Default::default() });
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(card_proc), hInstance: instance.into(), hCursor: cursor, lpszClassName: card_class, ..Default::default() });
        let title = wide(&format!("標準部品ギャラリー  ({})", option_env!("GITHUB_SHA").map_or("local", |s| s.get(..7).unwrap_or(s))));
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), main_class, PCWSTR(title.as_ptr()), WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN, 40, 40, 1000, 900, None, None, Some(instance.into()), None) else { return };
        build(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn ui_font(dpi: i32) -> HFONT {
    unsafe {
        let old = UI_FONT.load(Ordering::Relaxed);
        if old != 0 {
            let _ = DeleteObject(HGDIOBJ(old as *mut _));
        }
        let mut face = [0u16; 32];
        for (i, c) in "Yu Gothic UI".encode_utf16().enumerate() {
            face[i] = c;
        }
        let f = CreateFontIndirectW(&LOGFONTW { lfHeight: -(15 * dpi / 96), lfWeight: 400, lfCharSet: DEFAULT_CHARSET, lfFaceName: face, ..Default::default() });
        UI_FONT.store(f.0 as isize, Ordering::Relaxed);
        f
    }
}

/// 子ウィンドウをすべて作り直す（ライト／ダークの切り替えのたび）
unsafe fn build(main: HWND) {
    unsafe {
        // いまの子を消す
        while let Ok(child) = GetWindow(main, GW_CHILD) {
            if child.0.is_null() {
                break;
            }
            let _ = DestroyWindow(child);
        }
        let dpi = GetDpiForWindow(main).max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        let font = ui_font(dpi);
        let d = dark();
        let v = BOOL::from(d);
        let _ = DwmSetWindowAttribute(main, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
        let theme_of = |h: HWND, name: PCWSTR| {
            if d {
                let _ = SetWindowTheme(h, name, PCWSTR::null());
            }
        };
        let place = |h: HWND, x: i32, y: i32, w: i32, hh: i32| {
            let _ = SetWindowPos(h, None, s(x), s(y), s(w), s(hh), SWP_NOZORDER | SWP_NOACTIVATE);
        };
        let top = make_child(main, w!("BUTTON"), if d { "ライトにする" } else { "ダークにする" }, BS_PUSHBUTTON as u32 | WS_TABSTOP.0, 0, ID_TOGGLE, font);
        place(top, 16, 12, 140, 30);
        let note = make_child(main, w!("STATIC"), "各カードの左が方式の名前。方式ごとの見え方（背景・文字・箱・操作）を見比べる。切り替えてもう 1 枚撮る", 0, 0, 2, font);
        place(note, 170, 18, 800, 22);

        // 2 列に並べる。部品の種類ごとに、新しい行から始める
        let col = std::cell::Cell::new(0);
        let y = std::cell::Cell::new(56);
        let last_h = std::cell::Cell::new(44);
        let next = |h: i32| -> (i32, i32, i32) {
            let r = (16 + col.get() * 484, y.get(), 468);
            col.set(col.get() + 1);
            last_h.set(h);
            if col.get() == 2 {
                col.set(0);
                y.set(y.get() + h + 8);
            }
            r
        };
        let new_group = || {
            if col.get() == 1 {
                col.set(0);
                y.set(y.get() + last_h.get() + 8);
            }
        };
        // カード 1 枚：(x, y, 幅) と高さ、方式の名前。戻り値はカードのウィンドウ（部品はこの子にする）
        let card_win = |pos: (i32, i32, i32), h: i32, label: &str| -> HWND {
            let c = CreateWindowExW(WINDOW_EX_STYLE(0), w!("KazakiriGalleryCard"), PCWSTR::null(), WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN, s(pos.0), s(pos.1), s(pos.2), s(h), Some(main), None, GetModuleHandleW(None).ok().map(|m| m.into()), None).unwrap_or_default();
            let l = make_child(c, w!("STATIC"), label, 0, 0, 3, font);
            let _ = SetWindowPos(l, None, s(10), s(10), s(250), s(24), SWP_NOZORDER | SWP_NOACTIVATE);
            c
        };
        let put = |h: HWND, x: i32, yy: i32, w: i32, hh: i32| {
            let _ = SetWindowPos(h, None, s(x), s(yy), s(w), s(hh), SWP_NOZORDER | SWP_NOACTIVATE);
        };

        // ---- チェックボックス ----
        let variants: [(&str, i32); 5] = [("チェック 1：標準そのまま", 101), ("チェック 2：DarkMode_Explorer", 102), ("チェック 3：Explorer＋文字色（NM_CUSTOMDRAW）", ID_CUSTOMDRAW), ("チェック 4：テーマなし（クラシック）", 104), ("チェック 5：サブクラスで描画（設定画面の方式）", 105)];
        for (label, id) in variants {
            let c = card_win(next(44), 44, label);
            let cb = make_child(c, w!("BUTTON"), "題の文字", BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0, 0, id, font);
            put(cb, 270, 10, 190, 24);
            SendMessageW(cb, BM_SETCHECK, Some(WPARAM(1)), None);
            match id {
                102 | ID_CUSTOMDRAW => theme_of(cb, w!("DarkMode_Explorer")),
                104 => {
                    let _ = SetWindowTheme(cb, w!(""), w!(""));
                }
                105 => {
                    theme_of(cb, w!("DarkMode_Explorer"));
                    super::settings_dialog::attach_check_subclass(cb, &BR_CARD, d);
                }
                _ => {}
            }
        }
        // ---- コンボボックス ----
        new_group();
        for (label, id, th) in [("コンボ 1：標準そのまま", 111, ""), ("コンボ 2：DarkMode_Explorer", 112, "DarkMode_Explorer"), ("コンボ 3：DarkMode_CFD", 113, "DarkMode_CFD")] {
            let c = card_win(next(44), 44, label);
            let cb = make_child(c, w!("COMBOBOX"), "", CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0, 0, id, font);
            put(cb, 270, 8, 190, 200);
            for t in ["システム", "ライト", "ダーク"] {
                let w = wide(t);
                SendMessageW(cb, CB_ADDSTRING, None, Some(LPARAM(w.as_ptr() as isize)));
            }
            SendMessageW(cb, CB_SETCURSEL, Some(WPARAM(0)), None);
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(cb, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
        }
        // ---- 入力欄・増減 ----
        new_group();
        for (label, id, th, ex) in [("入力 1：標準（枠あり）", 121, "", 0u32), ("入力 2：DarkMode_CFD＋クライアント枠", 122, "DarkMode_CFD", WS_EX_CLIENTEDGE.0), ("入力 3：DarkMode_Explorer", 123, "DarkMode_Explorer", 0)] {
            let c = card_win(next(44), 44, label);
            let style = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0 | if ex == 0 { WS_BORDER.0 } else { 0 };
            let e = make_child(c, w!("EDIT"), "入力欄の文字", style, ex, id, font);
            put(e, 270, 8, 190, 28);
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(e, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
        }
        for (label, id, th) in [("増減 1：標準", 131, ""), ("増減 2：Explorer（入力欄は CFD）", 132, "DarkMode_Explorer")] {
            let c = card_win(next(44), 44, label);
            let e = make_child(c, w!("EDIT"), "4", ES_NUMBER as u32 | WS_BORDER.0 | WS_TABSTOP.0, 0, id, font);
            put(e, 270, 8, 90, 28);
            let ud = make_child(c, w!("msctls_updown32"), "", UDS_ALIGNRIGHT | UDS_SETBUDDYINT | UDS_ARROWKEYS, 0, id + 100, font);
            SendMessageW(ud, UDM_SETBUDDY, Some(WPARAM(e.0 as usize)), None);
            SendMessageW(ud, UDM_SETRANGE32, Some(WPARAM(1)), Some(LPARAM(32)));
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(ud, PCWSTR(n.as_ptr()), PCWSTR::null());
                let _ = SetWindowTheme(e, w!("DarkMode_CFD"), PCWSTR::null());
            }
        }
        // ---- ボタン ----
        new_group();
        for (label, id, th) in [("ボタン 1：標準", 141, ""), ("ボタン 2：DarkMode_Explorer", 142, "DarkMode_Explorer")] {
            let c = card_win(next(44), 44, label);
            let b = make_child(c, w!("BUTTON"), "参照...", BS_PUSHBUTTON as u32 | WS_TABSTOP.0, 0, id, font);
            put(b, 270, 8, 100, 28);
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(b, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
        }
        // ---- リストビュー ----
        new_group();
        for (label, id, th) in [("リスト 1：標準", 151, ""), ("リスト 2：Explorer＋色を指定", 152, "DarkMode_Explorer")] {
            let c = card_win(next(100), 100, label);
            let lv = make_child(c, w!("SysListView32"), "", LVS_LIST | LVS_SINGLESEL | WS_TABSTOP.0, 0, id, font);
            put(lv, 270, 8, 190, 84);
            for (i, t) in ["基本", "スクロール", "外観", "表示"].into_iter().enumerate() {
                let mut w = wide(t);
                let item = LVITEMW { mask: LVIF_TEXT, iItem: i as i32, pszText: windows::core::PWSTR(w.as_mut_ptr()), ..Default::default() };
                SendMessageW(lv, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const _ as isize)));
            }
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(lv, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
            if id == 152 {
                SendMessageW(lv, LVM_SETBKCOLOR, None, Some(LPARAM(card().0 as isize)));
                SendMessageW(lv, LVM_SETTEXTBKCOLOR, None, Some(LPARAM(card().0 as isize)));
                SendMessageW(lv, LVM_SETTEXTCOLOR, None, Some(LPARAM(text().0 as isize)));
            }
        }
        // ---- タブ ----
        new_group();
        for (label, id, th) in [("タブ 1：標準", 161, ""), ("タブ 2：Explorer", 162, "DarkMode_Explorer")] {
            let c = card_win(next(80), 80, label);
            let tab = make_child(c, w!("SysTabControl32"), "", WS_TABSTOP.0, 0, id, font);
            put(tab, 270, 8, 190, 64);
            for (i, t) in ["a.log", "（無題）", "people.csv"].into_iter().enumerate() {
                let mut w = wide(t);
                let item = TCITEMW { mask: TCIF_TEXT, pszText: windows::core::PWSTR(w.as_mut_ptr()), ..Default::default() };
                SendMessageW(tab, TCM_INSERTITEMW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
            }
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(tab, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
        }
        // ---- ツールバー ----
        new_group();
        for (label, id, th) in [("ツールバー 1：標準", 171, ""), ("ツールバー 2：Explorer", 172, "DarkMode_Explorer")] {
            let c = card_win(next(44), 44, label);
            let tb = make_child(c, w!("ToolbarWindow32"), "", TBSTYLE_FLAT | TBSTYLE_LIST | CCS_NODIVIDER as u32 | CCS_NORESIZE as u32, 0, id, font);
            put(tb, 270, 6, 190, 32);
            SendMessageW(tb, TB_BUTTONSTRUCTSIZE, Some(WPARAM(std::mem::size_of::<TBBUTTON>())), None);
            let mut labels: Vec<u16> = Vec::new();
            for t in ["新規", "開く", "保存"] {
                labels.extend(t.encode_utf16());
                labels.push(0);
            }
            labels.push(0);
            SendMessageW(tb, TB_ADDSTRINGW, None, Some(LPARAM(labels.as_ptr() as isize)));
            let buttons: Vec<TBBUTTON> = (0..3).map(|i| TBBUTTON { iBitmap: -2, idCommand: 500 + i, fsState: TBSTATE_ENABLED as u8, fsStyle: BTNS_AUTOSIZE as u8, iString: i as isize, ..Default::default() }).collect();
            SendMessageW(tb, TB_ADDBUTTONSW, Some(WPARAM(buttons.len())), Some(LPARAM(buttons.as_ptr() as isize)));
            if !th.is_empty() && d {
                let n = wide(th);
                let _ = SetWindowTheme(tb, PCWSTR(n.as_ptr()), PCWSTR::null());
            }
        }
        let _ = InvalidateRect(Some(main), None, true);
        let _ = (rgb_ref, bg());
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(HDC(wparam.0 as *mut _), &rc, brush(&BR_BG));
                LRESULT(1)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let hdc = HDC(wparam.0 as *mut _);
                SetTextColor(hdc, text());
                SetBkColor(hdc, bg());
                LRESULT(brush(&BR_BG).0 as isize)
            }
            WM_COMMAND => {
                if (wparam.0 & 0xFFFF) as i32 == ID_TOGGLE {
                    GDARK.store(!dark(), Ordering::Relaxed);
                    build(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// カード：単色で背景を塗る子ウィンドウ。標準部品が「親の背景を借りる」とき、借り先がこの単色になる
unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(HDC(wparam.0 as *mut _), &rc, brush(&BR_CARD));
                LRESULT(1)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let hdc = HDC(wparam.0 as *mut _);
                SetTextColor(hdc, text());
                SetBkColor(hdc, if msg == WM_CTLCOLOREDIT { if dark() { COLORREF(0x001f_1f1f) } else { COLORREF(0x00ff_ffff) } } else { card() });
                SetBkMode(hdc, OPAQUE);
                LRESULT(brush(&BR_CARD).0 as isize)
            }
            WM_NOTIFY => {
                let nm = &*(lparam.0 as *const NMHDR);
                if nm.code == NM_CUSTOMDRAW && nm.idFrom as i32 == ID_CUSTOMDRAW {
                    let cd = &*(lparam.0 as *const NMCUSTOMDRAW);
                    if cd.dwDrawStage == CDDS_PREPAINT {
                        SetTextColor(cd.hdc, text());
                    }
                    return LRESULT(CDRF_DODEFAULT as isize);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
