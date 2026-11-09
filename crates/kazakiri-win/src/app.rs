//! Windows ネイティブ UI（Win32 と Direct2D/DirectWrite）。
//!
//! いまは閲覧専用ビューア。見えている行だけを描画するので、ファイルサイズに
//! 関係なく描画コストは一定になる。検索は別スレッドで行い、UI は止めない。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use encoding_rs::Encoding;
use kazakiri_core::{Direction, Document, EditKind, Editor, Line, Query, Refresh, SearchControl, SearchOptions, SearchOutcome};
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

use crate::menu::{self, Menus};
use crate::{clipboard, recent};

mod edit;
use edit::{Pending, SaveJob, WM_RUN_PENDING, WM_SAVE_DONE};

const APP_NAME: &str = "kazakiri";
const FONT_CANDIDATES: [&str; 4] = ["BIZ UDGothic", "BIZ UDゴシック", "MS Gothic", "Consolas"];
const FONT_SIZE: f32 = 14.0;
/// 1 行のうち描画する最大 UTF-16 単位数（それ以上は「…」）
const MAX_RENDER_UNITS: usize = 8192;
/// クリップボードへコピーできる最大バイト数
const COPY_LIMIT: u64 = 256 << 20;
const TIMER_INDEX: usize = 1;
const TIMER_SEARCH: usize = 2;
const TIMER_FOLLOW: usize = 3;
/// 検索スレッドからの完了通知
const WM_SEARCH_DONE: u32 = WM_APP + 1;

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
const COL_SELECT: D2D1_COLOR_F = rgb(0xAD, 0xD6, 0xFF);
const COL_DIM: D2D1_COLOR_F = rgb(0xB0, 0xB0, 0xB0);
const COL_CARET: D2D1_COLOR_F = rgb(0x00, 0x00, 0x00);

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
    /// 名前を付けて保存ダイアログを出して保存する
    SaveAs,
    /// 未保存の変更を確認してから行う
    Confirm(Pending),
    /// そのまま行う（ダイアログを伴うことがある）
    Perform(Pending),
}

struct Gfx {
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
}

/// 実行中の検索
struct SearchJob {
    id: u64,
    ctl: Arc<SearchControl>,
    started: Instant,
    dir: Direction,
    /// 進捗表示用：探す範囲の大きさ
    span: u64,
    /// 先頭（後方なら末尾）に戻って探し直している最中か
    wrapped: bool,
    /// 検索を始めた位置（折り返したときの上限の目安）
    origin: u64,
}

/// 描画済みの行レイアウト（スクロールで使い回す）
struct CachedLine {
    end: u64,
    layout: IDWriteTextLayout,
    units: Vec<u16>,
    shown: usize,
}

/// 検索スレッドから UI へ渡す結果
struct SearchDone {
    id: u64,
    outcome: SearchOutcome,
    elapsed: Duration,
}

struct App {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    format: IDWriteTextFormat,
    gfx: Option<Gfx>,
    menus: Menus,
    line_h: f32,
    char_w: f32,
    /// いま表示している Document（編集のたびに editor から取り直す）
    doc: Option<Arc<Document>>,
    editor: Option<Editor>,
    /// 上下移動で保つ x 座標
    goal_x: Option<f32>,
    /// サロゲートペアの前半
    high_surrogate: Option<u16>,
    saving: Option<SaveJob>,
    /// 保存が終わったら行う操作
    pending: Option<Pending>,
    top: u64,
    x_off: f32,
    prompt: Option<Prompt>,
    input: String,
    last_find: String,
    opts: SearchOptions,
    search: Option<SearchJob>,
    next_job: u64,
    /// 現在の検索一致（バイト範囲）
    hit: Option<(u64, u64)>,
    /// 選択（アンカー, 先端）のバイト位置
    sel: Option<(u64, u64)>,
    dragging: bool,
    message: String,
    opened_at: Option<Instant>,
    recent: Vec<PathBuf>,
    /// 末尾の追従（tail -f）
    follow: bool,
    /// 追記を読み込んだら末尾へスクロールする
    stick_bottom: bool,
    /// 行の開始位置 → レイアウト。直前に描いた行だけを持つ
    layout_cache: RefCell<HashMap<u64, CachedLine>>,
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Err((step, e)) = run_inner() {
            let msg = wide(&format!("起動に失敗しました（{step}）: {e}"));
            MessageBoxW(None, PCWSTR(msg.as_ptr()), w!("kazakiri"), MB_ICONERROR);
        }
    }
}

unsafe fn run_inner() -> std::result::Result<(), (&'static str, windows::core::Error)> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| ("GetModuleHandle", e))?;
        let class = w!("KazakiriMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_IBEAM).map_err(|e| ("LoadCursor", e))?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let menus = menu::build().map_err(|e| ("CreateMenu", e))?;
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("kazakiri"),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_HSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
            None,
            Some(menus.bar),
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
            menus,
            line_h,
            char_w,
            doc: None,
            editor: None,
            goal_x: None,
            high_surrogate: None,
            saving: None,
            pending: None,
            top: 0,
            x_off: 0.0,
            prompt: None,
            input: String::new(),
            last_find: String::new(),
            opts: SearchOptions { case_sensitive: false, regex: false },
            search: None,
            next_job: 1,
            hit: None,
            sel: None,
            dragging: false,
            message: "Ctrl+O: 開く / Ctrl+N: 新規 / ドラッグ&ドロップでも開けます".into(),
            opened_at: None,
            recent: recent::load(),
            follow: false,
            stick_bottom: false,
            layout_cache: RefCell::new(HashMap::new()),
        };
        match std::env::args_os().nth(1) {
            Some(arg) => app.open(PathBuf::from(arg), None),
            None => app.new_doc(),
        }
        APP.with(|a| *a.borrow_mut() = Some(app));
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().take() {
                app.cancel_search();
            }
        });
        Ok(())
    }
}

