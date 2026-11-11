//! ダークテーマのメニューバー・メニュー・スクロールバー（Windows 10 1903 以降）。
//!
//! Windows には公式のダークモード API がないため、次の非公開の仕組みを使う。見つからなければ
//! 何もしない（明るいままになるだけ）。
//!
//! - uxtheme.dll の関数（序数 133・135・136）：メニューとスクロールバーを暗くする
//! - `WM_UAHDRAWMENU` / `WM_UAHDRAWMENUITEM`：メニューバーを自分で塗る（バーは上の方法では暗くならない）
//!
//! 動作は Windows 10/11 の実機で確認する必要がある（Wine にはどちらもない）。

use windows::core::{PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Controls::{SetWindowTheme, DRAWITEMSTRUCT};

use super::*;

/// メニューバーを塗れという非公開メッセージ（lParam は `UahMenu`）
pub(super) const WM_UAHDRAWMENU: u32 = 0x0091;
/// メニューバーの項目を塗れという非公開メッセージ（lParam は `UahDrawMenuItem`）
pub(super) const WM_UAHDRAWMENUITEM: u32 = 0x0092;

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

fn colorref(c: &D2D1_COLOR_F) -> COLORREF {
    let b = |v: f32| (v * 255.0).round() as u32;
    COLORREF(b(c.r) | (b(c.g) << 8) | (b(c.b) << 16))
}

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

/// メニューバーの背景を塗る
pub(super) unsafe fn draw_menu_bar(hwnd: HWND, lparam: LPARAM, theme: &Theme) -> bool {
    unsafe {
        let um = &*(lparam.0 as *const UahMenu);
        let mut mbi = MENUBARINFO { cbSize: std::mem::size_of::<MENUBARINFO>() as u32, ..Default::default() };
        if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi).is_err() {
            return false;
        }
        let mut window = RECT::default();
        let _ = GetWindowRect(hwnd, &mut window);
        let mut rc = mbi.rcBar;
        let _ = OffsetRect(&mut rc, -window.left, -window.top);
        let brush = CreateSolidBrush(colorref(&theme.gutter_bg));
        FillRect(um.hdc, &rc, brush);
        let _ = DeleteObject(brush.into());
        true
    }
}

/// メニューバーの項目（「ファイル」など）を塗る
pub(super) unsafe fn draw_menu_item(lparam: LPARAM, theme: &Theme) {
    unsafe {
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
            return;
        }
        let state = it.dis.itemState.0;
        let hot = state & (ODS_HOTLIGHT | ODS_SELECTED) != 0;
        let brush = CreateSolidBrush(colorref(if hot { &theme.panel_head } else { &theme.gutter_bg }));
        FillRect(it.um.hdc, &it.dis.rcItem, brush);
        let _ = DeleteObject(brush.into());
        SetBkMode(it.um.hdc, TRANSPARENT);
        SetTextColor(it.um.hdc, colorref(&theme.text));
        let mut rc = it.dis.rcItem;
        let flags = DT_CENTER | DT_SINGLELINE | DT_VCENTER | if state & ODS_NOACCEL != 0 { DT_HIDEPREFIX } else { DRAW_TEXT_FORMAT(0) };
        let n = (mii.cch as usize).min(text.len());
        DrawTextW(it.um.hdc, &mut text[..n], &mut rc, flags);
    }
}

/// メニューバーの下に出る 1 ピクセルの明るい線を塗りつぶす
pub(super) unsafe fn paint_bar_line(hwnd: HWND, theme: &Theme) {
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
        let brush = CreateSolidBrush(colorref(&theme.gutter_bg));
        FillRect(hdc, &line, brush);
        let _ = DeleteObject(brush.into());
        ReleaseDC(Some(hwnd), hdc);
    }
}
