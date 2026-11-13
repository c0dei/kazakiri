//! ダークテーマのメニュー（ポップアップ）とスクロールバー（Windows 10 1903 以降）。
//!
//! Windows には公式のダークモード API がないため、uxtheme.dll の非公開関数（序数 133・135・136）を使う。
//! 見つからなければ何もしない（明るいままになるだけ）。メニューバーはこの方法では暗くならないので、
//! 標準のメニューバーを暗くするには、非公開メッセージ `WM_UAHDRAWMENU` / `WM_UAHDRAWMENUITEM` で自分で塗る。
//! このメッセージは `DrawMenuBar` などの中で同期的に届き、`App` を借りている最中のこともあるので、
//! `wndproc` の最初（`App` を借りる前）で、色を入れたグローバル変数だけを使って処理する。
//! それでも暗くならない環境のために、自前で描くメニューバー（`menubar = custom`）も残してある。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::core::{PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Controls::{SetWindowTheme, DRAWITEMSTRUCT};

use super::*;

unsafe fn uxtheme_ordinal<T: Copy>(n: u16) -> Option<T> {
    unsafe {
        let module = GetModuleHandleW(windows::core::w!("uxtheme.dll"))
            .or_else(|_| LoadLibraryExW(windows::core::w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32))
            .ok()?;
        let f = GetProcAddress(module, PCSTR(n as usize as *const u8))?;
        Some(std::mem::transmute_copy::<_, T>(&f))
    }
}

