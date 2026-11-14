//! タイトルバーの統合：標準のタイトルバー・メニューバーをやめて、いちばん上の行にタブとウィンドウのボタン（最小化・最大化・閉じる）を
//! 自前で描く。ウィンドウの移動・サイズ変更・最大化ボタンの上のスナップ表示は、WM_NCHITTEST の結果を返すことで、Windows にまかせる。
//! メニューは、ツールバー左端の ☰ ボタン（F10）から開く。

use super::*;

/// ウィンドウのボタン 1 つの幅（DIP）
pub(super) const CAP_BTN_W: f32 = 46.0;
/// 左の、アプリのアイコンの幅（DIP）。タブは、この右から並ぶ
pub(super) const TAB_X0: f32 = 48.0;
/// ウィンドウの縁（サイズ変更のつかみ代。DIP）
const EDGE: f32 = 6.0;

/// ウィンドウのボタンの種類（cap_hover / cap_down の値）
const BTN_MIN: u8 = 0;
const BTN_MAX: u8 = 1;
const BTN_CLOSE: u8 = 2;

impl App {
    /// ウィンドウのボタンの並びの幅
    pub(super) fn controls_w(&self) -> f32 {
        CAP_BTN_W * 3.0
    }

    /// 点（クライアント座標。DIP）の下のウィンドウのボタン
    fn cap_button_at(&self, x: f32, y: f32) -> Option<u8> {
        let w = self.client_w_dip();
        if y < 0.0 || y >= self.tab_h() || x < w - self.controls_w() {
            return None;
        }
        Some(((x - (w - self.controls_w())) / CAP_BTN_W) as u8).filter(|&b| b <= BTN_CLOSE)
    }

