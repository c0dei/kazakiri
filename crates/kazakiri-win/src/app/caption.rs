//! ウィンドウの幅（DIP）。タイトルバーとメニューバーは標準のものを使う。

use super::*;

impl App {
    pub(super) fn client_w_dip(&self) -> f32 {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right - rc.left) as f32 / (unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0)
    }
}