/// メニューとスクロールバーを暗く（または元に）する
pub(super) unsafe fn apply(hwnd: HWND, dark: bool) {
    unsafe {
        // SetPreferredAppMode(AllowDark=1 / ForceDark=2 / Default=0)
        if let Some(f) = uxtheme_ordinal::<unsafe extern "system" fn(i32) -> i32>(135) {
            f(if dark { 2 } else { 0 });
        }
        // AllowDarkModeForWindow
        if let Some(f) = uxtheme_ordinal::<unsafe extern "system" fn(HWND, BOOL) -> BOOL>(133) {
            let _ = f(hwnd, BOOL::from(dark));
        }
        // 窓のスクロールバー
        let _ = SetWindowTheme(hwnd, if dark { windows::core::w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
        // FlushMenuThemes
        if let Some(f) = uxtheme_ordinal::<unsafe extern "system" fn()>(136) {
            f();
        }
        let _ = DrawMenuBar(hwnd);
    }
}

// ---- 標準のメニューバーを暗く塗る ----

/// 標準のメニューバーを暗く塗っているか
static BAR_DARK: AtomicBool = AtomicBool::new(false);
static BAR_BG: AtomicU32 = AtomicU32::new(0);
static BAR_HOT: AtomicU32 = AtomicU32::new(0);
static BAR_TEXT: AtomicU32 = AtomicU32::new(0);

/// メニューバーの非公開メッセージ（lParam は `UahMenu`、`UahDrawMenuItem`）
const WM_UAHDRAWMENU: u32 = 0x0091;
const WM_UAHDRAWMENUITEM: u32 = 0x0092;
const WM_NCPAINT: u32 = 0x0085;
const WM_NCACTIVATE: u32 = 0x0086;

#[repr(C)]
struct UahMenu {
    hmenu: HMENU,
    hdc: HDC,
    flags: u32,
}

/// 実際の構造体はこのあとにも続くが、使うのは先頭の 3 つだけ
#[repr(C)]
struct UahDrawMenuItem {
    dis: DRAWITEMSTRUCT,
    um: UahMenu,
    position: i32,
}

const ODS_SELECTED: u32 = 0x0001;
const ODS_HOTLIGHT: u32 = 0x0040;
const ODS_NOACCEL: u32 = 0x0100;

fn colorref(c: &D2D1_COLOR_F) -> u32 {
    let b = |v: f32| (v * 255.0).round() as u32;
    b(c.r) | (b(c.g) << 8) | (b(c.b) << 16)
}

/// 標準のメニューバーを暗く塗る（`on`）か、塗らない（Windows に任せる）かを決める
pub(super) fn set_bar(on: bool, theme: &Theme) {
    BAR_BG.store(colorref(&theme.gutter_bg), Ordering::Relaxed);
    BAR_HOT.store(colorref(&theme.panel_head), Ordering::Relaxed);
    BAR_TEXT.store(colorref(&theme.text), Ordering::Relaxed);
    BAR_DARK.store(on, Ordering::Relaxed);
}

/// `wndproc` の最初で呼ぶ。メニューバー関係のメッセージを処理したら、その結果を返す
pub(super) unsafe fn bar_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if !BAR_DARK.load(Ordering::Relaxed) {
        return None;
    }
    unsafe {
        match msg {
            WM_UAHDRAWMENU => {
                let um = &*(lparam.0 as *const UahMenu);
                let mut mbi = MENUBARINFO { cbSize: std::mem::size_of::<MENUBARINFO>() as u32, ..Default::default() };
                if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi).is_err() {
                    return None;
                }
                let mut window = RECT::default();
                let _ = GetWindowRect(hwnd, &mut window);
                let mut rc = mbi.rcBar;
                let _ = OffsetRect(&mut rc, -window.left, -window.top);
                let brush = CreateSolidBrush(COLORREF(BAR_BG.load(Ordering::Relaxed)));
                FillRect(um.hdc, &rc, brush);
                let _ = DeleteObject(brush.into());
                Some(LRESULT(1))
            }
            WM_UAHDRAWMENUITEM => {
                let it = &*(lparam.0 as *const UahDrawMenuItem);
                let mut text = [0u16; 128];
                let mut mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_STRING,
                    dwTypeData: PWSTR(text.as_mut_ptr()),
                    cch: text.len() as u32 - 1,
                    ..Default::default()
                };
                if GetMenuItemInfoW(it.um.hmenu, it.position as u32, true, &mut mii).is_err() {
                    return None;
                }
                let state = it.dis.itemState.0;
                let hot = state & (ODS_HOTLIGHT | ODS_SELECTED) != 0;
                let brush = CreateSolidBrush(COLORREF(BAR_HOT.load(Ordering::Relaxed)));
                let bg = CreateSolidBrush(COLORREF(BAR_BG.load(Ordering::Relaxed)));
                FillRect(it.um.hdc, &it.dis.rcItem, if hot { brush } else { bg });
                let _ = DeleteObject(brush.into());
                let _ = DeleteObject(bg.into());
                SetBkMode(it.um.hdc, TRANSPARENT);
                SetTextColor(it.um.hdc, COLORREF(BAR_TEXT.load(Ordering::Relaxed)));
                let mut rc = it.dis.rcItem;
                let flags = DT_CENTER | DT_SINGLELINE | DT_VCENTER | if state & ODS_NOACCEL != 0 { DT_HIDEPREFIX } else { DRAW_TEXT_FORMAT(0) };
                let n = (mii.cch as usize).min(text.len());
                DrawTextW(it.um.hdc, &mut text[..n], &mut rc, flags);
                Some(LRESULT(1))
            }
            // メニューバーの下に出る 1 ピクセルの明るい線は、非クライアント領域の描画のあとで塗りつぶす
            WM_NCPAINT | WM_NCACTIVATE => {
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                paint_bar_line(hwnd);
                Some(r)
            }
            _ => None,
        }
    }
}

unsafe fn paint_bar_line(hwnd: HWND) {
    unsafe {
        let mut client = RECT::default();
        let mut window = RECT::default();
        if GetClientRect(hwnd, &mut client).is_err() || GetWindowRect(hwnd, &mut window).is_err() {
            return;
        }
        let mut origin = POINT { x: client.left, y: client.top };
        let _ = ClientToScreen(hwnd, &mut origin);
        let line = RECT { left: origin.x - window.left, right: origin.x - window.left + (client.right - client.left), top: origin.y - window.top - 1, bottom: origin.y - window.top };
        let hdc = GetWindowDC(Some(hwnd));
        if hdc.is_invalid() {
            return;
        }
        let brush = CreateSolidBrush(COLORREF(BAR_BG.load(Ordering::Relaxed)));
        FillRect(hdc, &line, brush);
        let _ = DeleteObject(brush.into());
        ReleaseDC(Some(hwnd), hdc);
    }
}
