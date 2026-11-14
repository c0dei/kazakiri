//! ダークテーマのポップアップメニューとスクロールバー（Windows 10 1903 以降）。
//!
//! Windows には公式のダークモード API がないため、uxtheme.dll の非公開関数（序数 133・135・136）を使う。
//! 見つからなければ何もしない（明るいままになるだけ）。標準のメニューバーはもう使わない（☰ から開くポップアップメニュー）。

use windows::core::{BOOL, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Controls::SetWindowTheme;

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
        // 窓のスクロールバー（子ウィンドウのスクロールバーも）
        let _ = SetWindowTheme(hwnd, if dark { windows::core::w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
        unsafe extern "system" fn theme_child(child: HWND, l: LPARAM) -> BOOL {
            unsafe {
                let _ = SetWindowTheme(child, if l.0 != 0 { windows::core::w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
            }
            BOOL(1)
        }
        let _ = EnumChildWindows(Some(hwnd), Some(theme_child), LPARAM(dark as isize));
        // FlushMenuThemes
        if let Some(f) = uxtheme_ordinal::<unsafe extern "system" fn()>(136) {
            f();
        }
        let _ = DrawMenuBar(hwnd);
    }
}
