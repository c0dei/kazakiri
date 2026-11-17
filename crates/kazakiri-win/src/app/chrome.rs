//! 画面の上下の部品のうち、標準の部品（Win32 のコモンコントロール）に替えたもの。
//!
//! いまは、ステータスバー（`msctls_statusbar32`）。本文より下の端に置き、左にメッセージ、右に行・桁・文字コードなどを並べる。
//! クリックできる部分（行・桁、文字コード、改行コード、タブ幅、挿入／上書き）は、部分（パート）ごとに分けてあり、クリックは `NM_CLICK` で受ける。
//! 色は、標準の部品がダークにならないので、各パートを自前で描く（`SBT_OWNERDRAW`）。

use windows::Win32::UI::Controls::*;

use super::find_dialog::rgb_ref;
use super::*;

/// ステータスバーの部品の ID
pub(super) const ID_STATUS: i32 = 9001;

/// 部品の左右の余白（DIP）
const PART_PAD: f32 = 8.0;

impl App {
    /// ステータスバーを作る（ウィンドウを作った直後に 1 回）
    pub(super) unsafe fn create_status(hwnd: HWND, instance: HINSTANCE) -> HWND {
        unsafe {
            let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES };
            let _ = InitCommonControlsEx(&icc);
            let h = CreateWindowExW(WINDOW_EX_STYLE(0), w!("msctls_statusbar32"), PCWSTR::null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, 0, 0, Some(hwnd), Some(HMENU(ID_STATUS as usize as *mut _)), Some(instance), None).unwrap_or_default();
            // 色を自分で決めるため、テーマの描画はやめる
            let _ = SetWindowTheme(h, w!(""), w!(""));
            h
        }
    }

    /// 画面の部品のフォント（設定の ui_font。なければ Yu Gothic UI）の HFONT
    unsafe fn ui_hfont(&self) -> HFONT {
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96) as f32;
            let height = -((self.settings.ui_font_size * dpi / 72.0).round() as i32);
            let name = if self.settings.ui_font.is_empty() { "Yu Gothic UI" } else { self.settings.ui_font.as_str() };
            let mut face = [0u16; 32];
            for (i, c) in name.encode_utf16().take(31).enumerate() {
                face[i] = c;
            }
            let lf = LOGFONTW { lfHeight: height, lfWeight: 400, lfCharSet: DEFAULT_CHARSET, lfFaceName: face, ..Default::default() };
            CreateFontIndirectW(&lf)
        }
    }

    /// ステータスバーを、ウィンドウの下端に置く（高さは `status_base_h`）
    pub(super) fn layout_status(&self) {
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let h = (self.status_base_h() * scale).round() as i32;
        let placed = [rc.right, rc.bottom, h];
        if self.status_placed.get() == placed {
            return;
        }
        self.status_placed.set(placed);
        unsafe {
            let _ = SetWindowPos(self.status, None, 0, rc.bottom - h, rc.right, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        // 幅が変わったので、パートの位置を作り直す
        self.status_sig.borrow_mut().clear();
    }

    /// ステータスバーの中身（左のメッセージと、右の行・桁・文字コードなど）を、変わったときだけ入れ直す
    pub(super) fn update_status(&self) {
        let left = self.status_left();
        let segs: Vec<(String, Option<u8>)> = self.status_segments().into_iter().filter(|(s, _)| !s.trim().is_empty()).collect();
        let sig = format!("{left}\u{1}{}", segs.iter().map(|(s, k)| format!("{s}\u{2}{k:?}")).collect::<Vec<_>>().join("\u{1}"));
        if *self.status_sig.borrow() == sig {
            return;
        }
        *self.status_sig.borrow_mut() = sig;
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        // 右のパートの幅（文字の幅 + 余白）。左のパートは、残り全部
        let widths: Vec<i32> = segs.iter().map(|(s, _)| (((unsafe { self.ui_text_width(&s.encode_utf16().collect::<Vec<_>>()) }.unwrap_or(0.0)) + PART_PAD * 2.0) * scale).ceil() as i32).collect();
        let right_total: i32 = widths.iter().sum();
        let mut edges = Vec::with_capacity(widths.len() + 1);
        let mut x = (rc.right - right_total).max(0);
        edges.push(x);
        for w in &widths {
            x += w;
            edges.push(x);
        }
        if let Some(last) = edges.last_mut() {
            *last = -1;
        }
        let mut texts = vec![left];
        let mut kinds = vec![None];
        for (s, k) in segs {
            texts.push(s);
            kinds.push(k);
        }
        unsafe {
            if self.status_font.get().is_invalid() {
                self.status_font.set(self.ui_hfont());
                SendMessageW(self.status, WM_SETFONT, Some(WPARAM(self.status_font.get().0 as usize)), Some(LPARAM(1)));
            }
            let bg = rgb_ref(&self.theme.status_bg);
            SendMessageW(self.status, SB_SETBKCOLOR, Some(WPARAM(0)), Some(LPARAM(bg.0 as isize)));
            SendMessageW(self.status, SB_SETPARTS, Some(WPARAM(edges.len())), Some(LPARAM(edges.as_ptr() as isize)));
            for i in 0..texts.len() {
                SendMessageW(self.status, SB_SETTEXTW, Some(WPARAM(i | SBT_OWNERDRAW as usize | SBT_NOBORDERS as usize)), Some(LPARAM(0)));
            }
            let _ = InvalidateRect(Some(self.status), None, true);
        }
        *self.status_texts.borrow_mut() = texts;
        *self.status_kinds.borrow_mut() = kinds;
    }

    /// ステータスバーの 1 パートを描く（WM_DRAWITEM）
    pub(super) unsafe fn draw_status_part(&self, di: &DRAWITEMSTRUCT) {
        unsafe {
            let bg = CreateSolidBrush(rgb_ref(&self.theme.status_bg));
            FillRect(di.hDC, &di.rcItem, bg);
            let _ = DeleteObject(bg.into());
            let texts = self.status_texts.borrow();
            let Some(text) = texts.get(di.itemID as usize) else { return };
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            SetBkMode(di.hDC, TRANSPARENT);
            SetTextColor(di.hDC, rgb_ref(&self.theme.status_text));
            let mut t: Vec<u16> = text.encode_utf16().collect();
            let pad = (PART_PAD * scale) as i32;
            let mut r = RECT { left: di.rcItem.left + pad, top: di.rcItem.top, right: di.rcItem.right - pad, bottom: di.rcItem.bottom };
            DrawTextW(di.hDC, &mut t, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        }
    }

    /// ステータスバーのパートをクリックした（NM_CLICK）。クリックで動くパートなら、その操作
    pub(super) unsafe fn status_click(&mut self, part: usize) -> Option<Deferred> {
        let kind = self.status_kinds.borrow().get(part).copied().flatten()?;
        match kind {
            2 => unsafe { self.command(menu::ID_GOTO) },
            4 => unsafe { self.command(menu::ID_OVERWRITE) },
            k => Some(Deferred::StatusMenu(k)),
        }
    }
}
