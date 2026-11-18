//! 画面の上下の部品のうち、標準の部品（Win32 のコモンコントロール）に替えたもの。
//!
//! いまは、ステータスバー（`msctls_statusbar32`）。本文より下の端に置き、左にメッセージ、右に行・桁・文字コードなどを並べる。
//! クリックできる部分（行・桁、文字コード、改行コード、タブ幅、挿入／上書き）は、部分（パート）ごとに分けてあり、クリックは `NM_CLICK` で受ける。
//! 色は、標準の部品がダークにならないので、各パートを自前で描く（`SBT_OWNERDRAW`）。

use windows::Win32::UI::Controls::*;

use super::find_dialog::rgb_ref;
use super::*;

/// ステータスバーの描画に使うもの（`update_status` が決める）。描画は App を借りずに行う
#[derive(Default)]
struct StatusDraw {
    left: String,
    /// 左のメッセージを描く右端（DIP）
    left_right: f32,
    /// 右の各部分：(文字, 左端, 右端)（DIP）
    items: Vec<(String, f32, f32)>,
    bg: COLORREF,
    fg: COLORREF,
    scale: f32,
}

thread_local! {
    static STATUS_DRAW: std::cell::RefCell<StatusDraw> = std::cell::RefCell::new(StatusDraw::default());
}

/// ステータスバーを描く（WM_DRAWITEM。パートは 1 つ）。`wndproc` の先頭から、App を借りる前に呼ぶ
pub(super) unsafe fn draw_status_item(di: &DRAWITEMSTRUCT) {
    STATUS_DRAW.with(|d| {
        let d = d.borrow();
        unsafe {
            let bg = CreateSolidBrush(d.bg);
            FillRect(di.hDC, &di.rcItem, bg);
            let _ = DeleteObject(bg.into());
            if d.scale <= 0.0 {
                return;
            }
            SetBkMode(di.hDC, TRANSPARENT);
            SetTextColor(di.hDC, d.fg);
            let line = |t: &str, x: f32, right: f32| {
                let mut u: Vec<u16> = t.encode_utf16().collect();
                let mut r = RECT { left: di.rcItem.left + (x * d.scale) as i32, top: di.rcItem.top, right: di.rcItem.left + (right * d.scale) as i32, bottom: di.rcItem.bottom };
                DrawTextW(di.hDC, &mut u, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            };
            for (t, x0, x1) in &d.items {
                line(t, *x0, *x1);
            }
            line(&d.left, 8.0, d.left_right);
        }
    });
}

/// ステータスバーの部品の ID
pub(super) const ID_STATUS: i32 = 9001;

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
            let height = -((self.settings.ui_font_size * dpi / 96.0).round() as i32);
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
        if rc.right <= 0 || rc.bottom <= 0 {
            return;
        }
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
        let segs: Vec<(String, Option<u8>)> = self.status_segments();
        let sig = format!("{left}\u{1}{}", segs.iter().map(|(s, k)| format!("{s}\u{2}{k:?}")).collect::<Vec<_>>().join("\u{1}"));
        if *self.status_sig.borrow() == sig {
            return;
        }
        *self.status_sig.borrow_mut() = sig;
        // 描画に使う位置を、ここで決めておく（描画は、App を借りずにできる：描く最中にステータスバーが同期で描き直されても、空白にならない）
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let w = self.client_w_dip();
        let width = |t: &str| unsafe { self.ui_text_width(&t.encode_utf16().collect::<Vec<_>>()) }.unwrap_or(0.0);
        let total: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        let rw = width(&total).min(w * 0.75);
        let mut x0 = w - rw - 8.0;
        let mut items = Vec::new();
        for (t, _) in &segs {
            let x1 = x0 + width(t);
            if !t.trim().is_empty() {
                items.push((t.clone(), x0, x1 + 4.0));
            }
            x0 = x1;
        }
        STATUS_DRAW.with(|d| {
            *d.borrow_mut() = StatusDraw { left: left.clone(), left_right: (w - rw - 24.0).max(8.0), items, bg: rgb_ref(&self.theme.status_bg), fg: rgb_ref(&self.theme.status_text), scale };
        });
        let mut texts = vec![left];
        for (s, _) in segs {
            texts.push(s);
        }
        unsafe {
            if self.status_font.get().is_invalid() {
                self.status_font.set(self.ui_hfont());
                SendMessageW(self.status, WM_SETFONT, Some(WPARAM(self.status_font.get().0 as usize)), Some(LPARAM(1)));
            }
            // パートは 1 つだけ（枠なし）。中身は全部、自前で描く（パートごとの枠は、標準の部品だと白い線で出てしまうため）
            let edge = [-1i32];
            SendMessageW(self.status, SB_SETPARTS, Some(WPARAM(1)), Some(LPARAM(edge.as_ptr() as isize)));
            SendMessageW(self.status, SB_SETTEXTW, Some(WPARAM(SBT_OWNERDRAW as usize | SBT_NOBORDERS as usize)), Some(LPARAM(0)));
            let bg = rgb_ref(&self.theme.status_bg);
            SendMessageW(self.status, SB_SETBKCOLOR, Some(WPARAM(0)), Some(LPARAM(bg.0 as isize)));
            let _ = InvalidateRect(Some(self.status), None, false);
        }
        *self.status_texts.borrow_mut() = texts;
    }

    /// ステータスバーをクリックした（NM_CLICK。x は DIP）。クリックで動く所なら、その操作
    pub(super) unsafe fn status_click(&mut self, x: f32) -> Option<Deferred> {
        let kind = self.status_hit(x)?;
        match kind {
            2 => unsafe { self.command(menu::ID_GOTO) },
            4 => unsafe { self.command(menu::ID_OVERWRITE) },
            k => Some(Deferred::StatusMenu(k)),
        }
    }
}