    pub(super) fn client_w_dip(&self) -> f32 {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right - rc.left) as f32 / (unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0)
    }

    /// WM_NCCALCSIZE：標準のタイトルバーの分を、クライアント領域に戻す（最大化のときは、はみ出す縁の分だけ内側へ）
    pub(super) unsafe fn nc_calc_size(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            let p = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let top = p.rgrc[0].top;
            let r = DefWindowProcW(self.hwnd, msg, wparam, lparam);
            if r.0 != 0 {
                return r;
            }
            p.rgrc[0].top = top;
            if IsZoomed(self.hwnd).as_bool() {
                let dpi = GetDpiForWindow(self.hwnd);
                p.rgrc[0].top += GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            }
            LRESULT(0)
        }
    }

    /// WM_NCHITTEST：ウィンドウのボタン・サイズ変更の縁・移動のつかみ（タブのない所）を答える
    pub(super) unsafe fn nc_hit_test(&self, lparam: LPARAM) -> LRESULT {
        unsafe {
            let pt = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let mut rc = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut rc);
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            let (x, y) = ((pt.x - rc.left) as f32 / scale, (pt.y - rc.top) as f32 / scale);
            let (w, h) = ((rc.right - rc.left) as f32 / scale, (rc.bottom - rc.top) as f32 / scale);
            // ボタンとタブの判定は、クライアント座標で（左の縁の分だけ、ウィンドウの座標とずれる）
            let mut cpt = pt;
            let _ = ScreenToClient(self.hwnd, &mut cpt);
            let (cx, cy) = (cpt.x as f32 / scale, cpt.y as f32 / scale);
            if let Some(b) = self.cap_button_at(cx, cy) {
                return LRESULT(match b {
                    BTN_MIN => HTMINBUTTON,
                    BTN_MAX => HTMAXBUTTON,
                    _ => HTCLOSE,
                } as isize);
            }
            if !IsZoomed(self.hwnd).as_bool() {
                let (l, r, t, b) = (x < EDGE, x >= w - EDGE, y < EDGE, y >= h - EDGE);
                let code = match (l, r, t, b) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, ..) => HTLEFT,
                    (_, true, ..) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => 0,
                };
                if code != 0 {
                    return LRESULT(code as isize);
                }
            }
            // いちばん上の行：タブの上は操作（クライアント）、それ以外はウィンドウの移動
            if cy < self.tab_h() {
                return LRESULT(if self.tab_zone_hit(cx) { HTCLIENT } else { HTCAPTION } as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
    }

    /// タブの上（クリック・ドラッグを受ける所）か
    fn tab_zone_hit(&self, x: f32) -> bool {
        self.tab_hover(x).is_some()
    }

    /// ウィンドウのボタンの上にマウスがある（WM_NCMOUSEMOVE）
    pub(super) fn cap_mouse_move(&mut self, hit: u32) {
        let new = match hit {
            HTMINBUTTON => Some(BTN_MIN),
            HTMAXBUTTON => Some(BTN_MAX),
            HTCLOSE => Some(BTN_CLOSE),
            _ => None,
        };
        if new != self.cap_hover {
            self.cap_hover = new;
            self.invalidate();
        }
        if new.is_some() {
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut t);
            }
        }
    }

    pub(super) fn cap_mouse_leave(&mut self) {
        if self.cap_hover.take().is_some() || self.cap_down.take().is_some() {
            self.invalidate();
        }
    }

    /// WM_NCLBUTTONDOWN：ボタンを押した（押したままの見た目にする）。ボタンなら true
    pub(super) fn cap_button_down(&mut self, hit: u32) -> bool {
        let b = match hit {
            HTMINBUTTON => BTN_MIN,
            HTMAXBUTTON => BTN_MAX,
            HTCLOSE => BTN_CLOSE,
            _ => return false,
        };
        self.cap_down = Some(b);
        self.invalidate();
        true
    }

    /// WM_NCLBUTTONUP：同じボタンの上で離したら、実行
    pub(super) fn cap_button_up(&mut self, hit: u32) -> bool {
        let b = match hit {
            HTMINBUTTON => BTN_MIN,
            HTMAXBUTTON => BTN_MAX,
            HTCLOSE => BTN_CLOSE,
            _ => return false,
        };
        let pressed = self.cap_down.take() == Some(b);
        self.invalidate();
        if pressed {
            let cmd = match b {
                BTN_MIN => SC_MINIMIZE,
                BTN_MAX => if unsafe { IsZoomed(self.hwnd) }.as_bool() { SC_RESTORE } else { SC_MAXIMIZE },
                _ => SC_CLOSE,
            };
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
            }
        }
        true
    }

    /// exe に埋め込んだアイコン（16 DIP の大きさ）を、描画用のビットマップにする
    pub(super) unsafe fn load_app_icon(&self, target: &ID2D1HwndRenderTarget) -> Option<ID2D1Bitmap> {
        unsafe {
            let size = (16.0 * GetDpiForWindow(self.hwnd) as f32 / 96.0).round() as i32;
            let module = GetModuleHandleW(None).ok()?;
            let icon = LoadImageW(Some(module.into()), PCWSTR(std::ptr::without_provenance::<u16>(1)), IMAGE_ICON, size, size, LR_DEFAULTCOLOR).ok()?;
            // 32 ビットの DIB に描いて、その画素を取り出す（透明な背景のまま）
            let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: size, biHeight: -size, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
            let hdc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(hdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0);
            let result = match dib {
                Ok(dib) if !bits.is_null() => {
                    let old = SelectObject(hdc, dib.into());
                    std::ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
                    let _ = DrawIconEx(hdc, 0, 0, HICON(icon.0), size, size, 0, None, DI_NORMAL);
                    let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                    let bmp = target.CreateBitmap(D2D_SIZE_U { width: size as u32, height: size as u32 }, Some(bits as *const _), (size * 4) as u32, &props).ok();
                    SelectObject(hdc, old);
                    let _ = DeleteObject(dib.into());
                    bmp
                }
                _ => None,
            };
            let _ = DeleteDC(hdc);
            let _ = DestroyIcon(HICON(icon.0));
            result
        }
    }

    /// いちばん上の行の左のアイコン、右のウィンドウのボタンを描く（タブバーの中）
    pub(super) unsafe fn draw_caption(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let (t, h) = (self.theme, self.tab_h());
        let accent = theme::accent(self.settings.dark);
        unsafe {
            // アプリのアイコン（読み込めなかったときは、アクセントカラーの四角）
            let dest = rect(16.0, (h - 16.0) / 2.0, 32.0, (h + 16.0) / 2.0);
            match self.gfx.as_ref().and_then(|g| g.icon.as_ref()) {
                Some(bmp) => rt.DrawBitmap(bmp, Some(&dest), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                None => {
                    brush.SetColor(&accent);
                    rt.FillRoundedRectangle(&rrect(16.0, (h - 16.0) / 2.0, 32.0, (h + 16.0) / 2.0, 4.0), brush);
                }
            }
            // ウィンドウのボタン
            let zoomed = IsZoomed(self.hwnd).as_bool();
            for b in [BTN_MIN, BTN_MAX, BTN_CLOSE] {
                let x0 = w - self.controls_w() + b as f32 * CAP_BTN_W;
                let hot = self.cap_hover == Some(b);
                let down = self.cap_down == Some(b);
                let on_red = b == BTN_CLOSE && (hot || down);
                if hot || down {
                    brush.SetColor(&if b == BTN_CLOSE { D2D1_COLOR_F { r: 0.77, g: 0.17, b: 0.11, a: if down { 0.85 } else { 1.0 } } } else { t.panel_head });
                    rt.FillRectangle(&rect(x0, 0.0, x0 + CAP_BTN_W, h), brush);
                }
                brush.SetColor(&if on_red { D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 } } else { t.text });
                let (cx, cy) = (x0 + CAP_BTN_W / 2.0, h / 2.0);
                let line = |a: (f32, f32), c: (f32, f32)| rt.DrawLine(Vector2 { X: cx + a.0, Y: cy + a.1 }, Vector2 { X: cx + c.0, Y: cy + c.1 }, brush, 1.0, None);
                match b {
                    BTN_MIN => line((-5.0, 0.5), (5.0, 0.5)),
                    BTN_MAX if zoomed => {
                        // 元に戻す：重なった 2 つの四角
                        rt.DrawRectangle(&rect(cx - 5.0, cy - 2.0, cx + 3.0, cy + 5.0), brush, 1.0, None);
                        line((-2.0, -2.0), (-2.0, -5.0));
                        line((-2.0, -5.0), (5.0, -5.0));
                        line((5.0, -5.0), (5.0, 2.0));
                        line((5.0, 2.0), (3.0, 2.0));
                    }
                    BTN_MAX => {
                        rt.DrawRectangle(&rect(cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0), brush, 1.0, None);
                    }
                    _ => {
                        line((-5.0, -5.0), (5.0, 5.0));
                        line((-5.0, 5.0), (5.0, -5.0));
                    }
                }
            }
        }
        Ok(())
    }
}
