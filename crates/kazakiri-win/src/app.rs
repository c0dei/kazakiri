//! Windows ネイティブ UI（Win32 と Direct2D/DirectWrite）。
//!
//! 見えている行だけを描画するので、ファイルサイズに関係なく描画コストは一定になる。
//! 検索・保存・分割などは別スレッドで行い、UI は止めない。
//!
//! このファイルには、状態（`App`）・ウィンドウプロシージャ・メッセージの振り分け（`handle`）だけを置き、
//! 機能ごとにモジュールへ分けてある。
//!
//! | モジュール | 内容 |
//! |---|---|
//! | `startup` / `deferred` / `fonts` / `util` | 起動、状態を借りずに行う操作（ダイアログなど）、フォント、小さな共通関数 |
//! | `input` / `command` | マウス・キー・文字の入力、コマンド（メニュー ID）の実行 |
//! | `layout` / `paint` | 表示の寸法・スクロール、描画 |
//! | `files` / `search` / `results` | 開く・再読み込み、検索、結果一覧・GREP |
//! | `edit` / `lines` / `multi` / `rect` / `wrap` / `complete` | 編集、行の操作と変換、複数カーソル、矩形選択、折り返し、補完 |
//! | `csv_view` / `diffview` / `bookmarks` / `markers` | CSV 表示、差分・分割表示、ブックマーク、カラーマーカー |
//! | `tabs` / `menubar` / `toolbar` / `status_menu` / `view` / `theme` / `dark` | タブ、メニューバー、ツールバー、ステータスバー、表示設定、配色 |
//! | `find_dialog` / `settings_dialog` / `keys_dialog` | 各ダイアログ |

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use encoding_rs::Encoding;
use kazakiri_core::{Direction, Document, EditKind, Editor, Line, Query, Refresh, SearchControl, SearchOptions, SearchOutcome};
use windows::core::{w, IUnknown, Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Controls::{SetScrollInfo, ShowScrollBar};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::menu::{self, Menus};
use crate::{clipboard, keymap, recent};

mod csv_view;
mod command;
mod deferred;
mod files;
mod fonts;
mod input;
mod layout;
mod paint;
mod search;
mod startup;
mod util;
mod theme;
mod view;
mod edit;
mod results;
mod tabs;
mod rect;
mod dark;
mod find_dialog;
mod bookmarks;
mod diffview;
mod markers;
mod complete;
mod toolbar;
mod keys_dialog;
mod lines;
mod settings_dialog;
mod status_menu;
mod wrap;
mod multi;
use csv_view::{CsvView, Disp, Effect, SortDone, WM_SORT_DONE};
use crate::settings::{self, Settings};
use theme::Theme;
use deferred::*;
use fonts::*;
use util::*;
use edit::{Pending, SaveJob, WM_RUN_PENDING, WM_SAVE_DONE};
use results::{JobDone, Results, ResultsJob, WM_REPLACE_FOUND, WM_RESULTS_DONE};

const APP_NAME: &str = "kazakiri";
/// 1 行のうち描画する最大 UTF-16 単位数（それ以上は「…」）
const MAX_RENDER_UNITS: usize = 8192;
/// クリップボードへコピーできる最大バイト数
const COPY_LIMIT: u64 = 256 << 20;
const TIMER_INDEX: usize = 1;
const TIMER_SEARCH: usize = 2;
const TIMER_FOLLOW: usize = 3;
/// ツールバーのボタンの上にマウスを置いてから、説明（ツールチップ）を出すまでの待ち
const TIMER_TIP: usize = 4;
/// 長い行の横スクロールで取り出す窓の大きさの上限（バイト）。描画の上限（8192 UTF-16 単位）に収まる大きさ
const LONG_LINE_WINDOW: usize = 8000;
/// 検索スレッドからの完了通知
const WM_SEARCH_DONE: u32 = WM_APP + 1;

const fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// 入力欄（ステータスバー上の簡易プロンプト）
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prompt {
    GotoLine,
    /// CSV の列フィルタ（「3 = ERROR」の形）
    ColumnFilter,
    /// ファイルの分割（行数 または サイズ）
    Split,
}

/// 借用中に実行できない処理（モーダルダイアログはメッセージを再入させる）
enum Deferred {
    /// 名前を付けて保存ダイアログを出して保存する
    SaveAs,
    /// 未保存の変更を確認してから行う
    Confirm(Pending),
    /// そのまま行う（ダイアログを伴うことがある）
    Perform(Pending),
    /// GREP 置換の最終確認（文章）
    ConfirmGrepReplace(String),
    /// 検索・置換・GREP のダイアログを開く（タブの番号）
    ShowFind(usize),
    ShowSettings,
    ShowKeys,
    /// 比較するタブを選ぶメニュー
    DiffMenu,
    /// ステータスバーの文字コード（0）・改行コード（1）のメニュー
    StatusMenu(u8),
    /// いまのファイルを変換して上書き保存してよいか確認する（メニューの ID）
    ConfirmConvertHere(u32),
    /// 結合するファイルを選んで、出力先を決める
    Join,
    /// 外部ソートの保存先を選ぶ（降順か）
    ExternalSort(bool),
    /// 結果の一覧の保存先を選ぶ
    SaveResults,
}

impl Gfx {
    /// 色の種類に対応する、文字の色を変えるためのブラシ
    fn brush_for(&self, e: Effect) -> &IUnknown {
        use kazakiri_core::syntax::Kind;
        &self.effects[match e {
            Effect::Mark => 0,
            Effect::LogError => 1,
            Effect::LogWarn => 2,
            Effect::LogDebug => 3,
            Effect::Syn(Kind::Str) => 4,
            Effect::Syn(Kind::Num) => 5,
            Effect::Syn(Kind::Keyword) => 6,
            Effect::Syn(Kind::Comment) => 7,
            Effect::Syn(Kind::Tag) => 8,
            Effect::Syn(Kind::Key) => 9,
        }]
    }
}

struct Gfx {
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    /// 文字の色を変える範囲用（空白の印、ERROR、WARN、DEBUG）
    effects: Vec<IUnknown>,
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
    /// CSV の列幅の世代（変わったら作り直す）
    generation: u64,
    layout: IDWriteTextLayout,
    disp: Disp,
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
    /// 画面の部品（メニュー・タブ・ツールバー・ステータスバー）用のフォントと、その文字の高さ・幅
    ui_format: IDWriteTextFormat,
    ui_line_h: f32,
    ui_char_w: f32,
    /// 画面の部品用フォントでの文字列の幅（フォントを変えたら捨てる）
    ui_width_cache: RefCell<HashMap<Vec<u16>, f32>>,
    /// 設定した本文のフォントが等幅でない（CSV 表示のあいだは、列がそろうよう等幅フォントに替える）
    font_prop: bool,
    /// 等幅フォントに替えている最中
    font_overridden: bool,
    gfx: Option<Gfx>,
    menus: Menus,
    keymap: keymap::Keymap,
    /// ブックマークの行（0 始まり）と、そのときの総行数
    bookmarks: std::collections::BTreeSet<u64>,
    bm_lines: u64,
    /// 直前の編集が行頭から始まったか（ブックマークのずらし方に使う）
    bm_col0: bool,
    /// ツールバー・機能ボタンの上にあるマウス
    hover: Option<toolbar::Hover>,
    /// 出している説明（ツールチップ）：文字、左の位置（x）
    tip: Option<(String, f32)>,
    /// 単語補完の途中（続けて押すと次の候補）
    dabbrev: Option<complete::Dab>,
    markers: Vec<markers::Marker>,
    /// 差分表示（左右に並べた比較）
    diffview: Option<diffview::DiffView>,
    tracking_leave: bool,
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
    /// 次の保存で文字コード・改行コードを変える
    convert: Option<kazakiri_core::Convert>,
    /// 保存が終わったら行う操作
    pending: Option<Pending>,
    /// 検索結果の一覧
    results: Option<Results>,
    /// 実行中の全件検索・GREP・すべて置換
    rjob: Option<ResultsJob>,
    last_replace: String,
    grep_folder: String,
    grep_files: String,
    /// 索引ができたら移動する位置（行, 開始, 終了）
    pending_goto: Option<(u64, u64, u64)>,
    /// CSV / TSV モード
    csv: Option<CsvView>,
    theme: &'static Theme,
    settings: Settings,
    /// 表示していないタブの状態（`tabs[active]` は空の置き場所）
    tabs: Vec<tabs::Tab>,
    active: usize,
    /// タブバーの横スクロール
    tab_scroll: f32,
    /// 終了の確認で、表示中のタブは「保存しない」を選んだ
    discard: bool,
    top: u64,
    x_off: f32,
    prompt: Option<Prompt>,
    input: String,
    last_find: String,
    last_filter: String,
    /// 検索語・置換語の履歴
    history: crate::history::History,
    /// 置換の対象を絞る範囲（複数行を選んで Ctrl+H したとき）
    replace_scope: Option<(u64, u64)>,
    /// ダイアログを開いたときの、複数行の選択範囲
    dlg_scope: Option<(u64, u64)>,
    /// GREP でサブフォルダも探す
    grep_recursive: bool,
    /// IME で変換中の文字
    comp: Option<edit::Comp>,
    /// CSV の、前の行から続く行の一覧
    csv_cont: Option<csv_view::CsvCont>,
    /// 操作の途中で、検索ダイアログ（のタブ）を開きたいとき
    want_dialog: Option<usize>,
    /// 外部ソートの保存先を選びたいとき（降順か）
    want_sort: Option<bool>,
    opts: SearchOptions,
    search: Option<SearchJob>,
    next_job: u64,
    /// 現在の検索一致（バイト範囲）
    hit: Option<(u64, u64)>,
    /// 選択（アンカー, 先端）のバイト位置
    sel: Option<(u64, u64)>,
    dragging: bool,
    /// 上書きモード（Insert キー）
    overwrite: bool,
    /// 矩形選択
    rect: Option<rect::RectSel>,
    /// マルチカーソル（ほかのカーソル）
    multi: Option<multi::Multi>,
    /// Alt を矩形選択に使った（離したときにメニューを開かない）
    alt_used: bool,
    /// ドラッグで並べ替え中のタブ（番号, 動かしたか）
    tab_drag: Option<(usize, bool)>,
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
        if let Err((step, e)) = startup::run_inner() {
            let msg = wide(&format!("起動に失敗しました（{step}）: {e}"));
            MessageBoxW(None, PCWSTR(msg.as_ptr()), w!("kazakiri"), MB_ICONERROR);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 標準のメニューバーを暗く塗るメッセージは、App を借りる前に処理する（描画の最中に同期で届くため）
    if let Some(r) = unsafe { dark::bar_message(hwnd, msg, wparam, lparam) } {
        return r;
    }
    let result = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return None };
        let app = guard.as_mut()?;
        let (handled, deferred) = unsafe { app.handle(msg, wparam, lparam) };
        let deferred = deferred.or_else(|| app.want_dialog.take().map(Deferred::ShowFind)).or_else(|| app.want_sort.take().map(Deferred::ExternalSort));
        Some((handled, deferred))
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
                WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                    let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
                    let (x, y) = (pt.x as f32 / scale, pt.y as f32 / scale - self.chrome_h());
                    let (cw, ch) = self.client_size();
                    let _ = cw;
                    // 本文の中だけ I ビーム。メニュー・ツールバー・タブ・行番号・結果一覧・ステータスバーは矢印、ボタンは手
                    let in_text = self.diffview.is_none() && y >= 0.0 && x >= self.gutter_width() && y < ch - self.status_h() - self.panel_h();
                    let id = if self.hover_at(x, y).is_some() { IDC_HAND } else if in_text { IDC_IBEAM } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, id) {
                        SetCursor(Some(c));
                    }
                    (Some(LRESULT(1)), None)
                }
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
                    if done {
                        self.apply_pending_goto();
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
                WM_TIMER if wparam.0 == TIMER_TIP => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_TIP);
                    self.show_tip();
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
                    if self.search.is_none() && self.saving.is_none() && self.rjob.is_none() {
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
                diffview::WM_DIFF_DONE => {
                    let done = Box::from_raw(lparam.0 as *mut diffview::DiffDone);
                    self.on_diff_done(*done);
                    (DONE, None)
                }
                WM_RUN_PENDING => (DONE, self.pending.take().map(Deferred::Perform)),
                csv_view::WM_CONT_DONE => {
                    self.layout_cache.borrow_mut().clear();
                    self.invalidate();
                    (DONE, None)
                }
                WM_SORT_DONE => {
                    let done = Box::from_raw(lparam.0 as *mut SortDone);
                    self.on_sort_done(*done);
                    (DONE, None)
                }
                WM_RESULTS_DONE | WM_REPLACE_FOUND => {
                    let done = Box::from_raw(lparam.0 as *mut JobDone);
                    self.on_job_done(*done);
                    (DONE, None)
                }
                WM_CLOSE => {
                    if self.saving.is_some() {
                        self.message = "保存が終わるまでお待ちください".into();
                        self.invalidate();
                        return (DONE, None);
                    }
                    self.clear_discard();
                    (DONE, self.guarded(Pending::Close))
                }
                // 変換中の文字を自前で描くので、IME の標準の変換ウィンドウは出さない
                WM_IME_SETCONTEXT if self.ime_inline_on() => {
                    let l = LPARAM(lparam.0 & !(edit::ISC_SHOWUICOMPOSITIONWINDOW as isize));
                    (Some(DefWindowProcW(self.hwnd, msg, wparam, l)), None)
                }
                WM_IME_STARTCOMPOSITION => {
                    self.update_ime_position();
                    if self.ime_inline_on() { (DONE, None) } else { (None, None) }
                }
                WM_IME_COMPOSITION if self.ime_result(lparam) => {
                    self.comp = None;
                    self.invalidate();
                    (DONE, None)
                }
                WM_IME_COMPOSITION if self.ime_inline_on() && lparam.0 as u32 & edit::GCS_COMPSTR_BIT != 0 => {
                    self.read_composition();
                    (DONE, None)
                }
                WM_IME_ENDCOMPOSITION if self.ime_inline_on() => {
                    self.comp = None;
                    self.invalidate();
                    (DONE, None)
                }
                WM_INITMENUPOPUP => {
                    let forced = self.doc.as_ref().map(|d| d.forced_encoding());
                    self.menus.refresh(self.opts.case_sensitive, self.opts.regex, self.follow, forced, &self.recent);
                    self.menus.refresh_clips(&clipboard::history());
                    self.menus.refresh_view(self.csv.as_ref().map(|v| (v.delim, v.header)), &self.settings);
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
                        // 1 ページ = 画面の幅（長い行を、画面の幅ずつ読み進められる）
                        SB_PAGELEFT => self.hscroll_by(-(self.client_size().0 - self.gutter_width()).max(self.char_w * 40.0)),
                        SB_PAGERIGHT => self.hscroll_by((self.client_size().0 - self.gutter_width()).max(self.char_w * 40.0)),
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
                    // ホイールの座標は画面座標
                    let mut pt = POINT { x: loword(lparam.0 as usize) as i16 as i32, y: hiword(lparam.0 as usize) as i16 as i32 };
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                    let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
                    if self.diffview.is_some() {
                        self.diff_wheel(pt.x as f32 / scale, -delta * 3 / 120);
                    } else if GetKeyState(VK_CONTROL.0 as i32) < 0 {
                        // Ctrl+ホイールで文字の大きさ
                        self.zoom(if delta > 0 { 1.0 } else { -1.0 });
                    } else if self.in_panel(pt.y as f32 / scale - self.chrome_h() - self.pin_h()) {
                        self.panel_scroll(-delta * 3 / 120);
                    } else if GetKeyState(VK_SHIFT.0 as i32) < 0 {
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
                    // ツールバー・機能ボタン・メニューバー・ステータスバー
                    if let Some(r) = self.chrome_press(x, y) {
                        return (DONE, r);
                    }
                    if y < 0.0 {
                        let d = self.tab_click(x, false);
                        self.tab_drag_start(x);
                        return (DONE, d);
                    }
                    if self.diffview.is_some() {
                        self.diff_focus_at(x);
                        return (DONE, None);
                    }
                    // 固定した見出し行
                    if y < self.pin_h() {
                        return (DONE, None);
                    }
                    let y = y - self.pin_h();
                    if self.in_panel(y) {
                        return (DONE, self.panel_click(y));
                    }
                    if GetKeyState(VK_MENU.0 as i32) < 0 && x >= self.gutter_width() && y < self.text_height() {
                        self.rect_start(x, y);
                    } else if GetKeyState(VK_CONTROL.0 as i32) < 0 && x >= self.gutter_width() && y < self.text_height() {
                        self.multi_click(x, y);
                    } else {
                        self.on_mouse_down(x, y, GetKeyState(VK_SHIFT.0 as i32) < 0);
                    }
                    (DONE, None)
                }
                WM_MBUTTONDOWN => {
                    let (x, y) = self.mouse_pos(lparam);
                    // タブを中ボタンでクリックしたら閉じる
                    (DONE, if (-self.tab_h()..0.0).contains(&y) { self.tab_click(x, true) } else { None })
                }
                WM_LBUTTONDBLCLK => {
                    let (x, y) = self.mouse_pos(lparam);
                    // 素早く続けて押したときも、1 回目と同じボタンの操作として扱う（ダブルクリックとして無視しない）
                    if let Some(r) = self.chrome_press(x, y) {
                        return (DONE, r);
                    }
                    if y < -self.tab_h() {
                        return (DONE, None);
                    }
                    if y < 0.0 {
                        self.tab_double_click(x);
                        return (DONE, None);
                    }
                    if y < self.pin_h() {
                        return (DONE, None);
                    }
                    let y = y - self.pin_h();
                    if self.in_panel(y) {
                        return (DONE, self.panel_click(y));
                    }
                    self.select_word_at(x, y);
                    (DONE, None)
                }
                WM_MOUSEMOVE if self.tab_drag.is_some() => {
                    let (x, _) = self.mouse_pos(lparam);
                    self.tab_drag_move(x);
                    (DONE, None)
                }
                WM_LBUTTONUP if self.tab_drag.is_some() => {
                    self.tab_drag = None;
                    let _ = ReleaseCapture();
                    (DONE, None)
                }
                0x02A3 => {
                    // WM_MOUSELEAVE
                    self.mouse_left();
                    (DONE, None)
                }
                WM_MOUSEMOVE if !self.dragging && self.tab_drag.is_none() => {
                    let (x, y) = self.mouse_pos(lparam);
                    self.update_hover(x, y);
                    (DONE, None)
                }
                WM_MOUSEMOVE if self.dragging => {
                    let (x, y) = self.mouse_pos(lparam);
                    self.on_drag(x, y - self.pin_h());
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
                // 検索欄で Alt+Enter：すべて検索して一覧に。置換欄で Alt+A：すべて置換
                WM_SYSKEYUP if self.alt_used && VIRTUAL_KEY(wparam.0 as u16) == VK_MENU => {
                    self.alt_used = false;
                    (DONE, None)
                }
                WM_SYSKEYDOWN
                    if self.prompt.is_none()
                        && GetKeyState(VK_SHIFT.0 as i32) < 0
                        && matches!(VIRTUAL_KEY(wparam.0 as u16), VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN) =>
                {
                    self.rect_key(VIRTUAL_KEY(wparam.0 as u16));
                    (DONE, None)
                }
                WM_SYSCHAR if wparam.0 == 0x0D => (DONE, None),
                WM_SYSKEYDOWN if matches!(VIRTUAL_KEY(wparam.0 as u16), VK_C | VK_R) => {
                    let id = if VIRTUAL_KEY(wparam.0 as u16) == VK_C { menu::ID_CASE } else { menu::ID_REGEX };
                    (None, self.command(id))
                }
                WM_SYSCHAR if matches!(char::from_u32(wparam.0 as u32), Some('c' | 'C' | 'r' | 'R')) => (DONE, None),
                // Ctrl+Space（単語補完）は文字として入れない
                WM_CHAR if wparam.0 == 0x20 && GetKeyState(VK_CONTROL.0 as i32) < 0 => (DONE, None),
                WM_CHAR => {
                    self.on_char(wparam.0 as u32);
                    (DONE, None)
                }
                WM_DROPFILES => {
                    let drop = HDROP(wparam.0 as _);
                    // 複数のファイルを落としたら、それぞれタブで開く
                    let mut buf = vec![0u16; 32768];
                    let count = DragQueryFileW(drop, u32::MAX, None);
                    let mut paths = Vec::new();
                    for i in 0..count {
                        let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                        if n > 0 {
                            paths.push(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
                        }
                    }
                    DragFinish(drop);
                    for p in paths {
                        self.open(p, None);
                    }
                    (DONE, None)
                }
                WM_DESTROY => {
                    // ウィンドウの位置と大きさを覚える（最大化していれば、戻したときの大きさ）
                    let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                    if GetWindowPlacement(self.hwnd, &mut wp).is_ok() {
                        let r = wp.rcNormalPosition;
                        self.settings.window = Some((r.left, r.top, r.right - r.left, r.bottom - r.top, wp.showCmd == SW_SHOWMAXIMIZED.0 as u32));
                        self.settings.save();
                    }
                    self.save_session();
                    self.cancel_search();
                    PostQuitMessage(0);
                    (DONE, None)
                }
                _ => (None, None),
            }
        }
    }

    // ---- ファイル ----

}
