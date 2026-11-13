//! クリップボードへの書き込み（Unicode テキスト）。

use windows::core::Result;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// 履歴（新しい順）。このアプリでコピー・切り取りした文字列と、貼り付けた文字列
static HISTORY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const HISTORY_MAX: usize = 20;
/// 履歴に残す 1 件の上限
const HISTORY_ITEM_MAX: usize = 1 << 20;

pub fn remember(text: &str) {
    if text.is_empty() || text.len() > HISTORY_ITEM_MAX {
        return;
    }
    if let Ok(mut h) = HISTORY.lock() {
        h.retain(|t| t != text);
        h.insert(0, text.to_string());
        h.truncate(HISTORY_MAX);
    }
}

pub fn history() -> Vec<String> {
    HISTORY.lock().map(|h| h.clone()).unwrap_or_default()
}

pub fn set_text(hwnd: HWND, text: &str) -> Result<()> {
    remember(text);
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let bytes = units.len() * 2;
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
            let _ = GlobalUnlock(mem);
            // 成功したら所有権はシステムに移る
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}
