//! Windows ネイティブ UI（Win32 と Direct2D/DirectWrite）。
//!
//! いまは閲覧専用ビューア。見えている行だけを描画するので、ファイルサイズに
//! 関係なく描画コストは一定になる。

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

use hayate_core::{Document, Line};
use windows::core::{w, Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

const APP_NAME: &str = "hayate";
const FONT_CANDIDATES: [&str; 4] = ["BIZ UDGothic", "BIZ UDゴシック", "MS Gothic", "Consolas"];
const FONT_SIZE: f32 = 14.0;
/// 1 行のうち描画する最大 UTF-16 単位数（それ以上は「…」）
const MAX_RENDER_UNITS: usize = 8192;
const TIMER_INDEX: usize = 1;

const fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}
const COL_BG: D2D1_COLOR_F = rgb(0xFF, 0xFF, 0xFF);
const COL_TEXT: D2D1_COLOR_F = rgb(0x1E, 0x1E, 0x1E);
const COL_GUTTER_BG: D2D1_COLOR_F = rgb(0xF3, 0xF3, 0xF3);
const COL_GUTTER_TEXT: D2D1_COLOR_F = rgb(0x8A, 0x8A, 0x8A);
const COL_STATUS_BG: D2D1_COLOR_F = rgb(0x2B, 0x57, 0x9A);
const COL_STATUS_TEXT: D2D1_COLOR_F = rgb(0xFF, 0xFF, 0xFF);
const COL_MATCH: D2D1_COLOR_F = rgb(0xFF, 0xE0, 0x6B);
const COL_DIM: D2D1_COLOR_F = rgb(0xB0, 0xB0, 0xB0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// 入力欄（ステータスバー上の簡易プロンプト）
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prompt {
    Find,
    GotoLine,
}

/// 借用中に実行できない処理（モーダルダイアログはメッセージを再入させる）
enum Deferred {
    OpenDialog,
}

struct Gfx {
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
}

struct App {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    format: IDWriteTextFormat,
    gfx: Option<Gfx>,
    line_h: f32,
    char_w: f32,
    doc: Option<Document>,
    top: u64,
    x_off: f32,
    prompt: Option<Prompt>,
    input: String,
    last_find: String,
    /// 現在の検索一致（バイト範囲）
    hit: Option<(u64, u64)>,
    message: String,
    opened_at: Option<Instant>,
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Err((step, e)) = run_inner() {
            let msg = wide(&format!("起動に失敗しました（{step}）: {e}"));
            MessageBoxW(None, PCWSTR(msg.as_ptr()), w!("hayate"), MB_ICONERROR);
        }
    }
}

unsafe fn run_inner() -> std::result::Result<(), (&'static str, windows::core::Error)> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| ("GetModuleHandle", e))?;
        let class = w!("HayateMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_IBEAM).map_err(|e| ("LoadCursor", e))?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("hayate"),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_HSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| ("CreateWindow", e))?;

        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(|e| ("D2D1CreateFactory", e))?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| ("DWriteCreateFactory", e))?;
        let format = create_format(&dw).map_err(|e| ("CreateTextFormat", e))?;
        // フォントが見つからない環境でも起動できるよう、計測失敗時は既定値
        let (char_w, line_h) = measure(&dw, &format).unwrap_or((FONT_SIZE * 0.5, (FONT_SIZE * 1.4).ceil()));
        let _ = format.SetIncrementalTabStop(char_w * 4.0);

        let mut app = App {
            hwnd,
            d2d,
            dw,
            format,
            gfx: None,
            line_h,
            char_w,
            doc: None,
            top: 0,
            x_off: 0.0,
            prompt: None,
            input: String::new(),
            last_find: String::new(),
            hit: None,
            message: "Ctrl+O: 開く / ドラッグ&ドロップでも開けます".into(),
            opened_at: None,
        };
        if let Some(arg) = std::env::args_os().nth(1) {
            app.open(PathBuf::from(arg));
        }
        APP.with(|a| *a.borrow_mut() = Some(app));
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| a.borrow_mut().take());
        Ok(())
    }
}