unsafe fn create_format(dw: &IDWriteFactory) -> Result<IDWriteTextFormat> {
    unsafe {
        let mut fonts = None;
        dw.GetSystemFontCollection(&mut fonts, false)?;
        let fonts = fonts.unwrap();
        // 環境変数 KAZAKIRI_FONT でフォントを指定できる
        let user_font = std::env::var("KAZAKIRI_FONT").ok();
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
        Some((handled, deferred)) => {
            if let Some(d) = deferred {
                unsafe { run_deferred(hwnd, d) };
            }
            handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
        }
        None => {
            if msg == WM_DESTROY {
                unsafe { PostQuitMessage(0) };
            }
            if msg == WM_SEARCH_DONE {
                // 受け取れないときも結果は解放する
                drop(unsafe { Box::from_raw(lparam.0 as *mut SearchDone) });
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

unsafe fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::SaveAs => {
            let current = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.to_path_buf()))).flatten();
            match unsafe { edit::save_dialog(hwnd, current) } {
                Some(path) => {
                    if let Some(Some(d)) = with_app(|app| app.save(Some(path))) {
                        unsafe { run_deferred(hwnd, d) };
                    }
                }
                // 名前を付けずにやめたら、保留していた操作も取りやめる
                None => {
                    with_app(|app| app.pending = None);
                }
            }
        }
        Deferred::Confirm(action) => {
            let name = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.display().to_string()))).flatten();
            let text = wide(&format!("{} への変更を保存しますか？", name.unwrap_or_else(|| "（無題）".into())));
            let r = unsafe { MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), w!("kazakiri"), MB_YESNOCANCEL | MB_ICONQUESTION) };
            if r == IDYES {
                let next = with_app(|app| {
                    app.pending = Some(action);
                    app.save(None)
                });
                if let Some(Some(d)) = next {
                    unsafe { run_deferred(hwnd, d) };
                }
            } else if r == IDNO {
                unsafe { run_deferred(hwnd, Deferred::Perform(action)) };
            }
        }
        Deferred::Perform(action) => match action {
            Pending::OpenDialog => {
                if let Some(path) = unsafe { open_dialog(hwnd) } {
                    with_app(|app| app.open(path, None));
                }
            }
            Pending::OpenPath(p) => {
                with_app(|app| app.open(p, None));
            }
            Pending::Reopen(enc) => {
                with_app(|app| {
                    app.reopen(enc);
                    let name = menu::ENCODINGS.iter().find(|(_, e)| *e == enc).map_or("", |(n, _)| n);
                    app.message = format!("{name} で開き直しました");
                });
            }
            Pending::New => {
                with_app(|app| app.new_doc());
            }
            Pending::Close => unsafe {
                let _ = DestroyWindow(hwnd);
            },
        },
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
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
        const DONE: Option<LRESULT> = Some(LRESULT(0));
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
                    (DONE, None)
                }
                WM_ERASEBKGND => (Some(LRESULT(1)), None),
                WM_SIZE => {
                    if let Some(g) = &self.gfx {
                        let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
                        let _ = g.target.Resize(&D2D_SIZE_U { width: w, height: h });
                    }
                    self.update_scrollbars();
                    self.invalidate();
                    (DONE, None)
                }
                WM_DPICHANGED => {
                    self.gfx = None;
                    let r = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                    (DONE, None)
                }
                WM_TIMER if wparam.0 == TIMER_INDEX => {
                    let done = self.doc.as_ref().is_none_or(|d| d.progress().done);
                    if done {
                        let _ = KillTimer(Some(self.hwnd), TIMER_INDEX);
                        if let (Some(t), Some(d)) = (self.opened_at.take(), &self.doc) {
                            // 開き直し・再読み込みのメッセージは残す
                            if self.search.is_none() && self.message.is_empty() {
                                self.message = format!("索引作成完了 {:.2} 秒（{} 行）", t.elapsed().as_secs_f64(), group(d.line_count().0));
                            }
                        }
                    }
                    if self.stick_bottom {
                        self.top = self.max_top();
                        if done {
                            self.stick_bottom = false;
                        }
                    }
                    self.update_scrollbars();
                    self.invalidate();
                    (DONE, None)
                }
                WM_TIMER if wparam.0 == TIMER_FOLLOW => {
                    if self.follow {
                        self.refresh(true);
                    } else {
                        let _ = KillTimer(Some(self.hwnd), TIMER_FOLLOW);
                    }
                    (DONE, None)
                }
                WM_TIMER if wparam.0 == TIMER_SEARCH => {
                    if self.search.is_none() && self.saving.is_none() {
                        let _ = KillTimer(Some(self.hwnd), TIMER_SEARCH);
                    }
                    self.invalidate();
                    (DONE, None)
                }
                WM_SEARCH_DONE => {
                    let done = Box::from_raw(lparam.0 as *mut SearchDone);
                    self.on_search_done(*done);
                    (DONE, None)
                }
                WM_SAVE_DONE => {
                    let done = Box::from_raw(lparam.0 as *mut edit::SaveDone);
                    self.on_save_done(*done);
                    (DONE, None)
                }
                WM_RUN_PENDING => (DONE, self.pending.take().map(Deferred::Perform)),
                WM_CLOSE => {
                    if self.saving.is_some() {
                        self.message = "保存が終わるまでお待ちください".into();
                        self.invalidate();
                        return (DONE, None);
                    }
                    (DONE, self.guarded(Pending::Close))
                }
                WM_IME_STARTCOMPOSITION => {
                    self.update_ime_position();
                    (None, None)
                }
                WM_IME_COMPOSITION if self.ime_result(lparam) => (DONE, None),
                WM_INITMENUPOPUP => {
                    let forced = self.doc.as_ref().map(|d| d.forced_encoding());
                    self.menus.refresh(self.opts.case_sensitive, self.opts.regex, self.follow, forced, &self.recent);
                    (DONE, None)
                }
                WM_COMMAND => (DONE, self.command(loword(wparam.0))),
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
                    (DONE, None)
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
                    (DONE, None)
                }
                WM_MOUSEWHEEL => {
                    let delta = hiword(wparam.0) as i16 as i64;
                    if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                        self.hscroll_by(-(delta as f32) / 120.0 * self.char_w * 8.0);
                    } else {
                        self.scroll_by(-delta * 3 / 120);
                    }
                    (DONE, None)
                }
                WM_MOUSEHWHEEL => {
                    let delta = hiword(wparam.0) as i16 as f32;
                    self.hscroll_by(delta / 120.0 * self.char_w * 8.0);
                    (DONE, None)
                }
                WM_LBUTTONDOWN => {
                    let (x, y) = self.mouse_pos(lparam);
                    self.on_mouse_down(x, y, GetKeyState(VK_SHIFT.0 as i32) < 0);
                    (DONE, None)
                }
                WM_LBUTTONDBLCLK => {
                    let (x, y) = self.mouse_pos(lparam);
                    self.select_word_at(x, y);
                    (DONE, None)
                }
                WM_MOUSEMOVE if self.dragging => {
                    let (x, y) = self.mouse_pos(lparam);
                    self.on_drag(x, y);
                    (DONE, None)
                }
                WM_LBUTTONUP if self.dragging => {
                    self.dragging = false;
                    let _ = ReleaseCapture();
                    (DONE, None)
                }
                WM_KEYDOWN => (DONE, self.on_key(VIRTUAL_KEY(wparam.0 as u16))),
                // Alt+C / Alt+R：検索オプションの切り替え。
                // DefWindowProc にも渡す（渡さないと Alt を離したときにメニューが開いてしまう）
                WM_SYSKEYDOWN if matches!(VIRTUAL_KEY(wparam.0 as u16), VK_C | VK_R) => {
                    let id = if VIRTUAL_KEY(wparam.0 as u16) == VK_C { menu::ID_CASE } else { menu::ID_REGEX };
                    (None, self.command(id))
                }
                WM_SYSCHAR if matches!(char::from_u32(wparam.0 as u32), Some('c' | 'C' | 'r' | 'R')) => (DONE, None),
                WM_CHAR => {
                    self.on_char(wparam.0 as u32);
                    (DONE, None)
                }
                WM_DROPFILES => {
                    let drop = HDROP(wparam.0 as _);
                    let mut buf = vec![0u16; 32768];
                    let n = DragQueryFileW(drop, 0, Some(&mut buf)) as usize;
                    DragFinish(drop);
                    if n > 0 {
                        return (DONE, self.guarded(Pending::OpenPath(PathBuf::from(String::from_utf16_lossy(&buf[..n])))));
                    }
                    (DONE, None)
                }
                WM_DESTROY => {
                    self.cancel_search();
                    PostQuitMessage(0);
                    (DONE, None)
                }
                _ => (None, None),
            }
        }
    }

    // ---- ファイル ----

    fn open(&mut self, path: PathBuf, forced: Option<&'static Encoding>) {
        self.open_keep(path, forced, false);
    }

    /// `keep_view` なら表示位置と選択を保つ（再読み込み・開き直し）
    fn open_keep(&mut self, path: PathBuf, forced: Option<&'static Encoding>, keep_view: bool) {
        self.cancel_search();
        self.layout_cache.borrow_mut().clear();
        match Document::open_with(&path, forced) {
            Ok(doc) => {
                let doc = Arc::new(doc);
                self.editor = Some(Editor::new(doc.clone()));
                self.doc = Some(doc);
                self.goal_x = None;
                if !keep_view {
                    self.top = 0;
                    self.x_off = 0.0;
                    self.sel = None;
                }
                if self.sel.is_none() {
                    let start = self.doc.as_ref().map_or(0, |d| d.text_start() as u64);
                    self.sel = Some((start, start));
                }
                self.hit = None;
                self.opened_at = Some(Instant::now());
                self.message.clear();
                recent::push(&mut self.recent, &path);
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

    /// 文字コードを変えて開き直す
    fn reopen(&mut self, forced: Option<&'static Encoding>) {
        let Some(path) = self.doc.as_ref().and_then(|d| d.path().map(|p| p.to_path_buf())) else { return };
        self.open_keep(path, forced, true);
    }

    /// ディスク上の変更を読み込む。追記だけなら増えた分だけ索引を作る。
    /// `quiet` なら変化がないときに何も表示しない（追従モード用）
    fn refresh(&mut self, quiet: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let was_bottom = self.top >= self.max_top();
        match doc.refresh() {
            Ok(Refresh::Unchanged) => {
                if !quiet {
                    self.message = "変更はありません".into();
                    self.invalidate();
                }
            }
            Ok(Refresh::Appended(new)) => {
                let added = new.len() - doc.len();
                self.cancel_search();
                self.layout_cache.borrow_mut().clear();
                let new = Arc::new(new);
                if let Some(ed) = &mut self.editor {
                    ed.reset(new.clone());
                }
                self.doc = Some(new);
                self.message = format!("追記 +{} を読み込みました", human_size(added));
                if was_bottom && (self.follow || !quiet) {
                    self.stick_bottom = true;
                }
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_INDEX, 50, None);
                }
                self.update_scrollbars();
                self.invalidate();
            }
            Ok(Refresh::Replaced) => {
                let path = doc.path().map(|p| p.to_path_buf());
                if let Some(path) = path {
                    self.open_keep(path, doc.forced_encoding(), true);
                    self.message = "ファイルが置き換わったため開き直しました".into();
                    if self.follow {
                        self.stick_bottom = true;
                    }
                }
            }
            Err(e) => {
                if !quiet {
                    self.message = format!("再読み込みに失敗しました: {e}");
                    self.invalidate();
                }
            }
        }
    }

    fn set_follow(&mut self, on: bool) {
        self.follow = on;
        unsafe {
            if on {
                SetTimer(Some(self.hwnd), TIMER_FOLLOW, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_FOLLOW);
            }
        }
        if on {
            self.refresh(true);
            self.scroll_to(u64::MAX);
            self.stick_bottom = true;
        }
        self.message = format!("末尾の追従: {}", on_off(on));
        self.invalidate();
    }

    unsafe fn command(&mut self, id: u32) -> Option<Deferred> {
        match id {
            menu::ID_NEW => return self.guarded(Pending::New),
            menu::ID_OPEN => return self.guarded(Pending::OpenDialog),
            menu::ID_SAVE => return self.save(None),
            menu::ID_SAVE_AS => return Some(Deferred::SaveAs),
            menu::ID_RELOAD => {
                if self.is_modified() {
                    self.message = "編集中は再読み込みできません（保存するか、元に戻してください）".into();
                    self.invalidate();
                } else {
                    self.refresh(false)
                }
            }
            menu::ID_FOLLOW => {
                if self.is_modified() && !self.follow {
                    self.message = "編集中は追従できません".into();
                    self.invalidate();
                } else {
                    self.set_follow(!self.follow)
                }
            }
            menu::ID_EXIT => return self.guarded(Pending::Close),
            menu::ID_UNDO => self.undo(false),
            menu::ID_REDO => self.undo(true),
            menu::ID_CUT => self.cut(),
            menu::ID_PASTE => self.paste(),
            menu::ID_COPY => self.copy(),
            menu::ID_SELECT_ALL => {
                if let Some(doc) = &self.doc {
                    self.sel = Some((doc.text_start() as u64, doc.len()));
                    self.invalidate();
                }
            }
            menu::ID_FIND => self.start_prompt(Prompt::Find),
            menu::ID_FIND_NEXT => self.find(Direction::Forward),
            menu::ID_FIND_PREV => self.find(Direction::Backward),
            menu::ID_CASE => {
                self.opts.case_sensitive = !self.opts.case_sensitive;
                self.message = format!("大文字と小文字を区別: {}", on_off(self.opts.case_sensitive));
                self.invalidate();
            }
            menu::ID_REGEX => {
                self.opts.regex = !self.opts.regex;
                self.message = format!("正規表現: {}", on_off(self.opts.regex));
                self.invalidate();
            }
            menu::ID_GOTO => self.start_prompt(Prompt::GotoLine),
            id if (menu::ID_ENC_BASE..menu::ID_ENC_BASE + menu::ENCODINGS.len() as u32).contains(&id) => {
                let (_, enc) = menu::ENCODINGS[(id - menu::ID_ENC_BASE) as usize];
                return self.guarded(Pending::Reopen(enc));
            }
            id if (menu::ID_RECENT_BASE..menu::ID_RECENT_BASE + menu::RECENT_MAX as u32).contains(&id) => {
                if let Some(p) = self.recent.get((id - menu::ID_RECENT_BASE) as usize).cloned() {
                    return self.guarded(Pending::OpenPath(p));
                }
            }
            _ => {}
        }
        None
    }

    // ---- キーボード ----

    unsafe fn on_key(&mut self, vk: VIRTUAL_KEY) -> Option<Deferred> {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if self.prompt.is_some() {
            match vk {
                VK_ESCAPE => {
                    self.prompt = None;
                    self.invalidate();
                }
                VK_RETURN => self.submit_prompt(shift),
                VK_BACK => {
                    self.input.pop();
                    self.invalidate();
                }
                _ if ctrl && vk == VK_V => {
                    if let Some(s) = clipboard_text() {
                        self.input.push_str(s.lines().next().unwrap_or(""));
                        self.invalidate();
                    }
                }
                _ => {}
            }
            return None;
        }
        match vk {
            VK_ESCAPE => {
                if self.search.is_some() {
                    self.cancel_search();
                    self.message = "検索を中止しました".into();
                } else if let Some(job) = &self.saving {
                    job.cancel.store(true, Ordering::Relaxed);
                } else {
                    let c = self.caret();
                    self.sel = Some((c, c));
                    self.hit = None;
                }
                self.invalidate();
            }
            // カーソル移動（Shift で選択、Ctrl+↑↓ は画面だけスクロール）
            VK_UP if ctrl => self.scroll_by(-1),
            VK_DOWN if ctrl => self.scroll_by(1),
            VK_UP => self.move_vertical(-1, shift),
            VK_DOWN => self.move_vertical(1, shift),
            VK_LEFT if ctrl => self.move_word(false, shift),
            VK_RIGHT if ctrl => self.move_word(true, shift),
            VK_LEFT => self.move_horizontal(false, shift),
            VK_RIGHT => self.move_horizontal(true, shift),
            VK_PRIOR => self.move_page(false, shift),
            VK_NEXT => self.move_page(true, shift),
            VK_HOME if ctrl => self.move_doc_edge(false, shift),
            VK_END if ctrl => self.move_doc_edge(true, shift),
            VK_HOME => self.move_line_edge(false, shift),
            VK_END => self.move_line_edge(true, shift),
            // 編集
            VK_RETURN => self.insert_newline(),
            VK_BACK => self.delete_char(false),
            VK_DELETE if shift => self.cut(),
            VK_DELETE => self.delete_char(true),
            VK_TAB if !ctrl => self.type_text("\t", EditKind::Typing),
            VK_INSERT if ctrl => self.copy(),
            VK_INSERT if shift => self.paste(),
            VK_F3 if shift => return unsafe { self.command(menu::ID_FIND_PREV) },
            VK_F3 => return unsafe { self.command(menu::ID_FIND_NEXT) },
            VK_F5 => return unsafe { self.command(menu::ID_RELOAD) },
            _ if ctrl && shift && vk == VK_S => return Some(Deferred::SaveAs),
            _ if ctrl && shift && vk == VK_Z => self.undo(true),
            _ if ctrl && vk == VK_N => return unsafe { self.command(menu::ID_NEW) },
            _ if ctrl && vk == VK_O => return unsafe { self.command(menu::ID_OPEN) },
            _ if ctrl && vk == VK_S => return unsafe { self.command(menu::ID_SAVE) },
            _ if ctrl && vk == VK_Z => self.undo(false),
            _ if ctrl && vk == VK_Y => self.undo(true),
            _ if ctrl && vk == VK_X => self.cut(),
            _ if ctrl && vk == VK_V => self.paste(),
            _ if ctrl && vk == VK_F => self.start_prompt(Prompt::Find),
            _ if ctrl && vk == VK_G => self.start_prompt(Prompt::GotoLine),
            _ if ctrl && vk == VK_C => self.copy(),
            _ if ctrl && vk == VK_A => return unsafe { self.command(menu::ID_SELECT_ALL) },
            _ if ctrl && vk == VK_T => return unsafe { self.command(menu::ID_FOLLOW) },
            _ => {}
        }
        None
    }

    fn on_char(&mut self, ch: u32) {
        // 制御文字（Enter, BS, Tab, Esc, Ctrl+?）は WM_KEYDOWN 側で処理済み
        if ch < 0x20 || ch == 0x7F {
            return;
        }
        // 絵文字などはサロゲートペアで 2 回に分けて届く
        let c = match ch {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(ch as u16);
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(hi) = self.high_surrogate.take() else { return };
                char::decode_utf16([hi, ch as u16]).next().and_then(|r| r.ok())
            }
            _ => char::from_u32(ch),
        };
        let Some(c) = c else { return };
        if self.prompt.is_some() {
            self.input.push(c);
            self.invalidate();
        } else {
            self.type_text(c.encode_utf8(&mut [0; 4]), EditKind::Typing);
        }
    }

    fn start_prompt(&mut self, p: Prompt) {
        self.prompt = Some(p);
        self.input = match p {
            // 選択中の短い文字列があれば検索語の初期値にする
            Prompt::Find => self.selected_text(1024).filter(|s| !s.contains('\n')).unwrap_or_else(|| self.last_find.clone()),
            Prompt::GotoLine => String::new(),
        };
        self.invalidate();
    }

    fn submit_prompt(&mut self, shift: bool) {
        match self.prompt.take() {
            Some(Prompt::Find) => {
                self.last_find = std::mem::take(&mut self.input);
                self.hit = None;
                self.find(if shift { Direction::Backward } else { Direction::Forward });
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
                        if let Some((s, e)) = self.doc.as_ref().and_then(|d| d.line_range(target)) {
                            self.sel = Some((s, e));
                        }
                    }
                    _ => self.message = "行番号を入力してください".into(),
                }
            }
            None => {}
        }
        self.invalidate();
    }

    // ---- 検索 ----

    fn find(&mut self, dir: Direction) {
        let Some(doc) = self.doc.clone() else { return };
        if self.last_find.is_empty() {
            self.start_prompt(Prompt::Find);
            return;
        }
        let query = match Query::new(&doc, &self.last_find, self.opts) {
            Ok(q) => q,
            Err(e) => {
                self.message = e;
                self.invalidate();
                return;
            }
        };
        // 起点：直前の一致 → 選択 → 画面先頭
        let anchor = self.hit.or(self.sel.map(|(a, b)| (a.min(b), a.max(b))));
        let view_start = doc.line_range(self.top).map_or(0, |(s, _)| s);
        let from = match (dir, anchor) {
            (Direction::Forward, Some((s, e))) => if e > s { e } else { s + 1 },
            (Direction::Backward, Some((s, _))) => s,
            (_, None) => view_start,
        };
        self.start_search(doc, query, dir, from, false, from);
    }

    fn start_search(&mut self, doc: Arc<Document>, query: Query, dir: Direction, from: u64, wrapped: bool, origin: u64) {
        let span = match (dir, wrapped) {
            (Direction::Forward, false) => doc.len().saturating_sub(from),
            (Direction::Backward, false) => from,
            (Direction::Forward, true) => origin,
            (Direction::Backward, true) => doc.len().saturating_sub(origin),
        };

        self.cancel_search();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        self.search = Some(SearchJob { id, ctl: ctl.clone(), started: Instant::now(), dir, span, wrapped, origin });
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let t = Instant::now();
            let outcome = query.find(&doc, from, dir, &ctl);
            let done = Box::new(SearchDone { id, outcome, elapsed: t.elapsed() });
            let ptr = Box::into_raw(done);
            let hwnd = HWND(hwnd as *mut _);
            if unsafe { PostMessageW(Some(hwnd), WM_SEARCH_DONE, WPARAM(0), LPARAM(ptr as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(ptr) });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    fn cancel_search(&self) {
        if let Some(job) = &self.search {
            job.ctl.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn on_search_done(&mut self, done: SearchDone) {
        if self.search.as_ref().map(|j| j.id) != Some(done.id) {
            return; // 古い検索の結果
        }
        let job = self.search.take().unwrap();
        let ms = done.elapsed.as_secs_f64() * 1000.0;
        let Some(doc) = self.doc.clone() else { return };
        // 折り返し後に、検索を始めた位置より先で見つかったものは「見つからない」扱い
        let outcome = match done.outcome {
            SearchOutcome::Found(s, _) if job.wrapped && ((job.dir == Direction::Forward && s >= job.origin) || (job.dir == Direction::Backward && s < job.origin)) => {
                SearchOutcome::NotFound
            }
            o => o,
        };
        match outcome {
            SearchOutcome::Found(s, e) => {
                self.hit = Some((s, e));
                self.sel = Some((s, e));
                let wrap_note = match (job.wrapped, job.dir) {
                    (true, Direction::Forward) => "末尾まで達したので先頭から探しました。",
                    (true, Direction::Backward) => "先頭まで達したので末尾から探しました。",
                    _ => "",
                };
                match doc.line_of_offset(s) {
                    Some(line) => {
                        self.message = format!("{wrap_note}{} 行目で見つかりました（{ms:.0} ms）", group(line + 1));
                        let vis = self.visible_lines() as u64;
                        if line < self.top || line >= self.top + vis.saturating_sub(1) {
                            self.scroll_to(line.saturating_sub(vis / 3));
                        }
                        self.reveal_column(&doc, line, s);
                    }
                    None => self.message = "見つかりましたが、索引作成中のためまだ移動できません".into(),
                }
            }
            // まだ探していない側（前方なら先頭〜開始位置、後方なら開始位置〜末尾）があれば折り返す
            SearchOutcome::NotFound
                if !job.wrapped
                    && match job.dir {
                        Direction::Forward => job.origin > doc.text_start() as u64,
                        Direction::Backward => job.origin < doc.len(),
                    } =>
            {
                // 反対の端から、始めた位置まで探し直す
                if let Ok(query) = Query::new(&doc, &self.last_find, self.opts) {
                    let from = if job.dir == Direction::Forward { doc.text_start() as u64 } else { doc.len() };
                    self.start_search(doc, query, job.dir, from, true, job.origin);
                    return;
                }
            }
            SearchOutcome::NotFound => {
                self.message = format!("「{}」は見つかりませんでした（{ms:.0} ms）", self.last_find);
            }
            SearchOutcome::Cancelled => self.message = "検索を中止しました".into(),
        }
        self.invalidate();
    }

    /// 一致が横にはみ出していたら横スクロールする
    fn reveal_column(&mut self, doc: &Document, line: u64, offset: u64) {
        let Some(l) = doc.lines(line, 1).into_iter().next() else { return };
        let prefix = decode_units(doc, &doc.read(l.start, offset.min(l.end)));
        let x = prefix as f32 * self.char_w; // おおよその位置で十分
        let (w, _) = self.client_size();
        let view = w - self.gutter_width() - 8.0;
        if x < self.x_off || x > self.x_off + view - self.char_w * 8.0 {
            self.x_off = (x - view / 3.0).max(0.0);
            self.update_scrollbars();
        }
    }

    // ---- マウス・選択 ----

    fn mouse_pos(&self, lparam: LPARAM) -> (f32, f32) {
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let x = loword(lparam.0 as usize) as i16 as f32 / scale;
        let y = hiword(lparam.0 as usize) as i16 as f32 / scale;
        (x, y)
    }

    fn on_mouse_down(&mut self, x: f32, y: f32, shift: bool) {
        let Some(doc) = self.doc.clone() else { return };
        if y >= self.text_height() {
            return;
        }
        // 行番号欄のクリックは行全体を選択
        if x < self.gutter_width() {
            let line = self.top + (y / self.line_h) as u64;
            if let Some((s, _)) = doc.line_range(line) {
                let next = doc.line_range(line + 1).map_or(doc.len(), |(n, _)| n);
                self.sel = Some((s, next));
                self.invalidate();
            }
            return;
        }
        let Some(off) = self.offset_at(&doc, x, y) else { return };
        self.sel = match (shift, self.sel) {
            (true, Some((a, _))) => Some((a, off)),
            _ => Some((off, off)),
        };
        self.dragging = true;
        self.goal_x = None;
        if let Some(e) = &mut self.editor {
            e.break_group();
        }
        unsafe {
            SetCapture(self.hwnd);
        }
        self.update_ime_position();
        self.invalidate();
    }

    fn on_drag(&mut self, x: f32, y: f32) {
        let Some(doc) = self.doc.clone() else { return };
        // 画面の外へドラッグしたら自動スクロール
        if y < 0.0 {
            self.scroll_by(-1);
        } else if y > self.text_height() {
            self.scroll_by(1);
        }
        let y = y.clamp(0.0, self.text_height() - 1.0);
        if let (Some(off), Some((a, _))) = (self.offset_at(&doc, x.max(self.gutter_width()), y), self.sel) {
            self.sel = Some((a, off));
            self.invalidate();
        }
    }

    fn select_word_at(&mut self, x: f32, y: f32) {
        let Some(doc) = self.doc.clone() else { return };
        if x < self.gutter_width() || y >= self.text_height() {
            return;
        }
        let line_no = self.top + (y / self.line_h) as u64;
        let Some(line) = doc.lines(line_no, 1).into_iter().next() else { return };
        let Some(off) = self.offset_at(&doc, x, y) else { return };
        let chars: Vec<char> = line.text.chars().collect();
        let prefix_chars = doc.decode(&doc.read(line.start, off)).chars().count();
        let i = prefix_chars.min(chars.len().saturating_sub(1));
        if chars.is_empty() {
            return;
        }
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 3 };
        let k = class(chars[i]);
        let mut a = i;
        while a > 0 && class(chars[a - 1]) == k && k != 3 {
            a -= 1;
        }
        let mut b = i + 1;
        while b < chars.len() && class(chars[b]) == k && k != 3 {
            b += 1;
        }
        let s: String = chars[..a].iter().collect();
        let m: String = chars[a..b].iter().collect();
        let start = line.start + doc.encode(&s).len() as u64;
        let end = start + doc.encode(&m).len() as u64;
        self.sel = Some((start, end.min(line.end)));
        self.dragging = false;
        self.invalidate();
    }

    /// 画面上の点に対応するバイト位置
    fn offset_at(&self, doc: &Document, x: f32, y: f32) -> Option<u64> {
        let line_no = self.top + (y.max(0.0) / self.line_h) as u64;
        let Some(line) = doc.lines(line_no, 1).into_iter().next() else {
            return doc.progress().done.then(|| doc.len());
        };
        let units = render_units(&line).0;
        let tx = x - self.text_left();
        let idx = unsafe { self.unit_at_x(&units, tx) }.unwrap_or(0).min(units.len());
        let text = String::from_utf16_lossy(&units[..idx]);
        let off = line.start + doc.encode(&text).len() as u64;
        Some(off.min(line.end))
    }

    /// x 座標（行頭基準）に最も近い文字境界の UTF-16 位置
    unsafe fn unit_at_x(&self, units: &[u16], x: f32) -> Result<usize> {
        if x <= 0.0 || units.is_empty() {
            return Ok(0);
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.format, 1.0e6, self.line_h)?;
            let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            if layout.HitTestPoint(x, self.line_h / 2.0, &mut trailing, &mut inside, &mut m).is_ok() {
                let i = m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 };
                return Ok(i.min(units.len()));
            }
            // HitTestPoint が使えない環境（Wine など）向け：幅を二分探索
            // lo = 幅が x 以下になる最大の位置
            let (mut lo, mut hi) = (0usize, units.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if self.text_width(&units[..mid])? <= x {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            // 次の文字境界の方が近ければそちら
            if lo < units.len() {
                let (wl, wr) = (self.text_width(&units[..lo])?, self.text_width(&units[..lo + 1])?);
                if x - wl > wr - x {
                    lo += 1;
                }
            }
            // サロゲートペアの途中は避ける
            if lo > 0 && lo < units.len() && (0xDC00..0xE000).contains(&units[lo]) {
                lo -= 1;
            }
            Ok(lo)
        }
    }

    fn selection(&self) -> Option<(u64, u64)> {
        self.sel.map(|(a, b)| (a.min(b), a.max(b))).filter(|(a, b)| a < b)
    }

    fn selected_text(&self, limit: u64) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let (a, b) = self.selection()?;
        (b - a <= limit).then(|| doc.decode(&doc.read(a, b)).into_owned())
    }

    fn copy(&mut self) {
        let Some((a, b)) = self.selection() else { return };
        if b - a > COPY_LIMIT {
            self.message = format!("選択範囲が大きすぎます（{}）。コピーできるのは {} までです", human_size(b - a), human_size(COPY_LIMIT));
        } else if let Some(text) = self.selected_text(COPY_LIMIT) {
            self.message = match clipboard::set_text(self.hwnd, &text) {
                Ok(()) => format!("コピーしました（{}）", human_size(b - a)),
                Err(e) => format!("コピーに失敗しました: {e}"),
            };
        }
        self.invalidate();
    }

    // ---- スクロール ----

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

    fn text_height(&self) -> f32 {
        self.client_size().1 - self.status_h()
    }

    fn gutter_width(&self) -> f32 {
        let digits = group(self.total_lines()).len().max(4);
        self.char_w * (digits as f32 + 2.0)
    }

    /// 本文の行頭の x 座標（横スクロール込み）
    fn text_left(&self) -> f32 {
        self.gutter_width() + 4.0 - self.x_off
    }

    fn visible_lines(&self) -> usize {
        ((self.text_height() / self.line_h).floor() as usize).max(1)
    }

    fn max_top(&self) -> u64 {
        self.total_lines().saturating_sub(self.visible_lines() as u64)
    }

    fn scroll_by(&mut self, delta: i64) {
        if delta < 0 {
            self.stick_bottom = false;
        }
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

    // ---- 描画 ----

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
            let gutter = self.gutter_width();

            let lines = match &self.doc {
                Some(doc) => doc.lines(self.top, self.visible_lines() + 1),
                None => Vec::new(),
            };

            // 行番号の欄
            brush.SetColor(&COL_GUTTER_BG);
            rt.FillRectangle(&rect(0.0, 0.0, gutter, text_h), brush);

            // 本文
            rt.PushAxisAlignedClip(&rect(gutter, 0.0, w, text_h), D2D1_ANTIALIAS_MODE_ALIASED);
            // 直前に描いた行のレイアウトは使い回す（1 行スクロールなら作り直すのは 1 行だけ）
            let mut old = std::mem::take(&mut *self.layout_cache.borrow_mut());
            let mut cache = HashMap::with_capacity(lines.len());
            for (i, line) in lines.iter().enumerate() {
                let y = i as f32 * self.line_h;
                let entry = match old.remove(&line.start) {
                    Some(c) if c.end == line.end => c,
                    _ => {
                        let (units, shown) = render_units(line);
                        let layout = self.dw.CreateTextLayout(&units, &self.format, 1.0e6, self.line_h)?;
                        CachedLine { end: line.end, layout, units, shown }
                    }
                };
                self.draw_line(&rt, brush, line, &entry, self.text_left(), y)?;
                cache.insert(line.start, entry);
            }
            *self.layout_cache.borrow_mut() = cache;
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

    unsafe fn draw_line(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, line: &Line, c: &CachedLine, x: f32, y: f32) -> Result<()> {
        let (layout, units, shown) = (&c.layout, &c.units, c.shown);
        unsafe {
            if let Some(doc) = &self.doc {
                // 選択範囲
                if let Some((a, b)) = self.selection() {
                    let next = line.end + newline_len(doc, line);
                    if a < next.max(line.start + 1) && b > line.start {
                        let (ua, ub) = self.span_units(doc, line, a, b, shown);
                        self.fill_units(rt, brush, layout, units, ua, ub, x, y, COL_SELECT)?;
                        // 改行まで選択していたら行末に印
                        if b > line.end && a <= line.end {
                            let ex = x + self.text_width(&units[..shown])?;
                            rt.FillRectangle(&rect(ex, y, ex + self.char_w, y + self.line_h), brush);
                        }
                    }
                }
                // 検索一致（選択と同じなら一致の色を優先）
                if let Some((hs, he)) = self.hit {
                    if hs < line.end.max(line.start + 1) && he > line.start {
                        let (ua, ub) = self.span_units(doc, line, hs, he, shown);
                        self.fill_units(rt, brush, layout, units, ua, ub, x, y, COL_MATCH)?;
                    }
                }
            }
            brush.SetColor(&COL_TEXT);
            rt.DrawTextLayout(Vector2 { X: x, Y: y }, layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            // カーソル
            if let (Some(doc), Some(_)) = (&self.doc, self.sel) {
                let c = self.caret();
                if c >= line.start && c <= line.end && self.prompt.is_none() {
                    let (ua, _) = self.span_units(doc, line, c, c, shown);
                    let cx = x + self.text_width(&units[..ua as usize])?;
                    brush.SetColor(&COL_CARET);
                    rt.FillRectangle(&rect(cx - 0.5, y + 1.0, cx + 1.5, y + self.line_h - 1.0), brush);
                }
            }
        }
        Ok(())
    }

    /// バイト範囲 [s, e) を、この行の描画文字列中の UTF-16 範囲に直す
    fn span_units(&self, doc: &Document, line: &Line, s: u64, e: u64, shown: usize) -> (u32, u32) {
        let s = s.clamp(line.start, line.end);
        let e = e.clamp(s, line.end);
        let a = decode_units(doc, &doc.read(line.start, s)).min(shown);
        let b = (a + decode_units(doc, &doc.read(s, e))).min(shown);
        (a as u32, b as u32)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn fill_units(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        layout: &IDWriteTextLayout,
        units: &[u16],
        a: u32,
        b: u32,
        x: f32,
        y: f32,
        color: D2D1_COLOR_F,
    ) -> Result<()> {
        if b <= a {
            return Ok(());
        }
        unsafe {
            brush.SetColor(&color);
            let mut m = [DWRITE_HIT_TEST_METRICS::default(); 8];
            let mut n = 0u32;
            if layout.HitTestTextRange(a, b - a, x, y, Some(&mut m), &mut n).is_ok() {
                for r in &m[..n as usize] {
                    rt.FillRectangle(&rect(r.left, y, r.left + r.width, y + self.line_h), brush);
                }
            } else {
                // HitTestTextRange が使えない環境（Wine など）向け: 前後の文字列幅から求める
                let l = self.text_width(&units[..a as usize])?;
                let r = self.text_width(&units[..b as usize])?;
                rt.FillRectangle(&rect(x + l, y, x + r, y + self.line_h), brush);
            }
        }
        Ok(())
    }

    unsafe fn text_width(&self, units: &[u16]) -> Result<f32> {
        if units.is_empty() {
            return Ok(0.0);
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.format, 1.0e6, self.line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(m.widthIncludingTrailingWhitespace)
        }
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

    fn status_left(&self) -> String {
        let flags = format!(
            "{}{}",
            if self.opts.case_sensitive { "[Aa]" } else { "" },
            if self.opts.regex { "[.*]" } else { "" }
        );
        match self.prompt {
            Some(Prompt::Find) => format!("検索{flags}: {}|  Enter:次 Shift+Enter:前 Alt+C:大小区別 Alt+R:正規表現 Esc:取消", self.input),
            Some(Prompt::GotoLine) => format!("行番号へ移動: {}|", self.input),
            None if self.saving.is_some() => {
                let job = self.saving.as_ref().unwrap();
                let pct = job.written.load(Ordering::Relaxed) as f64 * 100.0 / job.total.max(1) as f64;
                let name = job.target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                format!("保存中… {:.0}%  {name}  (Esc で中止)", pct.min(100.0))
            }
            None => match &self.search {
                Some(job) => {
                    let pct = job.ctl.scanned.load(Ordering::Relaxed) as f64 * 100.0 / job.span.max(1) as f64;
                    let dir = if job.dir == Direction::Forward { "↓" } else { "↑" };
                    format!("検索中{dir}{flags} 「{}」 {:.0}%  {:.1} 秒  (Esc で中止)", self.last_find, pct.min(100.0), job.started.elapsed().as_secs_f64())
                }
                None => self.message.clone(),
            },
        }
    }

    unsafe fn draw_status(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, h: f32) -> Result<()> {
        let top = h - self.status_h();
        unsafe {
            brush.SetColor(&COL_STATUS_BG);
            rt.FillRectangle(&rect(0.0, top, w, h), brush);
            brush.SetColor(&COL_STATUS_TEXT);
        }
        let left = self.status_left();
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
                let sel = match self.selection() {
                    Some((a, b)) => format!("選択 {}   ", human_size(b - a)),
                    None => String::new(),
                };
                let follow = if self.follow { "追従中   " } else { "" };
                let modified = if self.is_modified() { "変更あり   " } else { "" };
                let enc = if doc.forced_encoding().is_some() { format!("{}（指定）", doc.encoding().name()) } else { doc.encoding().name() };
                // カーソルの行と列（列は文字数）
                let c = self.caret();
                let pos = match doc.line_of_offset(c).and_then(|l| doc.line_range(l).map(|r| (l, r))) {
                    Some((l, (s, _))) if c - s <= 1 << 20 => {
                        let col = doc.decode(&doc.read(s, c)).chars().count() + 1;
                        format!("{} 行, {} 列", group(l + 1), group(col as u64))
                    }
                    Some((l, _)) => format!("{} 行", group(l + 1)),
                    None => format!("{} 行目付近", group(self.top + 1)),
                };
                format!("{modified}{follow}{idx}{sel}{pos} / {lines} 行   {enc}   {}   {}", doc.newline_name(), human_size(doc.len()))
            }
        };
        unsafe {
            // 右側（ファイルの情報）を優先し、左側（メッセージ）は残りの幅で切る
            let units: Vec<u16> = right.encode_utf16().collect();
            let layout = self.dw.CreateTextLayout(&units, &self.format, w, self.line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let rw = m.widthIncludingTrailingWhitespace.min(w * 0.75);
            rt.DrawTextLayout(Vector2 { X: w - rw - 8.0, Y: top + 3.0 }, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
            self.draw_str(rt, brush, &left, 8.0, top + 3.0, w - rw - 24.0)?;
        }
        Ok(())
    }
}

/// 描画する UTF-16 列と、そのうち本文の長さ（省略記号を除く）
fn render_units(line: &Line) -> (Vec<u16>, usize) {
    let mut units: Vec<u16> = line.text.encode_utf16().take(MAX_RENDER_UNITS).collect();
    let shown = units.len();
    if line.truncated || shown == MAX_RENDER_UNITS {
        units.extend("  …(省略)".encode_utf16());
    }
    (units, shown)
}

/// 行末の改行コードのバイト数
fn newline_len(doc: &Document, line: &Line) -> u64 {
    doc.line_range(line.number + 1).map_or(0, |(next, _)| next - line.end)
}

/// バイト列をデコードしたときの UTF-16 単位数
fn decode_units(doc: &Document, bytes: &[u8]) -> usize {
    doc.decode(bytes).encode_utf16().count()
}

fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        OpenClipboard(None).ok()?;
        let result = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let mem = HGLOBAL(h.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut n = 0;
            while *ptr.add(n) != 0 && n < 1 << 20 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n));
            let _ = GlobalUnlock(mem);
            Some(s)
        })();
        let _ = CloseClipboard();
        result
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "オン" } else { "オフ" }
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