unsafe fn create_format(dw: &IDWriteFactory) -> Result<IDWriteTextFormat> {
    unsafe {
        let mut fonts = None;
        dw.GetSystemFontCollection(&mut fonts, false)?;
        let fonts = fonts.unwrap();
        // 環境変数 HAYATE_FONT でフォントを指定できる
        let user_font = std::env::var("HAYATE_FONT").ok();
        let candidates = user_font.iter().map(String::as_str).chain(FONT_CANDIDATES);
        let mut family = FONT_CANDIDATES[FONT_CANDIDATES.len() - 1];
        for name in candidates {
            let (mut idx, mut exists) = (0u32, BOOL(0));
            let wname = wide(name);
            fonts.FindFamilyName(PCWSTR(wname.as_ptr()), &mut idx, &mut exists)?;
            if exists.as_bool() {
                family = name;
                break;
            }
        }
        let wfamily = wide(family);
        let format = dw.CreateTextFormat(
            PCWSTR(wfamily.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            FONT_SIZE,
            w!("ja-jp"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// 半角 1 文字の幅と行の高さ（DIP）
unsafe fn measure(dw: &IDWriteFactory, format: &IDWriteTextFormat) -> Result<(f32, f32)> {
    unsafe {
        let s: Vec<u16> = "0000000000".encode_utf16().collect();
        let layout = dw.CreateTextLayout(&s, format, 10000.0, 1000.0)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut m)?;
        Ok((m.width / 10.0, (m.height * 1.15).ceil()))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let result = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return None };
        let app = guard.as_mut()?;
        Some(unsafe { app.handle(msg, wparam, lparam) })
    });
    match result {
        Some((Some(r), deferred)) => {
            if let Some(d) = deferred {
                unsafe { run_deferred(hwnd, d) };
            }
            r
        }
        Some((None, deferred)) => {
            if let Some(d) = deferred {
                unsafe { run_deferred(hwnd, d) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        None => {
            if msg == WM_DESTROY {
                unsafe { PostQuitMessage(0) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

unsafe fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::OpenDialog => {
            if let Some(path) = unsafe { open_dialog(hwnd) } {
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        app.open(path);
                    }
                });
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

unsafe fn open_dialog(hwnd: HWND) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let filter = wide("テキストファイル\0*.txt;*.log;*.csv;*.tsv\0すべてのファイル\0*.*\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 2,
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    } else {
        None
    }
}

impl App {
    /// 戻り値: (Some(LRESULT) なら処理済み, 借用を外してから行う処理)
    unsafe fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> (Option<LRESULT>, Option<Deferred>) {
        unsafe {
            match msg {
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(self.hwnd, &mut ps);
                    if let Err(e) = self.paint() {
                        if e.code() == D2DERR_RECREATE_TARGET {
                            self.gfx = None;
                            self.invalidate();
                        }
                    }
                    let _ = EndPaint(self.hwnd, &ps);
                    (Some(LRESULT(0)), None)
                }
                WM_ERASEBKGND => (Some(LRESULT(1)), None),
                WM_SIZE => {
                    if let Some(g) = &self.gfx {
                        let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
                        let _ = g.target.Resize(&D2D_SIZE_U { width: w, height: h });
                    }
                    self.update_scrollbars();
                    self.invalidate();
                    (Some(LRESULT(0)), None)
                }
                WM_DPICHANGED => {
                    self.gfx = None;
                    let r = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                    (Some(LRESULT(0)), None)
                }
                WM_TIMER if wparam.0 == TIMER_INDEX => {
                    let done = self.doc.as_ref().is_none_or(|d| d.progress().done);
                    if done {
                        let _ = KillTimer(Some(self.hwnd), TIMER_INDEX);
                        if let (Some(t), Some(d)) = (self.opened_at, &self.doc) {
                            self.message = format!("索引作成完了 {:.2} 秒（{} 行）", t.elapsed().as_secs_f64(), group(d.line_count().0));
                        }
                    }
                    self.update_scrollbars();
                    self.invalidate();
                    (Some(LRESULT(0)), None)
                }
                WM_VSCROLL => {
                    let code = SCROLLBAR_COMMAND(loword(wparam.0) as i32);
                    let page = self.visible_lines() as i64;
                    match code {
                        SB_LINEUP => self.scroll_by(-1),
                        SB_LINEDOWN => self.scroll_by(1),
                        SB_PAGEUP => self.scroll_by(-page),
                        SB_PAGEDOWN => self.scroll_by(page),
                        SB_TOP => self.scroll_to(0),
                        SB_BOTTOM => self.scroll_to(u64::MAX),
                        SB_THUMBTRACK | SB_THUMBPOSITION => {
                            let mut si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..Default::default() };
                            let _ = GetScrollInfo(self.hwnd, SB_VERT, &mut si);
                            let scale = self.scroll_scale();
                            self.scroll_to(si.nTrackPos as u64 * scale);
                        }
                        _ => {}
                    }
                    (Some(LRESULT(0)), None)
                }
                WM_HSCROLL => {
                    let code = SCROLLBAR_COMMAND(loword(wparam.0) as i32);
                    match code {
                        SB_LINELEFT => self.hscroll_by(-self.char_w * 4.0),
                        SB_LINERIGHT => self.hscroll_by(self.char_w * 4.0),
                        SB_PAGELEFT => self.hscroll_by(-self.char_w * 40.0),
                        SB_PAGERIGHT => self.hscroll_by(self.char_w * 40.0),
                        SB_THUMBTRACK | SB_THUMBPOSITION => {
                            let mut si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..Default::default() };
                            let _ = GetScrollInfo(self.hwnd, SB_HORZ, &mut si);
                            self.x_off = si.nTrackPos as f32;
                            self.update_scrollbars();
                            self.invalidate();
                        }
                        _ => {}
                    }
                    (Some(LRESULT(0)), None)
                }
                WM_MOUSEWHEEL => {
                    let delta = hiword(wparam.0) as i16 as i64;
                    if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                        self.hscroll_by(-(delta as f32) / 120.0 * self.char_w * 8.0);
                    } else {
                        self.scroll_by(-delta * 3 / 120);
                    }
                    (Some(LRESULT(0)), None)
                }
                WM_MOUSEHWHEEL => {
                    let delta = hiword(wparam.0) as i16 as f32;
                    self.hscroll_by(delta / 120.0 * self.char_w * 8.0);
                    (Some(LRESULT(0)), None)
                }
                WM_KEYDOWN => (Some(LRESULT(0)), self.on_key(VIRTUAL_KEY(wparam.0 as u16))),
                WM_CHAR => {
                    self.on_char(wparam.0 as u32);
                    (Some(LRESULT(0)), None)
                }
                WM_DROPFILES => {
                    let drop = HDROP(wparam.0 as _);
                    let mut buf = vec![0u16; 32768];
                    let n = DragQueryFileW(drop, 0, Some(&mut buf)) as usize;
                    DragFinish(drop);
                    if n > 0 {
                        self.open(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
                    }
                    (Some(LRESULT(0)), None)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    (Some(LRESULT(0)), None)
                }
                _ => (None, None),
            }
        }
    }

    fn open(&mut self, path: PathBuf) {
        match Document::open(&path) {
            Ok(doc) => {
                self.doc = Some(doc);
                self.top = 0;
                self.x_off = 0.0;
                self.hit = None;
                self.opened_at = Some(Instant::now());
                self.message.clear();
                let title = wide(&format!("{} - {APP_NAME}", path.display()));
                unsafe {
                    let _ = SetWindowTextW(self.hwnd, PCWSTR(title.as_ptr()));
                    SetTimer(Some(self.hwnd), TIMER_INDEX, 100, None);
                }
            }
            Err(e) => self.message = format!("開けませんでした: {} ({e})", path.display()),
        }
        self.update_scrollbars();
        self.invalidate();
    }

    unsafe fn on_key(&mut self, vk: VIRTUAL_KEY) -> Option<Deferred> {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if self.prompt.is_some() {
            match vk {
                VK_ESCAPE => {
                    self.prompt = None;
                    self.invalidate();
                }
                VK_RETURN => self.submit_prompt(),
                VK_BACK => {
                    self.input.pop();
                    self.invalidate();
                }
                _ => {}
            }
            return None;
        }
        let page = self.visible_lines() as i64;
        match vk {
            VK_UP => self.scroll_by(-1),
            VK_DOWN => self.scroll_by(1),
            VK_PRIOR => self.scroll_by(-page),
            VK_NEXT => self.scroll_by(page),
            VK_HOME if ctrl => self.scroll_to(0),
            VK_END if ctrl => self.scroll_to(u64::MAX),
            VK_HOME => self.hscroll_by(-f32::MAX),
            VK_LEFT => self.hscroll_by(-self.char_w * 4.0),
            VK_RIGHT => self.hscroll_by(self.char_w * 4.0),
            VK_F3 => self.find_next(!shift),
            _ if ctrl && vk == VK_O => return Some(Deferred::OpenDialog),
            _ if ctrl && vk == VK_F => self.start_prompt(Prompt::Find),
            _ if ctrl && vk == VK_G => self.start_prompt(Prompt::GotoLine),
            _ => {}
        }
        None
    }

    fn on_char(&mut self, ch: u32) {
        if self.prompt.is_none() {
            return;
        }
        // 制御文字（Enter, BS, Esc, Ctrl+?）は WM_KEYDOWN 側で処理済み
        if ch < 0x20 || ch == 0x7F {
            return;
        }
        if let Some(c) = char::from_u32(ch) {
            self.input.push(c);
            self.invalidate();
        }
    }

    fn start_prompt(&mut self, p: Prompt) {
        self.prompt = Some(p);
        self.input = if p == Prompt::Find { self.last_find.clone() } else { String::new() };
        self.invalidate();
    }

    fn submit_prompt(&mut self) {
        match self.prompt.take() {
            Some(Prompt::Find) => {
                self.last_find = std::mem::take(&mut self.input);
                self.hit = None;
                self.find_next(true);
            }
            Some(Prompt::GotoLine) => {
                let input = std::mem::take(&mut self.input);
                match input.trim().replace(',', "").parse::<u64>() {
                    Ok(n) if n >= 1 => {
                        let target = n - 1;
                        let known = self.doc.as_ref().map_or(0, |d| d.line_count().0);
                        if target >= known {
                            self.message = format!("{} 行目はまだ索引にありません", group(n));
                        }
                        self.scroll_to(target.saturating_sub(self.visible_lines() as u64 / 3));
                    }
                    _ => self.message = "行番号を入力してください".into(),
                }
            }
            None => {}
        }
        self.invalidate();
    }

    /// F3 / Shift+F3。いまは前方検索のみ（後方は今後対応）
    fn find_next(&mut self, forward: bool) {
        let Some(doc) = &self.doc else { return };
        if self.last_find.is_empty() {
            self.start_prompt(Prompt::Find);
            return;
        }
        if !forward {
            self.message = "後方検索は未対応です".into();
            self.invalidate();
            return;
        }
        let from = match self.hit {
            Some((s, _)) => s + 1,
            None => doc.line_range(self.top).map_or(0, |(s, _)| s),
        };
        let t = Instant::now();
        match doc.find(&self.last_find, from) {
            Some(hit) => {
                self.hit = Some(hit);
                match doc.line_of_offset(hit.0) {
                    Some(line) => {
                        self.message = format!("{} 行目で見つかりました（{:.0} ms）", group(line + 1), t.elapsed().as_secs_f64() * 1000.0);
                        let vis = self.visible_lines() as u64;
                        if line < self.top || line >= self.top + vis.saturating_sub(1) {
                            self.scroll_to(line.saturating_sub(vis / 3));
                        }
                    }
                    None => self.message = "見つかりましたが、索引作成中のため移動できません".into(),
                }
            }
            None => {
                self.message = format!("「{}」は見つかりませんでした（末尾まで）", self.last_find);
                self.hit = None;
            }
        }
        self.invalidate();
    }

    fn total_lines(&self) -> u64 {
        self.doc.as_ref().map_or(1, |d| d.line_count().0)
    }

    fn client_size(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            ((rc.right - rc.left) as f32 / scale, (rc.bottom - rc.top) as f32 / scale)
        }
    }

    fn status_h(&self) -> f32 {
        self.line_h + 6.0
    }

    fn visible_lines(&self) -> usize {
        let (_, h) = self.client_size();
        (((h - self.status_h()) / self.line_h).floor() as usize).max(1)
    }

    fn max_top(&self) -> u64 {
        self.total_lines().saturating_sub(self.visible_lines() as u64)
    }

    fn scroll_by(&mut self, delta: i64) {
        let t = (self.top as i64 + delta).max(0) as u64;
        self.scroll_to(t);
    }

    fn scroll_to(&mut self, line: u64) {
        self.top = line.min(self.max_top());
        self.update_scrollbars();
        self.invalidate();
    }

    fn hscroll_by(&mut self, dx: f32) {
        self.x_off = (self.x_off + dx).max(0.0);
        self.update_scrollbars();
        self.invalidate();
    }

    /// スクロールバーの値は i32 なので、行数が大きいときは縮尺をかける
    fn scroll_scale(&self) -> u64 {
        (self.total_lines() / (i32::MAX as u64 / 2)).max(1)
    }

    fn update_scrollbars(&self) {
        let scale = self.scroll_scale();
        let page = self.visible_lines() as u64;
        let total = self.total_lines();
        let v = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: (total.saturating_sub(1) / scale) as i32,
            nPage: (page / scale).max(1) as u32,
            nPos: (self.top / scale) as i32,
            nTrackPos: 0,
        };
        let (w, _) = self.client_size();
        let hmax = (self.char_w * 2000.0).max(self.x_off + w);
        let h = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: hmax as i32,
            nPage: w.max(1.0) as u32,
            nPos: self.x_off as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.hwnd, SB_VERT, &v, true);
            SetScrollInfo(self.hwnd, SB_HORZ, &h, true);
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn ensure_gfx(&mut self) -> Result<()> {
        if self.gfx.is_some() {
            return Ok(());
        }
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc)?;
            let dpi = GetDpiForWindow(self.hwnd) as f32;
            let props = D2D1_RENDER_TARGET_PROPERTIES { dpiX: dpi, dpiY: dpi, ..Default::default() };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            let brush = target.CreateSolidColorBrush(&COL_TEXT, None)?;
            self.gfx = Some(Gfx { target, brush });
        }
        Ok(())
    }

    unsafe fn paint(&mut self) -> Result<()> {
        unsafe {
            self.ensure_gfx()?;
            let (w, h) = self.client_size();
            let g = self.gfx.as_ref().unwrap();
            let rt: ID2D1RenderTarget = g.target.cast()?;
            let brush = &g.brush;
            rt.BeginDraw();
            rt.Clear(Some(&COL_BG));

            let text_h = h - self.status_h();
            let digits = group(self.total_lines()).len().max(4);
            let gutter = self.char_w * (digits as f32 + 2.0);

            let lines = match &self.doc {
                Some(doc) => doc.lines(self.top, self.visible_lines() + 1),
                None => Vec::new(),
            };

            // 行番号の欄
            brush.SetColor(&COL_GUTTER_BG);
            rt.FillRectangle(&rect(0.0, 0.0, gutter, text_h), brush);

            // 本文
            rt.PushAxisAlignedClip(&rect(gutter, 0.0, w, text_h), D2D1_ANTIALIAS_MODE_ALIASED);
            for (i, line) in lines.iter().enumerate() {
                let y = i as f32 * self.line_h;
                self.draw_line(&rt, brush, line, gutter + 4.0 - self.x_off, y)?;
            }
            rt.PopAxisAlignedClip();

            for (i, line) in lines.iter().enumerate() {
                let y = i as f32 * self.line_h;
                let num = format!("{:>width$}", group(line.number + 1), width = digits);
                brush.SetColor(&COL_GUTTER_TEXT);
                self.draw_str(&rt, brush, &num, self.char_w * 0.5, y, gutter)?;
            }
            if lines.is_empty() && self.doc.is_some() {
                brush.SetColor(&COL_DIM);
                self.draw_str(&rt, brush, "索引作成中…", gutter + 8.0, 0.0, w)?;
            }

            self.draw_status(&rt, brush, w, h)?;
            rt.EndDraw(None, None)
        }
    }

    unsafe fn draw_line(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, line: &Line, x: f32, y: f32) -> Result<()> {
        let mut units: Vec<u16> = line.text.encode_utf16().take(MAX_RENDER_UNITS).collect();
        let cut = line.truncated || units.len() == MAX_RENDER_UNITS;
        if cut {
            units.extend("  …(省略)".encode_utf16());
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(&units, &self.format, 1.0e6, self.line_h)?;
            // 検索一致のハイライト
            if let (Some((hs, he)), Some(doc)) = (self.hit, &self.doc) {
                if hs < line.end.max(line.start + 1) && he > line.start {
                    let (a, b) = self.hit_units(doc, line, hs, he);
                    let (a, b) = (a.min(units.len() as u32), b.min(units.len() as u32));
                    brush.SetColor(&COL_MATCH);
                    let mut m = [DWRITE_HIT_TEST_METRICS::default(); 4];
                    let mut n = 0u32;
                    if layout.HitTestTextRange(a, b - a, x, y, Some(&mut m), &mut n).is_ok() {
                        for r in &m[..n as usize] {
                            rt.FillRectangle(&rect(r.left, r.top, r.left + r.width, r.top + r.height), brush);
                        }
                    } else {
                        // HitTestTextRange が使えない環境（Wine など）向け: 前後の文字列幅から求める
                        let l = self.text_width(&units[..a as usize])?;
                        let r = self.text_width(&units[..b as usize])?;
                        rt.FillRectangle(&rect(x + l, y, x + r, y + self.line_h), brush);
                    }
                }
            }
            brush.SetColor(&COL_TEXT);
            rt.DrawTextLayout(Vector2 { X: x, Y: y }, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        Ok(())
    }

    unsafe fn text_width(&self, units: &[u16]) -> Result<f32> {
        unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.format, 1.0e6, self.line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(m.widthIncludingTrailingWhitespace)
        }
    }

    /// 一致範囲（バイト）を行内の UTF-16 位置に直す
    fn hit_units(&self, doc: &Document, line: &Line, hs: u64, he: u64) -> (u32, u32) {
        let s = hs.clamp(line.start, line.end);
        let e = he.clamp(s, line.end);
        let prefix = decode_units(doc, doc.slice(line.start, s));
        let body = decode_units(doc, doc.slice(s, e));
        (prefix as u32, (prefix + body) as u32)
    }

    unsafe fn draw_str(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, s: &str, x: f32, y: f32, right: f32) -> Result<()> {
        let units: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            rt.DrawText(
                &units,
                &self.format,
                &rect(x, y, right.max(x + 1.0), y + self.line_h),
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    unsafe fn draw_status(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, h: f32) -> Result<()> {
        let top = h - self.status_h();
        unsafe {
            brush.SetColor(&COL_STATUS_BG);
            rt.FillRectangle(&rect(0.0, top, w, h), brush);
            brush.SetColor(&COL_STATUS_TEXT);
        }
        let left = match self.prompt {
            Some(Prompt::Find) => format!("検索: {}▏  (Enter: 検索 / Esc: 取消)", self.input),
            Some(Prompt::GotoLine) => format!("行番号へ移動: {}▏", self.input),
            None => self.message.clone(),
        };
        let right = match &self.doc {
            None => String::new(),
            Some(doc) => {
                let p = doc.progress();
                let (lines, exact) = doc.line_count();
                let lines = if exact { group(lines) } else { format!("約{}", group(lines)) };
                let idx = if p.done {
                    String::new()
                } else {
                    format!("索引 {:.0}%  ", p.scanned_bytes as f64 * 100.0 / p.total_bytes.max(1) as f64)
                };
                format!(
                    "{idx}{} 行目 / {lines} 行   {}   {}   {}",
                    group(self.top + 1),
                    doc.encoding().name(),
                    doc.newline_name(),
                    human_size(doc.len())
                )
            }
        };
        unsafe {
            self.draw_str(rt, brush, &left, 8.0, top + 3.0, w * 0.55)?;
            let units: Vec<u16> = right.encode_utf16().collect();
            let layout = self.dw.CreateTextLayout(&units, &self.format, w * 0.45, self.line_h)?;
            layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            rt.DrawTextLayout(Vector2 { X: w * 0.55 - 8.0, Y: top + 3.0 }, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
        Ok(())
    }
}

/// バイト列をデコードしたときの UTF-16 単位数
fn decode_units(doc: &Document, bytes: &[u8]) -> usize {
    hayate_core::encoding::decode(doc.encoding(), bytes).encode_utf16().count()
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}

/// 3 桁区切り
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.2} {}", U[i]) }
}
