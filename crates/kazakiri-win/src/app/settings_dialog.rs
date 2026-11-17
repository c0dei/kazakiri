//! 設定ダイアログ（ファイル → 設定）。
//!
//! モードレスの 1 枚のウィンドウ。左に分類の一覧と検索欄（EmEditor の「すべての設定のプロパティ」のように、
//! 検索語に当てはまる項目を含む分類だけに絞る）、右に選んだ分類のページを出す。
//! 部品の操作は「下書き」を変えるだけで、「保存」（Enter も。押してもダイアログは閉じない）を押したときに初めて、アプリと `settings.ini` へ反映する
//! （表示メニューと同じ操作を呼ぶものもある）。保存せずに閉じるときは確認を出す。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::Dialogs::{ChooseFontW, CF_INITTOLOGFONTSTRUCT, CF_SCREENFONTS, CHOOSEFONTW};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};

use super::find_dialog::{make_child, pick_folder_titled, rgb_ref, wide};
use super::*;

/// ダイアログのウィンドウ（0 ならまだない）。メッセージループの `IsDialogMessage` が見る
pub(super) static SET_DLG: AtomicIsize = AtomicIsize::new(0);
static DARK: AtomicBool = AtomicBool::new(false);
/// ダークとライトの背景・入力欄のブラシ（[ライト, ダーク]）
static BG_BRUSH: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
static CARD_BRUSH: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
static EDIT_BRUSH: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
/// 本文のフォントと、見出し用の大きなフォント（HFONT）
static UI_FONT: AtomicIsize = AtomicIsize::new(0);
static FIRST: AtomicBool = AtomicBool::new(true);
/// 保存していない変更がある（下書きが、アプリの設定と違う）
static DIRTY: AtomicBool = AtomicBool::new(false);
/// 「保存しました」を出している間
static SAVED: AtomicBool = AtomicBool::new(false);
/// 「保存しました」を消すタイマー
const SAVED_TIMER: usize = 1;

const CLASS: PCWSTR = w!("KazakiriSettingsDialog");

const ID_LINENUM: i32 = 214;
/// テーマの選び方（システム・ライト・ダーク）と、アクセントカラーの見本の ID（先頭から連番）
const ID_THEME_BASE: i32 = 250;
const ID_ACCENT_BASE: i32 = 260;
/// 大きさの − ＋ ボタン
const ID_SIZE_DEC: i32 = 270;
const ID_SIZE_INC: i32 = 271;
const ID_UISIZE_DEC: i32 = 272;
const ID_UISIZE_INC: i32 = 273;
const ID_TRAY: i32 = 226;
const ID_MODEBAR: i32 = 227;
const ID_FONT: i32 = 204;
const ID_FONT_PICK: i32 = 205;
const ID_UIFONT_PICK: i32 = 208;
const ID_SIZE: i32 = 206;
const ID_UIFONT: i32 = 207;
const ID_UISIZE: i32 = 209;
const ID_WRAP: i32 = 210;
const ID_TAB: i32 = 212;
const ID_WS: i32 = 220;
const ID_LOG: i32 = 221;
const ID_SYN: i32 = 222;
const ID_TOOLBAR: i32 = 223;
const ID_FKEYS: i32 = 224;
const ID_BACKUP: i32 = 230;
const ID_WHEEL: i32 = 213;
const ID_MDAUTO: i32 = 231;
const ID_TITLE: i32 = 18;
const ID_SEARCH: i32 = 300;
const ID_CATS: i32 = 301;
const ID_BKSAVE: i32 = 232;
const ID_BKDIR: i32 = 233;
const ID_BKDIR_PICK: i32 = 234;
const ID_BKNUM: i32 = 235;
const ID_AUTOMIN: i32 = 236;
const ID_KEYS: i32 = 240;
const ID_CLOSE: i32 = 241;

/// チェックボックスの ID と、同じ働きの表示メニューのコマンド
const CHECKS: &[(i32, &str, u32)] = &[
    (ID_WRAP, "長い行を折り返して表示(&W)", menu::ID_WRAP),
    (ID_WS, "空白・タブ・改行を表示(&S)", menu::ID_WHITESPACE),
    (ID_LOG, "ログの ERROR / WARN などに色を付ける(&L)", menu::ID_LOG_COLORS),
    (ID_SYN, "構文の色分け（JSON・SQL・XML など）(&X)", menu::ID_SYNTAX),
    (ID_TOOLBAR, "ツールバーを表示(&B)", menu::ID_TOOLBAR),
    (ID_MODEBAR, "モードバー（CSV の区切りなどのボタンの行）を表示(&E)", menu::ID_MODEBAR),
    (ID_FKEYS, "機能ボタンの列を表示（ステータスバーの上）(&U)", menu::ID_FKEYS),
    (ID_BACKUP, "GREP 置換の前にバックアップ (.bak) を作る(&B)", menu::ID_GREP_BACKUP),
];

/// 分類（左の一覧）
const CATS: [&str; 9] = ["基本", "スクロール", "外観", "表示", "検索・GREP", "バックアップ", "自動保存", "Markdown", "キーボード"];
const EN_CHANGE_CODE: u32 = 0x0300;
const LBN_SELCHANGE_CODE: u32 = 1;

/// 設定の 1 行（カード）。`toggle` があれば、右端にスイッチを出す
struct Row {
    page: usize,
    /// ページの上端からの位置（DIP）
    y: i32,
    title: &'static str,
    desc: &'static str,
    toggle: Option<i32>,
    /// 行の高さ（DIP）
    h: i32,
    /// フォントの見え方を見せる行
    preview: bool,
    /// カードのない小見出し
    heading: bool,
    /// 選択肢（先頭の ID, 種類 0 = 切り替え・1 = 色の見本, 個数）
    choice: Option<(i32, u8, usize)>,
}

/// アクセントカラーの見本（先頭の「Windows の設定の色」のあとに並べる）
const ACCENTS: [u32; 5] = [0x0067C0, 0x107C10, 0xCA5010, 0xE3008C, 0x8764B8];

/// コントロールの置き方：ウィンドウの大きさに合わせて置き直す（`relayout`）
#[derive(Clone, Copy)]
enum Anchor {
    /// カードの右端から（右端までの距離）
    CardRight(i32),
    /// カードの幅いっぱい
    CardWidth,
    /// 下端の手前まで伸ばす
    ListHeight,
    /// 右下の隅から（右・下までの距離）
    Corner(i32, i32),
}

#[derive(Clone, Copy)]
struct Place {
    id: i32,
    anchor: Anchor,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// ダイアログの配色（Windows 11 の設定アプリに合わせた、背景・カード・文字・アクセント）
struct Pal {
    bg: D2D1_COLOR_F,
    card: D2D1_COLOR_F,
    cardb: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    dim: D2D1_COLOR_F,
}

const PAL_DARK: Pal = Pal { bg: rgb(0x20, 0x20, 0x20), card: rgb(0x2b, 0x2b, 0x2b), cardb: rgb(0x38, 0x38, 0x38), text: rgb(0xf2, 0xf2, 0xf2), dim: rgb(0x9a, 0x9a, 0x9a) };
const PAL_LIGHT: Pal = Pal { bg: rgb(0xf3, 0xf3, 0xf3), card: rgb(0xfb, 0xfb, 0xfb), cardb: rgb(0xe5, 0xe5, 0xe5), text: rgb(0x1a, 0x1a, 0x1a), dim: rgb(0x6b, 0x6b, 0x6b) };

/// いまのアクセントカラー（設定で選んだ色、なければ Windows の設定の色）
fn accent_now() -> D2D1_COLOR_F {
    super::theme::accent(DARK.load(Ordering::Relaxed))
}

/// ドロップダウン（テーマ・アクセントカラー）の位置と大きさ（DIP）。高さは、開いた一覧の分も含む
fn combo_rect(row_y: i32, row_h: i32, card_w: i32) -> (i32, i32, i32, i32) {
    let right = PAGE_X + card_w - 16;
    (right - 170, PAGE_Y + row_y + (row_h - 28) / 2, 170, 28 + 180)
}

const THEME_NAMES: [&str; 3] = ["システム", "ライト", "ダーク"];
const ACCENT_NAMES: [&str; 6] = ["Windows の設定の色", "青", "緑", "橙", "ピンク", "紫"];

/// アクセントカラーの見本 i（0 は Windows の設定の色）
fn swatch_color(i: usize) -> D2D1_COLOR_F {
    if i == 0 {
        return super::theme::system_accent();
    }
    let c = ACCENTS[(i - 1).min(ACCENTS.len() - 1)];
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// ウィンドウの大きさに合わせて、カードの幅とコントロールの位置を置き直す
/// ウィンドウの大きさをドラッグで変えている最中（`WM_ENTERSIZEMOVE`〜`WM_EXITSIZEMOVE`）。このあいだは、背景を GDI で簡易に描いて軽くする
static SIZING: AtomicBool = AtomicBool::new(false);

fn relayout(dlg: HWND) {
    unsafe {
        let scale = GetDpiForWindow(dlg).max(96) as f32 / 96.0;
        let mut rc = RECT::default();
        let _ = GetClientRect(dlg, &mut rc);
        let (cw, ch) = ((rc.right as f32 / scale) as i32, (rc.bottom as f32 / scale) as i32);
        let new_w = (cw - PAGE_X - 24).max(440);
        let (places, rows, ids, scroll, other) = CTRLS.with(|c| {
            let mut c = c.borrow_mut();
            let Some(ct) = c.as_mut() else { return (Vec::new(), Vec::new(), Vec::new(), 0, std::collections::HashSet::new()) };
            ct.card_w = new_w;
            // スクロール量は、ページの内容と表示領域の高さに収める
            let max = max_scroll(ch, ct.page_h.get(ct.cur).copied().unwrap_or(0));
            ct.scroll = ct.scroll.clamp(0, max);
            // 見えていないページの部品は、置き直さない（そのページを出すときに、`show_page` が置く）
            let other: std::collections::HashSet<i32> = ct.pages.iter().filter(|(i, pg, _)| *i != 0 && *pg != ct.cur).map(|(i, _, _)| *i).collect();
            (ct.places.clone(), ct.rows.iter().filter(|r| r.page == ct.cur).map(|r| (r.y, r.h, r.toggle, r.choice)).collect::<Vec<_>>(), ct.all.iter().map(|(i, _)| *i).collect::<Vec<_>>(), ct.scroll, other)
        });
        if ids.is_empty() {
            return;
        }
        let p = |v: i32| (v as f32 * scale).round() as i32;
        // 置く位置を集めてから、まとめて動かす（1 つずつ動かすより、再描画が少ない）。前回と同じ位置の部品は動かさない
        let mut moves: Vec<(i32, i32, i32, i32, i32)> = Vec::new();
        let mut put = |id: i32, x: i32, y: i32, w: i32, h: i32| {
            if !other.contains(&id) {
                moves.push((id, p(x), p(y), p(w), p(h)));
            }
        };
        // スクロールで動く部品（ページの中身）の ID。表示領域からはみ出した部分を切り取る
        let mut moving: Vec<i32> = Vec::new();
        for pl in places {
            match pl.anchor {
                Anchor::CardRight(off) => {
                    put(pl.id, PAGE_X + new_w - off, pl.y - scroll, pl.w, pl.h);
                    moving.push(pl.id);
                }
                Anchor::CardWidth => {
                    put(pl.id, pl.x, pl.y - scroll, new_w, pl.h);
                    moving.push(pl.id);
                }
                Anchor::ListHeight => put(pl.id, pl.x, pl.y, pl.w, (ch - pl.y - 14).max(100)),
                Anchor::Corner(dx, dy) => put(pl.id, cw - dx, ch - dy, pl.w, pl.h),
            }
        }
        for (y, h, toggle, choice) in rows {
            let y = y - scroll;
            if let Some(id) = toggle {
                let (tx, ty, tw, th) = toggle_rect(y, h, new_w);
                put(id, tx, ty, tw, th);
                moving.push(id);
            }
            if let Some((first, kind, n)) = choice {
                let _ = (kind, n);
                let (cx, cy, cw, ch) = combo_rect(y, h, new_w);
                put(first, cx, cy, cw, ch);
                moving.push(first);
            }
        }
        thread_local! {
            static PLACED: RefCell<HashMap<i32, (i32, i32, i32, i32)>> = RefCell::new(HashMap::new());
        }
        let changed: Vec<(i32, i32, i32, i32, i32)> = PLACED.with(|m| {
            let mut m = m.borrow_mut();
            moves
                .into_iter()
                .filter(|&(id, x, y, w, h)| {
                    let same = m.get(&id) == Some(&(x, y, w, h));
                    m.insert(id, (x, y, w, h));
                    !same
                })
                .collect()
        });
        if !changed.is_empty() {
            let flags = SWP_NOZORDER | SWP_NOACTIVATE;
            match BeginDeferWindowPos(changed.len() as i32) {
                Ok(mut hdwp) => {
                    for &(id, x, y, w, h) in &changed {
                        match DeferWindowPos(hdwp, ctl(id), None, x, y, w, h, flags) {
                            Ok(next) => hdwp = next,
                            Err(_) => break,
                        }
                    }
                    let _ = EndDeferWindowPos(hdwp);
                }
                Err(_) => {
                    for &(id, x, y, w, h) in &changed {
                        let _ = SetWindowPos(ctl(id), None, x, y, w, h, flags);
                    }
                }
            }
        }
        clip_to_view(dlg, &moving, p(PAGE_Y), rc.bottom - p(FOOTER_H));
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// ページの内容がスクロールできる最大の量（DIP）。`ch` はダイアログのクライアントの高さ、`content` はページの内容の高さ
fn max_scroll(ch: i32, content: i32) -> i32 {
    (content - (ch - FOOTER_H - PAGE_Y)).max(0)
}

/// スクロールで動く部品を、ページの表示領域（タイトルの下から、ボタンの行の上まで）で切り取る。
/// はみ出した部品は、タイトルやボタンの上に重ならない（`top`・`bottom` はクライアントの座標。ピクセル）
unsafe fn clip_to_view(dlg: HWND, ids: &[i32], top: i32, bottom: i32) {
    thread_local! {
        /// 部品ごとに、前に設定した切り取り（上, 下, 幅。切り取りなしは (0, 0, 0)）。同じなら設定し直さない
        static CLIPPED: RefCell<HashMap<i32, (i32, i32, i32)>> = RefCell::new(HashMap::new());
    }
    unsafe {
        for &id in ids {
            let h = ctl(id);
            if h.0.is_null() {
                continue;
            }
            let mut wr = RECT::default();
            if GetWindowRect(h, &mut wr).is_err() {
                continue;
            }
            let mut pts = [POINT { x: wr.left, y: wr.top }, POINT { x: wr.right, y: wr.bottom }];
            MapWindowPoints(None, Some(dlg), &mut pts);
            let (t, b, w) = (pts[0].y, pts[1].y, pts[1].x - pts[0].x);
            let (it, ib) = (t.max(top), b.min(bottom));
            let key = if t >= top && b <= bottom { (0, 0, 0) } else if it >= ib { (-1, -1, -1) } else { (it - t, ib - t, w) };
            if CLIPPED.with(|m| m.borrow_mut().insert(id, key)) == Some(key) {
                continue;
            }
            match key {
                (0, 0, 0) => {
                    let _ = SetWindowRgn(h, None, false);
                }
                (-1, -1, -1) => {
                    SetWindowRgn(h, Some(CreateRectRgn(0, 0, 0, 0)), false);
                }
                (a, c, w) => {
                    SetWindowRgn(h, Some(CreateRectRgn(0, a, w, c)), false);
                }
            }
        }
    }
}

/// ページを `delta`（DIP）だけスクロールする（正は下へ）
fn scroll_by(dlg: HWND, delta: i32) {
    let changed = CTRLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(ct) = c.as_mut() else { return false };
        ct.scroll += delta;
        delta != 0
    });
    if changed {
        relayout(dlg);
    }
}

/// 部品（ID）が表示領域にはみ出しているなら、見える位置までスクロールする（Tab でフォーカスが動いたときなど）
unsafe fn ensure_visible(dlg: HWND, id: i32) {
    unsafe {
        let on_page = CTRLS.with(|c| c.borrow().as_ref().is_some_and(|ct| ct.pages.iter().any(|(i, pg, _)| *i == id && *pg == ct.cur)));
        if !on_page {
            return;
        }
        let h = ctl(id);
        let mut wr = RECT::default();
        if GetWindowRect(h, &mut wr).is_err() {
            return;
        }
        let mut pts = [POINT { x: wr.left, y: wr.top }, POINT { x: wr.right, y: wr.bottom }];
        MapWindowPoints(None, Some(dlg), &mut pts);
        let scale = GetDpiForWindow(dlg).max(96) as f32 / 96.0;
        let mut rc = RECT::default();
        let _ = GetClientRect(dlg, &mut rc);
        let (top, bottom) = (((PAGE_Y as f32) * scale).round() as i32, rc.bottom - (FOOTER_H as f32 * scale).round() as i32);
        let d = if pts[0].y < top {
            pts[0].y - top
        } else if pts[1].y > bottom {
            pts[1].y - bottom
        } else {
            0
        };
        if d != 0 {
            scroll_by(dlg, (d as f32 / scale).round() as i32);
        }
    }
}

fn pal() -> &'static Pal {
    if DARK.load(Ordering::Relaxed) { &PAL_DARK } else { &PAL_LIGHT }
}

/// 1 行の高さ・行の間・カードの幅（DIP）と、ページの左上（ダイアログ内の座標）
const ROW_H: i32 = 56;
const ROW_GAP: i32 = 6;
const CARD_W: i32 = 496;
const PAGE_X: i32 = 184;
const PAGE_Y: i32 = 56;
/// 下のボタンの行の高さ（DIP）。ページの内容は、この上までに収め、はみ出す分はスクロールする
const FOOTER_H: i32 = 56;
/// マウスホイール 1 ノッチあたりのスクロール量（DIP）
const WHEEL_STEP: i32 = 48;

struct Ctrls {
    /// いまのカードの幅（DIP）
    card_w: i32,
    places: Vec<Place>,
    /// 選択肢のいま選んでいるもの（先頭の ID → 番号）
    sel: HashMap<i32, usize>,
    rows: Vec<Row>,
    all: Vec<(i32, HWND)>,
    /// 分類ごとの部品：(ID, 分類の番号, 検索用の文字列)
    pages: Vec<(i32, usize, String)>,
    /// 一覧に出している分類
    visible: Vec<usize>,
    cur: usize,
    /// ページの内容を、上へどれだけずらしているか（DIP）。ページの内容が表示領域より長いときのスクロール量
    scroll: i32,
    /// 分類ごとの、内容の高さ（DIP。ページの上端から）
    page_h: Vec<i32>,
}

thread_local! {
    static CTRLS: RefCell<Option<Ctrls>> = const { RefCell::new(None) };
}

fn ctl(id: i32) -> HWND {
    CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.all.iter().find(|(i, _)| *i == id).map(|(_, h)| *h)).unwrap_or_default())
}

fn text_of(id: i32) -> String {
    let mut buf = vec![0u16; 512];
    let n = unsafe { GetWindowTextW(ctl(id), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn set_text(id: i32, s: &str) {
    // 同じ内容なら触らない（コンボボックスは入れ直すと全選択になる）
    if text_of(id) == s {
        return;
    }
    let w = wide(s);
    unsafe {
        let _ = SetWindowTextW(ctl(id), PCWSTR(w.as_ptr()));
    }
}

/// チェックボックス（標準の `BS_AUTOCHECKBOX`）の状態は、部品そのものが持つ
/// チェックボックス本体（標準の部品）。`ctl(id)` は、その親の「ホスト」ウィンドウ
fn check_hwnd(id: i32) -> HWND {
    let host = ctl(id);
    if host.0.is_null() {
        return HWND::default();
    }
    unsafe { GetWindow(host, GW_CHILD).unwrap_or_default() }
}

fn set_checked(id: i32, on: bool) {
    let h = check_hwnd(id);
    if !h.0.is_null() {
        unsafe {
            SendMessageW(h, BM_SETCHECK, Some(WPARAM(if on { BST_CHECKED.0 as usize } else { BST_UNCHECKED.0 as usize })), Some(LPARAM(0)));
        }
    }
}

fn is_checked(id: i32) -> bool {
    let h = check_hwnd(id);
    !h.0.is_null() && unsafe { SendMessageW(h, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 } == BST_CHECKED.0 as isize
}

/// チェックボックスのホスト：カードと同じ単色で背景を塗り、中のチェックボックスの色（文字・背景）を決める。
/// クリックなどの通知（WM_COMMAND）は、ダイアログへ渡す。大きさは、中のチェックボックスにそのまま写す
unsafe extern "system" fn check_host_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let card = HBRUSH(CARD_BRUSH[DARK.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) as *mut _);
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(HDC(wparam.0 as *mut _), &rc, card);
                LRESULT(1)
            }
            WM_CTLCOLORBTN | WM_CTLCOLORSTATIC => {
                let hdc = HDC(wparam.0 as *mut _);
                SetTextColor(hdc, rgb_ref(&pal().text));
                SetBkColor(hdc, rgb_ref(&pal().card));
                SetBkMode(hdc, OPAQUE);
                LRESULT(card.0 as isize)
            }
            WM_SIZE => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                if let Ok(child) = GetWindow(hwnd, GW_CHILD) {
                    let _ = SetWindowPos(child, None, 0, 0, rc.right, rc.bottom, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                LRESULT(0)
            }
            WM_COMMAND => SendMessageW(GetParent(hwnd).unwrap_or_default(), WM_COMMAND, Some(wparam), Some(lparam)),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 標準部品ギャラリー用：チェックボックスにサブクラスの描画をつける（色の元は、ギャラリーのブラシと配色）
pub(super) unsafe fn attach_check_subclass(h: HWND, card_brushes: &[AtomicIsize; 2], dark: bool) {
    unsafe {
        DARK.store(dark, Ordering::Relaxed);
        for i in 0..2 {
            CARD_BRUSH[i].store(card_brushes[i].load(Ordering::Relaxed), Ordering::Relaxed);
        }
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(h, Some(check_subclass), 1, 0);
    }
}

/// チェックボックスの描画を自分で受ける（部品は標準のまま。メッセージ・キー操作・読み上げは標準の動き）。
/// 背景はカードと同じ色、箱は OS のテーマ（ダークなら `DarkMode_Explorer`）の本物の絵、題は自前の色で描く。
/// 標準の描画は、親から背景を借りる途中で、位置や色が合わずに帯が出てしまうため
unsafe extern "system" fn check_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        let card = HBRUSH(CARD_BRUSH[DARK.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) as *mut _);
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(HDC(wparam.0 as *mut _), &rc, card);
                LRESULT(1)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(hdc, &rc, card);
                let state = SendMessageW(hwnd, BM_GETSTATE, None, None).0 as u32;
                let checked = SendMessageW(hwnd, BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                let pushed = state & 0x0004 != 0; // BST_PUSHED
                let hot = state & 0x0200 != 0; // BST_HOT
                let disabled = !IsWindowEnabled(hwnd).as_bool();
                // 箱の状態：1 通常・2 ホバー・3 押下・4 無効（チェック済みは +4）
                let st = (if disabled { 4 } else if pushed { 3 } else if hot { 2 } else { 1 }) + if checked { 4 } else { 0 };
                let theme = OpenThemeData(Some(hwnd), w!("Button"));
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                let mut size = SIZE { cx: 13 * dpi / 96, cy: 13 * dpi / 96 };
                if !theme.is_invalid() {
                    if let Ok(sz) = GetThemePartSize(theme, Some(hdc), 3, st, None, TS_DRAW) {
                        size = sz;
                    }
                }
                let top = rc.top + (rc.bottom - rc.top - size.cy) / 2;
                let glyph = RECT { left: rc.left, top, right: rc.left + size.cx, bottom: top + size.cy };
                if !theme.is_invalid() {
                    let _ = DrawThemeBackground(theme, hdc, 3, st, &glyph, None);
                    let _ = CloseThemeData(theme);
                } else {
                    let _ = DrawFrameControl(hdc, &mut { glyph }, DFC_BUTTON, DFCS_BUTTONCHECK | if checked { DFCS_CHECKED } else { DFCS_BUTTONCHECK });
                }
                // 題
                let mut buf = [0u16; 256];
                let n = GetWindowTextW(hwnd, &mut buf) as usize;
                let old = SelectObject(hdc, HGDIOBJ(SendMessageW(hwnd, WM_GETFONT, None, None).0 as *mut _));
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, rgb_ref(&pal().text));
                let mut text = RECT { left: glyph.right + 6 * dpi / 96, top: rc.top, right: rc.right, bottom: rc.bottom };
                DrawTextW(hdc, &mut buf[..n], &mut text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                // キーボードのフォーカス（点線の枠）
                if GetFocus() == hwnd {
                    let mut measure = text;
                    DrawTextW(hdc, &mut buf[..n], &mut measure, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_CALCRECT);
                    let focus = RECT { left: text.left - 2, top: (rc.top + rc.bottom) / 2 - (measure.bottom - measure.top) / 2 - 1, right: (measure.right + 2).min(rc.right), bottom: (rc.top + rc.bottom) / 2 + (measure.bottom - measure.top) / 2 + 1 };
                    let _ = DrawFocusRect(hdc, &focus);
                }
                SelectObject(hdc, old);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            // フォーカスの出入り、ホバー、押下で描き直す
            WM_SETFOCUS | WM_KILLFOCUS | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | BM_SETCHECK => {
                let r = windows::Win32::UI::Shell::DefSubclassProc(hwnd, msg, wparam, lparam);
                let _ = InvalidateRect(Some(hwnd), None, false);
                if msg == WM_MOUSEMOVE {
                    let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut t);
                }
                r
            }
            _ => windows::Win32::UI::Shell::DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}

/// チェックボックスの位置と大きさ（DIP）。カードの上半分（題の行）に、カードの幅いっぱいに置く。
/// 部品は自分の背景をカードと同じ色で塗る（`WM_CTLCOLORBTN`）ので、説明文を覆わないよう、題の行だけの高さにする。
/// 箱と題は部品が描き、説明文はダイアログが、その下に（題の文字に合わせて）描く
fn toggle_rect(y: i32, _h: i32, w: i32) -> (i32, i32, i32, i32) {
    (PAGE_X + 12, PAGE_Y + y + 8, w - 24, 20)
}

/// 説明文を、チェックボックスの題の文字に合わせて右へずらす量（DIP。箱の幅と間）
const TOGGLE_INDENT: i32 = 24;

fn is_toggle(id: i32) -> bool {
    CTRLS.with(|c| c.borrow().as_ref().is_some_and(|ct| ct.rows.iter().any(|r| r.toggle == Some(id))))
}

/// アプリの今の設定を、ダイアログの各部品に写す
fn sync() {
    let Some(s) = with_app(|app| app.settings.clone()) else { return };
    set_checked(ID_LINENUM, s.line_numbers);
    set_checked(ID_WRAP, s.wrap);
    set_checked(ID_WS, s.whitespace);
    set_checked(ID_LOG, s.log_colors);
    set_checked(ID_SYN, s.syntax);
    set_checked(ID_TOOLBAR, s.toolbar);
    set_checked(ID_FKEYS, s.fkeys);
    set_checked(ID_TRAY, s.tray);
    set_checked(ID_MODEBAR, s.modebar);
    set_checked(ID_BACKUP, s.grep_backup);
    set_checked(ID_MDAUTO, s.md_auto);
    set_checked(ID_BKSAVE, s.backup_save);
    set_checked(ID_BKNUM, s.backup_numbered);
    if unsafe { GetFocus() } != ctl(ID_BKDIR) {
        set_text(ID_BKDIR, &s.backup_dir);
    }
    if unsafe { GetFocus() } != ctl(ID_AUTOMIN) {
        set_text(ID_AUTOMIN, &s.autosave_min.to_string());
    }
    if unsafe { GetFocus() } != ctl(ID_WHEEL) {
        set_text(ID_WHEEL, &s.wheel_lines.to_string());
    }
    // 編集中の欄は触らない
    let focus = unsafe { GetFocus() };
    if focus != ctl(ID_FONT) {
        set_text(ID_FONT, &s.font);
    }
    if focus != ctl(ID_SIZE) {
        set_text(ID_SIZE, &format!("{}", s.font_size));
    }
    if focus != ctl(ID_UIFONT) {
        set_text(ID_UIFONT, &s.ui_font);
    }
    if focus != ctl(ID_UISIZE) {
        set_text(ID_UISIZE, &format!("{}", s.ui_font_size));
    }
    if focus != ctl(ID_TAB) {
        set_text(ID_TAB, &s.tab_width.to_string());
    }
    // 選択肢：テーマの選び方、アクセントカラー
    let accent_idx = match s.accent {
        None => Some(0),
        Some(c) => ACCENTS.iter().position(|a| *a == c).map(|i| i + 1),
    };
    CTRLS.with(|c| {
        if let Some(ct) = c.borrow_mut().as_mut() {
            ct.sel.insert(ID_THEME_BASE, s.theme_mode.min(2) as usize);
            match accent_idx {
                Some(i) => ct.sel.insert(ID_ACCENT_BASE, i),
                None => ct.sel.remove(&ID_ACCENT_BASE),
            };
        }
    });
    set_combo_sel(ID_THEME_BASE, Some(s.theme_mode.min(2) as usize));
    set_combo_sel(ID_ACCENT_BASE, accent_idx);
    set_theme(s.dark);
    // 部品に写しただけで、変更ではない（入力欄を書き換えた通知で立った印を戻す）
    DIRTY.store(false, Ordering::Relaxed);
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if !dlg.0.is_null() {
        unsafe {
            let _ = InvalidateRect(Some(dlg), None, true);
        }
    }
}

fn set_theme(dark: bool) {
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if dlg.0.is_null() {
        return;
    }
    let changed = DARK.swap(dark, Ordering::Relaxed) != dark;
    if !changed {
        return;
    }
    unsafe {
        let all: Vec<(i32, HWND)> = CTRLS.with(|c| c.borrow().as_ref().map(|ct| ct.all.clone()).unwrap_or_default());
        for (id, h) in all {
            // チェックボックスは、ホストの子。ドロップダウンは CFD（Explorer だと、ダークでも白くなる。ギャラリーで確認）
            let target = if is_toggle(id) { GetWindow(h, GW_CHILD).unwrap_or(h) } else { h };
            let name = if id == ID_THEME_BASE || id == ID_ACCENT_BASE { w!("DarkMode_CFD") } else { w!("DarkMode_Explorer") };
            let _ = SetWindowTheme(target, if dark { name } else { PCWSTR::null() }, PCWSTR::null());
        }
        let v = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(dlg, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// 分類のページを出す（ほかの分類の部品は隠す）
fn show_page(cat: usize) {
    let pages: Vec<(i32, usize)> = CTRLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(ct) = c.as_mut() else { return Vec::new() };
        ct.cur = cat;
        ct.scroll = 0; // ページを変えたら、先頭から
        ct.pages.iter().map(|(i, p, _)| (*i, *p)).collect()
    });
    unsafe {
        for (id, page) in pages {
            let _ = ShowWindow(ctl(id), if page == cat { SW_SHOW } else { SW_HIDE });
        }
    }
    set_text(ID_TITLE, CATS[cat]);
    let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
    if !dlg.0.is_null() {
        // 先頭に戻した位置に、部品を置き直す（表示領域での切り取りも）
        relayout(dlg);
    }
}

/// 検索語に当てはまる項目を持つ分類だけを、左の一覧に出す
fn fill_cats(query: &str) {
    let q = query.trim().to_lowercase();
    let (visible, cur) = CTRLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(ct) = c.as_mut() else { return (Vec::new(), 0) };
        ct.visible = (0..CATS.len()).filter(|&i| q.is_empty() || CATS[i].to_lowercase().contains(&q) || ct.pages.iter().any(|(_, p, t)| *p == i && t.contains(&q))).collect();
        (ct.visible.clone(), ct.cur)
    });
    let list = ctl(ID_CATS);
    unsafe {
        SendMessageW(list, LB_RESETCONTENT, None, None);
        for &i in &visible {
            let w = wide(CATS[i]);
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(w.as_ptr() as isize)));
        }
        let pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
        if let Some(&cat) = visible.get(pos) {
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(pos)), None);
            show_page(cat);
        }
    }
}

/// 設定の項目 id に対応する、今のアプリの値（表示メニューのコマンドで切り替えるもの）
fn current_check(s: &settings::Settings, id: i32) -> bool {
    match id {
        ID_WRAP => s.wrap,
        ID_WS => s.whitespace,
        ID_LOG => s.log_colors,
        ID_SYN => s.syntax,
        ID_TOOLBAR => s.toolbar,
        ID_MODEBAR => s.modebar,
        ID_FKEYS => s.fkeys,
        ID_BACKUP => s.grep_backup,
        _ => false,
    }
}

/// 下書き（ダイアログの部品の状態）を、アプリと設定ファイルへ反映する。「保存」を押したときだけ呼ぶ
unsafe fn apply_all(owner: HWND) {
    let theme = CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.sel.get(&ID_THEME_BASE).copied())).map(|i| i.min(2) as u8);
    let accent = CTRLS.with(|c| c.borrow().as_ref().map(|ct| ct.sel.get(&ID_ACCENT_BASE).copied()));
    let (line_numbers, tray, bk_save, bk_num, md_auto) = (is_checked(ID_LINENUM), is_checked(ID_TRAY), is_checked(ID_BKSAVE), is_checked(ID_BKNUM), is_checked(ID_MDAUTO));
    let checks: Vec<(i32, bool, u32)> = CHECKS.iter().map(|&(id, _, cmd)| (id, is_checked(id), cmd)).collect();
    let mut deferred = Vec::new();
    with_app(|app| {
        if let Some(m) = theme.filter(|&m| m != app.settings.theme_mode) {
            app.set_theme_mode(m);
        }
        if let Some(a) = accent {
            let c = match a {
                Some(n) if n >= 1 => ACCENTS.get(n - 1).copied(),
                _ => None,
            };
            if c != app.settings.accent {
                app.set_accent(c);
            }
        }
        if line_numbers != app.settings.line_numbers {
            app.settings.line_numbers = line_numbers;
            app.settings.save();
            app.layout_cache.borrow_mut().clear();
            app.invalidate();
        }
        if tray != app.settings.tray {
            app.set_tray(tray);
        }
        if (bk_save, bk_num, md_auto) != (app.settings.backup_save, app.settings.backup_numbered, app.settings.md_auto) {
            app.settings.backup_save = bk_save;
            app.settings.backup_numbered = bk_num;
            app.settings.md_auto = md_auto;
            app.settings.save();
        }
        // 表示メニューと同じ働きのもの：今の値と違うときだけ、コマンドで切り替える
        for (id, on, cmd) in &checks {
            if *on != current_check(&app.settings, *id) {
                if let Some(d) = app.command(*cmd) {
                    deferred.push(d);
                }
            }
        }
    });
    for d in deferred {
        run_deferred(owner, d);
    }
    commit_edits();
    DIRTY.store(false, Ordering::Relaxed);
    with_app(|app| app.message = "設定を保存しました".into());
    unsafe {
        let _ = InvalidateRect(Some(owner), None, false);
        // ダイアログの下に「保存しました」を 2.5 秒だけ出す
        let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
        if !dlg.0.is_null() {
            SAVED.store(true, Ordering::Relaxed);
            SetTimer(Some(dlg), SAVED_TIMER, 2500, None);
            let _ = InvalidateRect(Some(dlg), None, true);
        }
    }
}

/// 保存していない変更があれば、確認する。戻り値は「閉じてよいか」（キャンセルなら false）
unsafe fn confirm_unsaved(dlg: HWND) -> bool {
    unsafe {
        if !DIRTY.load(Ordering::Relaxed) {
            return true;
        }
        let r = MessageBoxW(Some(dlg), w!("変更が保存されていません。保存しますか？"), w!("設定"), MB_YESNOCANCEL | MB_ICONQUESTION);
        if r == IDYES {
            apply_all(GetWindow(dlg, GW_OWNER).unwrap_or_default());
            true
        } else if r == IDNO {
            sync(); // 変更を捨てて、アプリの今の設定に戻す
            true
        } else {
            false
        }
    }
}

/// 下書きを変えた
fn mark_dirty(dlg: HWND) {
    DIRTY.store(true, Ordering::Relaxed);
    unsafe {
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

/// フォント名・大きさ・タブ幅の欄を設定へ反映する
fn commit_edits() {
    let font = text_of(ID_FONT).trim().to_string();
    let size = text_of(ID_SIZE).trim().parse::<f32>().ok();
    let tab = text_of(ID_TAB).trim().parse::<usize>().ok();
    let ui_font = text_of(ID_UIFONT).trim().to_string();
    let ui_size = text_of(ID_UISIZE).trim().parse::<f32>().ok();
    let wheel = text_of(ID_WHEEL).trim().parse::<usize>().ok();
    let bk_dir = text_of(ID_BKDIR).trim().to_string();
    let auto_min = text_of(ID_AUTOMIN).trim().parse::<usize>().ok().map(|n| n.min(1440));
    with_app(|app| {
        if bk_dir != app.settings.backup_dir {
            app.settings.backup_dir = bk_dir.clone();
            app.settings.save();
        }
        if let Some(n) = auto_min.filter(|&n| n != app.settings.autosave_min) {
            app.settings.autosave_min = n;
            app.settings.save();
            app.restart_autosave_timer();
        }
        if let Some(n) = wheel.map(|n| n.clamp(1, 30)).filter(|&n| n != app.settings.wheel_lines) {
            app.settings.wheel_lines = n;
            app.settings.save();
        }
        let s = &app.settings;
        let (fs, tw, us) = (s.font_size, s.tab_width, s.ui_font_size);
        app.set_text_settings(&font, size.unwrap_or(fs), tab.unwrap_or(tw));
        app.set_ui_font(&ui_font, ui_size.unwrap_or(us));
    });
    sync();
}

/// フォント選択のウィンドウ（Windows 標準）を開いて、選んだ名前と大きさを欄に入れる。反映は「保存」のとき
unsafe fn pick_font(dlg: HWND, ui: bool) {
    unsafe {
        let (id_font, id_size) = if ui { (ID_UIFONT, ID_UISIZE) } else { (ID_FONT, ID_SIZE) };
        let name = text_of(id_font);
        let size = text_of(id_size).trim().parse::<f32>().unwrap_or(14.0);
        let mut lf = LOGFONTW { lfHeight: -(size.round() as i32 * 96 / 72), lfCharSet: windows::Win32::Graphics::Gdi::SHIFTJIS_CHARSET, ..Default::default() };
        for (i, c) in name.encode_utf16().take(31).enumerate() {
            lf.lfFaceName[i] = c;
        }
        let mut cf = CHOOSEFONTW {
            lStructSize: std::mem::size_of::<CHOOSEFONTW>() as u32,
            hwndOwner: dlg,
            lpLogFont: &mut lf,
            Flags: CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT,
            ..Default::default()
        };
        if ChooseFontW(&mut cf).as_bool() {
            let n = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
            set_text(id_font, &String::from_utf16_lossy(&lf.lfFaceName[..n]));
            set_text(id_size, &format!("{}", (cf.iPointSize / 10).max(1)));
            mark_dirty(dlg);
        }
    }
}

/// 大きさを `delta` だけ変える（− ＋ ボタン）。反映は「保存」のとき
fn step_size(id_size: i32, delta: f32, min: f32, max: f32) {
    let cur = text_of(id_size).trim().parse::<f32>().unwrap_or(14.0);
    set_text(id_size, &format!("{}", (cur + delta).clamp(min, max)));
}

unsafe fn on_command(dlg: HWND, id: i32, code: u32) {
    unsafe {
        let owner = GetWindow(dlg, GW_OWNER).unwrap_or_default();
        if id == ID_SEARCH {
            if code == EN_CHANGE_CODE {
                fill_cats(&text_of(ID_SEARCH));
            }
            return;
        }
        if id == ID_CATS {
            if code == LBN_SELCHANGE_CODE {
                let sel = SendMessageW(ctl(ID_CATS), LB_GETCURSEL, None, None).0;
                let cat = CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| usize::try_from(sel).ok().and_then(|i| ct.visible.get(i).copied())));
                if let Some(cat) = cat {
                    show_page(cat);
                }
            }
            return;
        }
        // 検索欄で Enter を押しても、ダイアログは閉じない
        if id == IDOK.0 && GetFocus() == ctl(ID_SEARCH) {
            return;
        }
        // 入力欄を書き換えた：下書きを変えただけ（反映は「保存」のとき）
        if matches!(id, ID_FONT | ID_SIZE | ID_TAB | ID_UIFONT | ID_UISIZE | ID_WHEEL | ID_BKDIR | ID_AUTOMIN) {
            if code == EN_CHANGE_CODE {
                mark_dirty(dlg);
            }
            return;
        }
        // ドロップダウン（テーマ・アクセントカラー）：選んだ印を下書きに写す（反映は「保存」のとき）
        if id == ID_THEME_BASE || id == ID_ACCENT_BASE {
            if code == CBN_SELCHANGE {
                let i = SendMessageW(ctl(id), CB_GETCURSEL, None, None).0;
                if let Ok(i) = usize::try_from(i) {
                    select_choice(id, i);
                    mark_dirty(dlg);
                }
            }
            return;
        }
        if code != BN_CLICKED && id != IDOK.0 && id != IDCANCEL.0 {
            return;
        }
        // 保存（Enter も）：反映する。ダイアログは開いたまま
        if id == IDOK.0 {
            apply_all(owner);
            let _ = InvalidateRect(Some(dlg), None, true);
            return;
        }
        // 閉じる・Esc：保存していない変更があれば確認する
        if id == IDCANCEL.0 || id == ID_CLOSE {
            if confirm_unsaved(dlg) {
                let _ = ShowWindow(dlg, SW_HIDE);
            }
            return;
        }
        match id {
            ID_FONT_PICK => return pick_font(dlg, false),
            ID_UIFONT_PICK => return pick_font(dlg, true),
            ID_SIZE_DEC => step_size(ID_SIZE, -1.0, settings::MIN_FONT, settings::MAX_FONT),
            ID_SIZE_INC => step_size(ID_SIZE, 1.0, settings::MIN_FONT, settings::MAX_FONT),
            ID_UISIZE_DEC => step_size(ID_UISIZE, -1.0, settings::MIN_FONT, 40.0),
            ID_UISIZE_INC => step_size(ID_UISIZE, 1.0, settings::MIN_FONT, 40.0),
            ID_BKDIR_PICK => {
                if let Some(d) = pick_folder_titled(dlg, "バックアップを置くフォルダを選んでください") {
                    set_text(ID_BKDIR, &d);
                }
            }
            ID_KEYS => {
                // 別のダイアログを開く操作：保存していない変更があれば、先に確認する
                if !confirm_unsaved(dlg) {
                    return;
                }
                let d = with_app(|app| app.command(menu::ID_KEYS)).flatten();
                if let Some(d) = d {
                    run_deferred(owner, d);
                }
                let _ = ShowWindow(dlg, SW_HIDE);
                return;
            }
            // スイッチ：押した分は WM_COMMAND の側で切り替え済み。ここでは下書きが変わったことだけ覚える
            _ => {}
        }
        mark_dirty(dlg);
    }
}

/// ドロップダウンの選択を部品に写す
fn set_combo_sel(id: i32, idx: Option<usize>) {
    let h = ctl(id);
    if !h.0.is_null() {
        unsafe {
            SendMessageW(h, CB_SETCURSEL, Some(WPARAM(idx.unwrap_or(usize::MAX))), Some(LPARAM(0)));
        }
    }
}

/// 選択肢（テーマ・アクセントカラー）の印を変える
fn select_choice(first: i32, idx: usize) {
    CTRLS.with(|c| {
        if let Some(ct) = c.borrow_mut().as_mut() {
            ct.sel.insert(first, idx);
        }
    });
}

/// ダイアログを（なければ作って）開く
pub(super) unsafe fn show(owner: HWND) {
    unsafe {
        if SET_DLG.load(Ordering::Relaxed) == 0 {
            create(owner);
        }
        let dlg = HWND(SET_DLG.load(Ordering::Relaxed) as *mut _);
        if dlg.0.is_null() {
            return;
        }
        // すでに開いているなら、書きかけの下書きを消さない
        let newly = !IsWindowVisible(dlg).as_bool();
        if newly {
            DARK.store(!with_app(|app| app.settings.dark).unwrap_or(false), Ordering::Relaxed);
            sync();
        }
        if FIRST.swap(false, Ordering::Relaxed) {
            let mut r = RECT::default();
            let _ = GetWindowRect(owner, &mut r);
            let _ = SetWindowPos(dlg, None, r.left + 80, r.top + 60, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
        if newly {
            // 開いた直後は、フォーカスを左の分類の一覧に置く
            let _ = SetFocus(Some(ctl(ID_CATS)));
        }
    }
}

unsafe fn create(owner: HWND) {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(dlg_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        RegisterClassExW(&WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(check_host_proc), hInstance: instance.into(), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), lpszClassName: w!("KazakiriCheckHost"), ..Default::default() });
        // どのビルドか分かるように、題にコミットの短い識別子を付ける（CI のビルドだけ）
        let title = wide(&format!("設定  ({})", option_env!("GITHUB_SHA").map_or("local", |s| s.get(..7).unwrap_or(s))));
        // WS_EX_COMPOSITED：部品も含めて 1 枚に描いてから出す（標準の部品を動かすスクロール・サイズ変更で、ちらつかないように）
        let Ok(dlg) = CreateWindowExW(
            WS_EX_COMPOSITED,
            CLASS,
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            300,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let dpi = GetDpiForWindow(dlg).max(96);
        let scale = dpi as f32 / 96.0;
        // タイトルバーに、アプリのアイコン
        for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let size = GetSystemMetricsForDpi(metric, dpi);
            if let Ok(icon) = LoadImageW(Some(instance.into()), PCWSTR(std::ptr::without_provenance::<u16>(1)), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(dlg, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), 0, dpi).is_ok() {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        };
        // BS_NOTIFY：フォーカスが入ったときに通知をもらう（表示領域の外なら、見える位置までスクロールするため）
        let push = BS_PUSHBUTTON as u32 | BS_NOTIFY as u32 | WS_TABSTOP.0;
        let edit = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
        let mut all: Vec<(i32, HWND)> = Vec::new();
        let mut pages: Vec<(i32, usize, String)> = Vec::new();
        let mut rows: Vec<Row> = Vec::new();
        let mut places: Vec<Place> = Vec::new();
        let p = |v: i32| (v as f32 * scale).round() as i32;
        // page: 分類の番号（usize::MAX は、どの分類でも出す部品）。x, y はダイアログの左上から
        let mut put = |page: usize, id: i32, class: PCWSTR, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, h: i32| {
            let hw = make_child(dlg, class, text, style, ex, id, font);
            let _ = SetWindowPos(hw, None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
            all.push((id, hw));
            if page != usize::MAX {
                pages.push((id, page, text.replace('&', "").to_lowercase()));
            }
        };
        const ALL: usize = usize::MAX;
        // ページごとの、次の行の上端（ページの上端からの位置。DIP）
        let mut cur = [0i32; 9];
        // 1 行（カード）を足す。戻り値は、その行の上端（ダイアログ内の座標）。検索用に、題と説明も覚える
        let card = |rows: &mut Vec<Row>, cur: &mut [i32; 9], page: usize, title: &'static str, desc: &'static str, toggle: Option<i32>, choice: Option<(i32, u8, usize)>| -> i32 {
            let y = cur[page];
            rows.push(Row { page, y, title, desc, toggle, h: ROW_H, preview: false, heading: false, choice });
            cur[page] += ROW_H + ROW_GAP;
            PAGE_Y + y
        };
        // 小見出し
        let heading = |rows: &mut Vec<Row>, cur: &mut [i32; 9], page: usize, title: &'static str| {
            let y = cur[page];
            rows.push(Row { page, y, title, desc: "", toggle: None, h: 26, preview: false, heading: true, choice: None });
            cur[page] += 32;
        };
        // 入力欄の縦の位置（カードの中で縦中央）
        let mid = |top: i32, h: i32| top + (ROW_H - h) / 2;
        let right = PAGE_X + CARD_W - 16;
        // カードの右端までの距離（右に寄せる部品の置き方）
        let cr = |x: i32| Anchor::CardRight(PAGE_X + CARD_W - x);
        let flat = WS_BORDER.0;
        put(ALL, ID_SEARCH, w!("EDIT"), "", edit | flat, 0, 12, 14, 156, 26);
        put(ALL, ID_CATS, w!("LISTBOX"), "", (LBS_NOTIFY as u32) | (LBS_OWNERDRAWFIXED as u32) | (LBS_HASSTRINGS as u32) | WS_TABSTOP.0, 0, 12, 52, 164, 420);
        places.push(Place { id: ID_CATS, anchor: Anchor::ListHeight, x: 12, y: 52, w: 164, h: 420 });
        put(ALL, ID_TITLE, w!("STATIC"), "", 0, 0, PAGE_X, 12, 480, 34);
        // 基本
        card(&mut rows, &mut cur, 0, "長い行を折り返して表示", "画面の幅で折り返す（文字単位）", Some(ID_WRAP), None);
        card(&mut rows, &mut cur, 0, "行番号", "本文の左の行番号の欄を出す", Some(ID_LINENUM), None);
        let y = card(&mut rows, &mut cur, 0, "タブの幅", "半角の文字数", None, None);
        put(0, ID_TAB, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_TAB, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        // スクロール
        let y = card(&mut rows, &mut cur, 1, "マウスホイール", "1 回で動かす行数", None, None);
        put(1, ID_WHEEL, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_WHEEL, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        // 外観
        heading(&mut rows, &mut cur, 2, "テーマ");
        card(&mut rows, &mut cur, 2, "アプリのテーマ", "Windows の設定に合わせる、または固定する", None, Some((ID_THEME_BASE, 0, 3)));
        card(&mut rows, &mut cur, 2, "アクセントカラー", "選択・ボタン・フォーカスに使う色。左端は Windows の設定の色", None, Some((ID_ACCENT_BASE, 1, 1 + ACCENTS.len())));
        heading(&mut rows, &mut cur, 2, "フォント");
        // フォント名の欄（書くか、「選択...」でフォント選択のウィンドウから選ぶ）と、大きさの − [数] ＋
        for (title, desc, id_font, id_pick, id_dec, id_size, id_inc) in [
            ("本文のフォント", "空欄で自動。等幅のフォントがおすすめ", ID_FONT, ID_FONT_PICK, ID_SIZE_DEC, ID_SIZE, ID_SIZE_INC),
            ("画面のフォント", "メニュー・タブなど。空欄で自動", ID_UIFONT, ID_UIFONT_PICK, ID_UISIZE_DEC, ID_UISIZE, ID_UISIZE_INC),
        ] {
            let y = card(&mut rows, &mut cur, 2, title, desc, None, None);
            let (xi, xs, xd) = (right - 28, right - 28 - 4 - 48, right - 28 - 4 - 48 - 4 - 28);
            let xp = xd - 8 - 72;
            let xf = xp - 8 - 130;
            put(2, id_inc, w!("BUTTON"), "＋", push, 0, xi, mid(y, 28), 28, 28);
            put(2, id_size, w!("EDIT"), "", edit | ES_NUMBER as u32 | ES_CENTER as u32 | flat, 0, xs, mid(y, 26), 48, 26);
            put(2, id_dec, w!("BUTTON"), "－", push, 0, xd, mid(y, 28), 28, 28);
            put(2, id_pick, w!("BUTTON"), "選択...", push, 0, xp, mid(y, 28), 72, 28);
            put(2, id_font, w!("EDIT"), "", edit | flat, 0, xf, mid(y, 26), 130, 26);
            places.push(Place { id: id_inc, anchor: cr(xi), x: xi, y: mid(y, 28), w: 28, h: 28 });
            places.push(Place { id: id_size, anchor: cr(xs), x: xs, y: mid(y, 26), w: 48, h: 26 });
            places.push(Place { id: id_dec, anchor: cr(xd), x: xd, y: mid(y, 28), w: 28, h: 28 });
            places.push(Place { id: id_pick, anchor: cr(xp), x: xp, y: mid(y, 28), w: 72, h: 28 });
            places.push(Place { id: id_font, anchor: cr(xf), x: xf, y: mid(y, 26), w: 130, h: 26 });
        }
        // フォントの見え方（設定を変えると、すぐ変わる）
        let py = cur[2];
        rows.push(Row { page: 2, y: py, title: "プレビュー", desc: "", toggle: None, h: 92, preview: true, heading: false, choice: None });
        cur[2] += 98;
        // 表示
        heading(&mut rows, &mut cur, 3, "本文");
        card(&mut rows, &mut cur, 3, "空白・タブ・改行を表示", "印で見えるようにする（Ctrl+Shift+W）", Some(ID_WS), None);
        card(&mut rows, &mut cur, 3, "ログの色分け", "ERROR / WARN などに色を付ける", Some(ID_LOG), None);
        card(&mut rows, &mut cur, 3, "構文の色分け", "JSON・SQL・XML など", Some(ID_SYN), None);
        heading(&mut rows, &mut cur, 3, "画面の部品");
        card(&mut rows, &mut cur, 3, "ツールバー", "画面の上のボタンの行", Some(ID_TOOLBAR), None);
        card(&mut rows, &mut cur, 3, "モードバー", "CSV・Markdown のときの操作の行", Some(ID_MODEBAR), None);
        card(&mut rows, &mut cur, 3, "機能ボタンの列", "ステータスバーの上の「検索 Ctrl+F …」", Some(ID_FKEYS), None);
        card(&mut rows, &mut cur, 3, "通知領域のアイコン", "クリックで新規作成・開くなど", Some(ID_TRAY), None);
        // 検索・GREP
        card(&mut rows, &mut cur, 4, "GREP 置換の前にバックアップ", ".bak を作る", Some(ID_BACKUP), None);
        // バックアップ
        card(&mut rows, &mut cur, 5, "上書き保存の前にバックアップ", "元のファイルを残す", Some(ID_BKSAVE), None);
        let y = card(&mut rows, &mut cur, 5, "バックアップを置くフォルダ", "空欄で、元のファイルと同じ", None, None);
        put(5, ID_BKDIR, w!("EDIT"), "", edit | flat, 0, right - 80 - 8 - 190, mid(y, 26), 190, 26);
        put(5, ID_BKDIR_PICK, w!("BUTTON"), "参照...", push, 0, right - 80, mid(y, 28), 80, 28);
        places.push(Place { id: ID_BKDIR, anchor: cr(right - 80 - 8 - 190), x: right - 80 - 8 - 190, y: mid(y, 26), w: 190, h: 26 });
        places.push(Place { id: ID_BKDIR_PICK, anchor: cr(right - 80), x: right - 80, y: mid(y, 28), w: 80, h: 28 });
        card(&mut rows, &mut cur, 5, "古いバックアップを残す", ".bak2、.bak3 … と番号を増やす（オフなら .bak を上書き）", Some(ID_BKNUM), None);
        put(5, 41, w!("STATIC"), "バックアップは、名前を付けて保存・変換して保存を含む、既存のファイルを書き換える保存の直前に作ります。", 0, 0, PAGE_X, PAGE_Y + cur[5] + 4, CARD_W, 40);
        places.push(Place { id: 41, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[5] + 4, w: CARD_W, h: 40 });
        // 自動保存
        let y = card(&mut rows, &mut cur, 6, "自動保存の間隔", "分。0 で自動保存しない", None, None);
        put(6, ID_AUTOMIN, w!("EDIT"), "", edit | ES_NUMBER as u32 | flat, 0, right - 60, mid(y, 26), 60, 26);
        places.push(Place { id: ID_AUTOMIN, anchor: cr(right - 60), x: right - 60, y: mid(y, 26), w: 60, h: 26 });
        put(6, 43, w!("STATIC"), "未保存の編集を、『名前.autosave』に書き出します（元のファイルは書き換えません）。保存するか、通常どおり閉じると消え、異常終了したときだけ残ります。バックアップのフォルダを指定していれば、そこに書き出します。", 0, 0, PAGE_X, PAGE_Y + cur[6] + 4, CARD_W, 80);
        places.push(Place { id: 43, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[6] + 4, w: CARD_W, h: 80 });
        // Markdown
        card(&mut rows, &mut cur, 7, "開いたときにプレビューを並べる", "Markdown ファイル（.md）を開いたとき、右に並べる", Some(ID_MDAUTO), None);
        put(7, 16, w!("STATIC"), "プレビューは、表示メニューやモードバーのボタン（Ctrl+Shift+V：右に並べる、Ctrl+Shift+G：デザインビュー）でも切り替えられます。", 0, 0, PAGE_X, PAGE_Y + cur[7] + 4, CARD_W, 40);
        places.push(Place { id: 16, anchor: Anchor::CardWidth, x: PAGE_X, y: PAGE_Y + cur[7] + 4, w: CARD_W, h: 40 });
        // キーボード
        let y = card(&mut rows, &mut cur, 8, "キー割り当て", "コマンドごとのキーを、一覧から選んで変える", None, None);
        put(8, ID_KEYS, w!("BUTTON"), "変更...", push, 0, right - 90, mid(y, 28), 90, 28);
        places.push(Place { id: ID_KEYS, anchor: cr(right - 90), x: right - 90, y: mid(y, 28), w: 90, h: 28 });
        // ページごとの内容の高さ（末尾の説明文の分と、下の余白を含む）。表示領域より長ければ、スクロールする
        let notes = [0, 0, 0, 0, 0, 44, 84, 44, 0];
        let page_h: Vec<i32> = (0..cur.len()).map(|i| cur[i] + notes[i] + 8).collect();
        put(ALL, IDOK.0, w!("BUTTON"), "保存", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0, 0, 520, 520, 90, 30);
        places.push(Place { id: IDOK.0, anchor: Anchor::Corner(200, 46), x: 520, y: 520, w: 90, h: 30 });
        put(ALL, ID_CLOSE, w!("BUTTON"), "閉じる", push, 0, 620, 520, 90, 30);
        places.push(Place { id: ID_CLOSE, anchor: Anchor::Corner(100, 46), x: 620, y: 520, w: 90, h: 30 });
        // 本文より大きな見出しのフォント
        let mut big = font_lf(font);
        big.lfHeight = (big.lfHeight as f32 * 2.0) as i32;
        big.lfWeight = 600;
        big.lfFaceName = [0; 32];
        for (i, c) in "Yu Gothic UI".encode_utf16().enumerate() {
            big.lfFaceName[i] = c;
        }
        let big_font = CreateFontIndirectW(&big);
        if let Some(&(_, h)) = all.iter().find(|(i, _)| *i == ID_TITLE) {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(big_font.0 as usize)), Some(LPARAM(1)));
        }
        // スイッチの行は標準のチェックボックス、選択肢ごとにはカードの上に透明なボタンを重ねる（Tab で止まり、Space で押せる）
        for r in &rows {
            if let Some(id) = r.toggle {
                // チェックボックスは、単色で背景を塗る「ホスト」の子にする（標準部品が親の背景を借りても、色と位置が必ず合う）
                let hw = make_child(dlg, w!("KazakiriCheckHost"), "", WS_CLIPCHILDREN.0, WS_EX_CONTROLPARENT.0, id + 10000, font);
                let _ = make_child(hw, w!("BUTTON"), r.title, BS_AUTOCHECKBOX as u32 | BS_LEFT as u32 | WS_TABSTOP.0, 0, id, font);
                let (x, y, w, h) = toggle_rect(r.y, r.h, CARD_W);
                let _ = SetWindowPos(hw, None, p(x), p(y), p(w), p(h), SWP_NOZORDER | SWP_NOACTIVATE);
                all.push((id, hw));
                pages.push((id, r.page, String::new()));
            }
            if let Some((first, kind, n)) = r.choice {
                let _ = n;
                // ドロップダウン（標準のコンボボックス）。アクセントカラーは、色の丸を出すため、項目だけ自前で描く
                let style = CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0 | if kind == 1 { CBS_OWNERDRAWFIXED as u32 | CBS_HASSTRINGS as u32 } else { 0 };
                let hw = make_child(dlg, w!("COMBOBOX"), "", style, 0, first, font);
                let (cx, cy, cw, ch) = combo_rect(r.y, r.h, CARD_W);
                let _ = SetWindowPos(hw, None, p(cx), p(cy), p(cw), p(ch), SWP_NOZORDER | SWP_NOACTIVATE);
                let names: &[&str] = if kind == 0 { &THEME_NAMES } else { &ACCENT_NAMES };
                for name in names {
                    let w = wide(name);
                    SendMessageW(hw, CB_ADDSTRING, None, Some(LPARAM(w.as_ptr() as isize)));
                }
                all.push((first, hw));
                pages.push((first, r.page, String::new()));
            }
        }
        // 検索欄の薄い案内文
        if let Some(&(_, h)) = all.iter().find(|(i, _)| *i == ID_SEARCH) {
            let cue = wide("設定を検索");
            SendMessageW(h, 0x1501, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
        }
        UI_FONT.store(font.0 as isize, Ordering::Relaxed);
        // 検索で見つかるように、各行の題と説明も、その分類の文字列として覚える
        for r in &rows {
            pages.push((0, r.page, format!("{} {}", r.title, r.desc).to_lowercase()));
        }
        for (i, pal) in [&PAL_LIGHT, &PAL_DARK].into_iter().enumerate() {
            let edit_bg = if i == 1 { rgb(0x1f, 0x1f, 0x1f) } else { rgb(0xff, 0xff, 0xff) };
            BG_BRUSH[i].store(CreateSolidBrush(rgb_ref(&pal.bg)).0 as isize, Ordering::Relaxed);
            EDIT_BRUSH[i].store(CreateSolidBrush(rgb_ref(&edit_bg)).0 as isize, Ordering::Relaxed);
            CARD_BRUSH[i].store(CreateSolidBrush(rgb_ref(&pal.card)).0 as isize, Ordering::Relaxed);
        }
        CTRLS.with(|c| *c.borrow_mut() = Some(Ctrls { card_w: CARD_W, places, sel: HashMap::new(), rows, all, pages, visible: Vec::new(), cur: 0, scroll: 0, page_h }));
        fill_cats("");
        SET_DLG.store(dlg.0 as isize, Ordering::Relaxed);
        // キーボードのフォーカス枠は、マウスで押したときには出さない（Tab などを使うと、IsDialogMessage が出す状態に変える）
        // WM_CHANGEUISTATE = 0x127、wparam = UIS_SET (1) | UISF_HIDEFOCUS (1) << 16
        SendMessageW(dlg, 0x0127, Some(WPARAM(1 | (1 << 16))), None);
        SendMessageW(ctl(ID_CATS), LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(p(40) as isize)));
        // クライアント領域を 720 × 566 に
        let style = WINDOW_STYLE(GetWindowLongW(dlg, GWL_STYLE) as u32);
        let mut rc = RECT { left: 0, top: 0, right: p(720), bottom: p(566) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let _ = SetWindowPos(dlg, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// 設定ダイアログのフォントの LOGFONT
unsafe fn font_lf(font: HFONT) -> LOGFONTW {
    let mut lf = LOGFONTW::default();
    unsafe {
        GetObjectW(font.into(), std::mem::size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
    }
    lf
}

/// フォントの見え方（本文のフォントと大きさで、構文・ログの色の例を描く）
unsafe fn draw_preview(hdc: HDC, x: i32, y: i32, h: i32, scale: f32, pal: &Pal) {
    // 大きさをドラッグで変えているあいだは、描かない（終わったら描き直す）
    if SIZING.load(Ordering::Relaxed) {
        return;
    }
    // 保存前の下書き（入力欄）で描く。読めないときだけ、アプリの今の設定
    let Some(size) = with_app(|a| a.settings.font_size) else { return };
    let face = text_of(ID_FONT).trim().to_string();
    let size = text_of(ID_SIZE).trim().parse::<f32>().ok().map_or(size, |v| v.clamp(settings::MIN_FONT, settings::MAX_FONT));
    let face = if face.is_empty() { "BIZ UDGothic".to_string() } else { face };
    let t = if DARK.load(Ordering::Relaxed) { &super::theme::DARK } else { &super::theme::LIGHT };
    unsafe {
        let wface = wide(&face);
        let mut name = [0u16; 32];
        for (i, c) in wface.iter().take(31).enumerate() {
            name[i] = *c;
        }
        let lf = LOGFONTW { lfHeight: -((size * scale).round() as i32), lfCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET, lfFaceName: name, ..Default::default() };
        // フォントは、名前と大きさが同じあいだは使い回す（描くたびに作ると、サイズ変更中に重い）
        thread_local! {
            static PREVIEW_FONT: RefCell<Option<((String, i32), HFONT)>> = const { RefCell::new(None) };
        }
        let key = (face.clone(), lf.lfHeight);
        let font = PREVIEW_FONT.with(|c| {
            let mut c = c.borrow_mut();
            if let Some((k, f)) = c.as_ref() {
                if *k == key {
                    return *f;
                }
            }
            if let Some((_, old)) = c.take() {
                let _ = DeleteObject(old.into());
            }
            let f = CreateFontIndirectW(&lf);
            *c = Some((key, f));
            f
        });
        let old = SelectObject(hdc, font.into());
        SetBkMode(hdc, TRANSPARENT);
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(hdc, &mut tm);
        let lh = tm.tmHeight.max(12) + 2;
        // 見出し（小さく）
        let y0 = y + (h - lh * 2) / 2;
        let runs: [&[(&str, &D2D1_COLOR_F)]; 2] = [
            &[("SELECT", &t.syn_kw), (" id, name ", &t.text), ("FROM", &t.syn_kw), (" users ", &t.text), ("WHERE", &t.syn_kw), (" status = ", &t.text), ("'active'", &t.syn_str), (";", &t.text)],
            &[("ERROR", &t.log_error), (" 接続できません　", &t.text), ("WARN", &t.log_warn), (" 再試行 1/3　", &t.text), ("12345", &t.syn_num)],
        ];
        let _ = pal;
        for (row, line) in runs.iter().enumerate() {
            let mut cx = x;
            for (text, color) in line.iter() {
                SetTextColor(hdc, rgb_ref(color));
                let mut u: Vec<u16> = text.encode_utf16().collect();
                let mut rc = RECT { left: cx, top: y0 + row as i32 * lh, right: cx + 2000, bottom: y0 + row as i32 * lh + lh };
                DrawTextW(hdc, &mut u, &mut rc, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP);
                let mut sz = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &u, &mut sz);
                cx += sz.cx;
            }
        }
        SelectObject(hdc, old);
    }
}

/// アクセントカラーのドロップダウンの 1 項目（色の丸と名前）。部品は標準のコンボボックスで、項目の中身だけ描く
unsafe fn draw_accent_item(di: &DRAWITEMSTRUCT) {
    unsafe {
        let p = pal();
        let dark = DARK.load(Ordering::Relaxed);
        let selected = di.itemState.0 & ODS_SELECTED.0 != 0 && di.itemState.0 & ODS_COMBOBOXEDIT.0 == 0;
        let bg = if selected { if dark { rgb(0x3a, 0x3a, 0x3a) } else { rgb(0xe0, 0xe0, 0xe0) } } else if dark { rgb(0x1f, 0x1f, 0x1f) } else { rgb(0xff, 0xff, 0xff) };
        let b = CreateSolidBrush(rgb_ref(&bg));
        FillRect(di.hDC, &di.rcItem, b);
        let _ = DeleteObject(b.into());
        let Ok(i) = usize::try_from(di.itemID as i32) else { return };
        if i >= ACCENT_NAMES.len() {
            return;
        }
        let h = di.rcItem.bottom - di.rcItem.top;
        let r = h / 4;
        let (cx, cy) = (di.rcItem.left + h / 2 + 2, di.rcItem.top + h / 2);
        let c = CreateSolidBrush(rgb_ref(&swatch_color(i)));
        let old = SelectObject(di.hDC, c.into());
        let _ = Ellipse(di.hDC, cx - r, cy - r, cx + r, cy + r);
        SelectObject(di.hDC, old);
        let _ = DeleteObject(c.into());
        SetBkMode(di.hDC, TRANSPARENT);
        SetTextColor(di.hDC, rgb_ref(&p.text));
        let mut t: Vec<u16> = ACCENT_NAMES[i].encode_utf16().collect();
        let mut rc = RECT { left: di.rcItem.left + h + 4, top: di.rcItem.top, right: di.rcItem.right, bottom: di.rcItem.bottom };
        DrawTextW(di.hDC, &mut t, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
}

/// 左の分類の 1 行：アイコン・名前、選んでいる行は左にアクセントカラーの帯
unsafe fn draw_cat_item(di: &DRAWITEMSTRUCT) {
    const ICONS: [&str; 9] = ["\u{E713}", "\u{E8CB}", "\u{E790}", "\u{E890}", "\u{E721}", "\u{E74E}", "\u{E823}", "\u{E8A5}", "\u{E765}"];
    let p = pal();
    let hdc = di.hDC;
    let rc = di.rcItem;
    let Some(cat) = CTRLS.with(|c| c.borrow().as_ref().and_then(|ct| ct.visible.get(di.itemID as usize).copied())) else {
        unsafe {
            let b = CreateSolidBrush(rgb_ref(&p.bg));
            FillRect(hdc, &rc, b);
            let _ = DeleteObject(b.into());
        }
        return;
    };
    let scale = unsafe { GetDpiForWindow(di.hwndItem) }.max(96) as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let selected = di.itemState.0 & ODS_SELECTED.0 != 0;
    unsafe {
        let bg = CreateSolidBrush(rgb_ref(&p.bg));
        FillRect(hdc, &rc, bg);
        let _ = DeleteObject(bg.into());
        if selected {
            let sel = CreateSolidBrush(rgb_ref(&p.cardb));
            let pen = GetStockObject(NULL_PEN);
            let (ob, op) = (SelectObject(hdc, sel.into()), SelectObject(hdc, pen));
            let _ = RoundRect(hdc, rc.left + px(2.0), rc.top + px(2.0), rc.right - px(2.0), rc.bottom - px(2.0), px(8.0), px(8.0));
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(sel.into());
            let acc = CreateSolidBrush(rgb_ref(&accent_now()));
            let bar = RECT { left: rc.left + px(2.0), top: rc.top + px(11.0), right: rc.left + px(5.0), bottom: rc.bottom - px(11.0) };
            FillRect(hdc, &bar, acc);
            let _ = DeleteObject(acc.into());
        }
        SetBkMode(hdc, TRANSPARENT);
        // アイコン（Segoe Fluent Icons。なければ Segoe MDL2 Assets）
        let mut name = [0u16; 32];
        for (i, c) in "Segoe Fluent Icons".encode_utf16().enumerate() {
            name[i] = c;
        }
        let lf = LOGFONTW { lfHeight: -px(16.0), lfCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET, lfFaceName: name, ..Default::default() };
        let icon_font = CreateFontIndirectW(&lf);
        let old = SelectObject(hdc, icon_font.into());
        let acc = accent_now();
        SetTextColor(hdc, rgb_ref(if selected { &acc } else { &p.dim }));
        let mut g: Vec<u16> = ICONS[cat.min(8)].encode_utf16().collect();
        let mut r = RECT { left: rc.left + px(16.0), top: rc.top, right: rc.left + px(36.0), bottom: rc.bottom };
        DrawTextW(hdc, &mut g, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
        let _ = DeleteObject(icon_font.into());
        // 名前
        let font = HFONT(UI_FONT.load(Ordering::Relaxed) as *mut _);
        let old = SelectObject(hdc, font.into());
        SetTextColor(hdc, rgb_ref(&p.text));
        let mut t: Vec<u16> = CATS[cat].encode_utf16().collect();
        let mut r = RECT { left: rc.left + px(44.0), top: rc.top, right: rc.right - px(8.0), bottom: rc.bottom };
        DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(hdc, old);
    }
}

/// 描く 1 行：(スクロール後の y, 題, 説明, スイッチの ID, 高さ, プレビューか, 小見出しか, 選択肢)
type PaintRow = (i32, &'static str, &'static str, Option<i32>, i32, bool, bool, Option<(i32, u8, usize)>);

/// 2 色を混ぜる（t = 0 で a、1 で b）
fn mix(a: &D2D1_COLOR_F, b: &D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
}

/// 大きさをドラッグで変えている最中の、簡易な背景（GDI。縁は滑らかでないが、Direct2D のように描画結果を読み戻さないので速い）
#[allow(clippy::too_many_arguments)]
unsafe fn gdi_cards(hdc: HDC, rc: &RECT, scale: f32, rows: &[PaintRow], card_w: i32, scroll: i32, content: i32, pal: &Pal) {
    unsafe {
        let px = |v: i32| (v as f32 * scale).round() as i32;
        let bg = CreateSolidBrush(rgb_ref(&pal.bg));
        FillRect(hdc, rc, bg);
        let _ = DeleteObject(bg.into());
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, 0, px(PAGE_Y), rc.right, rc.bottom - px(FOOTER_H));
        let null = GetStockObject(NULL_BRUSH);
        let shape = |hdc: HDC, fill: Option<&D2D1_COLOR_F>, line: Option<&D2D1_COLOR_F>, f: &dyn Fn(HDC)| {
            let b = fill.map(|c| CreateSolidBrush(rgb_ref(c)));
            let pen = match line {
                Some(c) => CreatePen(PS_SOLID, 1, rgb_ref(c)),
                None => CreatePen(PS_NULL, 0, COLORREF(0)),
            };
            let (ob, op) = (SelectObject(hdc, b.map_or(null, |b| b.into())), SelectObject(hdc, pen.into()));
            f(hdc);
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(pen.into());
            if let Some(b) = b {
                let _ = DeleteObject(b.into());
            }
        };
        for &(y, _, _, _, rh, _, heading, _) in rows {
            if heading {
                continue;
            }
            let (x0, y0) = (px(PAGE_X), px(PAGE_Y + y));
            let (x1, y1) = (x0 + px(card_w), y0 + px(rh));
            let d = px(16);
            shape(hdc, Some(&pal.card), Some(&pal.cardb), &|h| {
                let _ = RoundRect(h, x0, y0, x1, y1, d, d);
            });
        }
        let _ = RestoreDC(hdc, saved);
        // スクロール位置の目印
        let (view_top, view_bottom) = (px(PAGE_Y), rc.bottom - px(FOOTER_H));
        let (vh, content_px) = (view_bottom - view_top, px(content));
        if content_px > vh && vh > 0 {
            let thumb = (vh * vh / content_px).max(px(24)).min(vh);
            let ty = view_top + (px(scroll) as f32 / (content_px - vh) as f32).clamp(0.0, 1.0).mul_add((vh - thumb) as f32, 0.0) as i32;
            let x1 = rc.right - px(6);
            shape(hdc, Some(&mix(&pal.bg, &pal.dim, 0.5)), None, &|h| {
                let _ = RoundRect(h, x1 - px(4), ty, x1, ty + thumb, px(4), px(4));
            });
        }
    }
}

/// カード・スイッチ・文字を、背景として描く（子ウィンドウの部品は、この上に重なる）
unsafe fn paint_bg(hwnd: HWND, hdc: HDC) {
    thread_local! {
        static FACTORY: RefCell<Option<ID2D1Factory>> = const { RefCell::new(None) };
        /// 描画のたびに作り直さない（サイズ変更中に何度も描くため）。失敗したら捨てて作り直す
        static DC_RT: RefCell<Option<windows::Win32::Graphics::Direct2D::ID2D1DCRenderTarget>> = const { RefCell::new(None) };
    }
    unsafe {
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let pal = pal();
        // 行の位置は、スクロール量だけ上へずらす
        let (rows, card_w, scroll, content) = CTRLS.with(|c| {
            c.borrow().as_ref().map_or((Vec::new(), CARD_W, 0, 0), |ct| {
                (
                    ct.rows.iter().filter(|r| r.page == ct.cur).map(|r| (r.y - ct.scroll, r.title, r.desc, r.toggle, r.h, r.preview, r.heading, r.choice)).collect::<Vec<_>>(),
                    ct.card_w,
                    ct.scroll,
                    ct.page_h.get(ct.cur).copied().unwrap_or(0),
                )
            })
        });
        let acc = accent_now();
        let factory = FACTORY.with(|f| {
            let mut f = f.borrow_mut();
            if f.is_none() {
                *f = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok();
            }
            f.clone()
        });
        let px = |v: i32| v as f32 * scale;
        let mut drawn = false;
        if SIZING.load(Ordering::Relaxed) {
            gdi_cards(hdc, &rc, scale, &rows, card_w, scroll, content, pal);
            drawn = true;
        } else if let Some(factory) = factory {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let cached = DC_RT.with(|r| {
                let mut r = r.borrow_mut();
                if r.is_none() {
                    *r = factory.CreateDCRenderTarget(&props).ok();
                }
                r.clone()
            });
            if let Some(rt) = cached {
                if rt.BindDC(hdc, &rc).is_ok() {
                    rt.BeginDraw();
                    // 標準の部品が親の背景を借りに来たときは、DC の原点が子の位置にずれている。その分だけずらして描く（カードの位置を合わせる）
                    let mut org = POINT::default();
                    let _ = GetViewportOrgEx(hdc, &mut org);
                    rt.SetTransform(&Matrix3x2::translation(org.x as f32, org.y as f32));
                    rt.Clear(Some(&pal.bg));
                    if let Ok(brush) = rt.CreateSolidColorBrush(&pal.card, None) {
                        // 行（カード）は、表示領域（タイトルの下から、ボタンの行の上まで）の中だけに描く
                        rt.PushAxisAlignedClip(&windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: 0.0, top: px(PAGE_Y), right: rc.right as f32, bottom: rc.bottom as f32 - px(FOOTER_H) }, D2D1_ANTIALIAS_MODE_ALIASED);
                        for &(y, _, _, _, rh, _, heading, _) in &rows {
                            if heading {
                                continue;
                            }
                            let (x0, y0) = (px(PAGE_X), px(PAGE_Y + y));
                            let (x1, y1) = (x0 + px(card_w), y0 + px(rh));
                            brush.SetColor(&pal.cardb);
                            rt.FillRoundedRectangle(&rrect(x0, y0, x1, y1, px(8)), &brush);
                            brush.SetColor(&pal.card);
                            rt.FillRoundedRectangle(&rrect(x0 + 1.0, y0 + 1.0, x1 - 1.0, y1 - 1.0, px(8) - 1.0), &brush);
                        }
                        rt.PopAxisAlignedClip();
                        // ページが表示領域より長いときだけ、右端にスクロール位置の目印（細い線）を出す
                        let (view_top, view_bottom) = (px(PAGE_Y), rc.bottom as f32 - px(FOOTER_H));
                        let (vh, content_px) = (view_bottom - view_top, px(content));
                        if content_px > vh && vh > 0.0 {
                            let thumb = (vh * vh / content_px).max(px(24)).min(vh);
                            let ty = view_top + (px(scroll) / (content_px - vh)).clamp(0.0, 1.0) * (vh - thumb);
                            let x1 = rc.right as f32 - px(6);
                            brush.SetColor(&D2D1_COLOR_F { a: 0.5, ..pal.dim });
                            rt.FillRoundedRectangle(&rrect(x1 - px(4), ty, x1, ty + thumb, px(2)), &brush);
                        }
                    }
                    drawn = rt.EndDraw(None, None).is_ok();
                    if !drawn {
                        DC_RT.with(|r| *r.borrow_mut() = None);
                    }
                }
            }
        }
        if !drawn {
            FillRect(hdc, &rc, HBRUSH(BG_BRUSH[DARK.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) as *mut _));
        }
        // 題と説明（GDI の文字）
        let font = HFONT(UI_FONT.load(Ordering::Relaxed) as *mut _);
        let old = SelectObject(hdc, font.into());
        SetBkMode(hdc, TRANSPARENT);
        let lh = {
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(hdc, &mut tm);
            tm.tmHeight.max(12)
        };
        // 行の文字も、表示領域の中だけに描く
        let saved_dc = SaveDC(hdc);
        IntersectClipRect(hdc, 0, px(PAGE_Y) as i32, rc.right, rc.bottom - px(FOOTER_H) as i32);
        for &(y, title, desc, toggle, rh, preview, heading, _) in &rows {
            if heading {
                SetTextColor(hdc, rgb_ref(&pal.dim));
                let mut t: Vec<u16> = title.encode_utf16().collect();
                let mut r = RECT { left: px(PAGE_X + 4) as i32, top: px(PAGE_Y + y) as i32 + px(4) as i32, right: px(PAGE_X + card_w) as i32, bottom: px(PAGE_Y + y + 26) as i32 };
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX);
                continue;
            }
            if preview {
                draw_preview(hdc, px(PAGE_X + 16) as i32, px(PAGE_Y + y) as i32, px(rh) as i32, scale, pal);
                continue;
            }
            let x0 = px(PAGE_X + 16) as i32;
            let y0 = px(PAGE_Y + y) as i32;
            let (ty, dy) = if desc.is_empty() { (y0 + (px(ROW_H) as i32 - lh) / 2, 0) } else { (y0 + px(10) as i32, y0 + px(10) as i32 + lh + px(2) as i32) };
            // チェックボックスの行は、題を部品が描く（ここでは説明だけ、題の文字の位置に合わせて描く）。入力欄のある行は入力欄の手前まで
            let limit = if toggle.is_some() { px(PAGE_X + card_w - 16) as i32 } else { x0 + px(card_w - 32 - 330) as i32 };
            if toggle.is_none() {
                SetTextColor(hdc, rgb_ref(&pal.text));
                let mut t: Vec<u16> = title.encode_utf16().collect();
                let mut r = RECT { left: x0, top: ty, right: limit.max(x0 + 40), bottom: ty + lh };
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            }
            if !desc.is_empty() {
                SetTextColor(hdc, rgb_ref(&pal.dim));
                let mut d: Vec<u16> = desc.encode_utf16().collect();
                let dx = if toggle.is_some() { x0 + px(TOGGLE_INDENT - 4) as i32 } else { x0 };
                let mut r = RECT { left: dx, top: dy, right: limit.max(dx + 40), bottom: dy + lh };
                DrawTextW(hdc, &mut d, &mut r, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            }
        }
        let _ = RestoreDC(hdc, saved_dc);
        // 保存した直後だけ、保存ボタンの左に控えめな一言（数秒で消える）
        if SAVED.load(Ordering::Relaxed) {
            let hb = ctl(IDOK.0);
            let mut wr = RECT::default();
            if !hb.0.is_null() && GetWindowRect(hb, &mut wr).is_ok() {
                let mut pts = [POINT { x: wr.left, y: wr.top }, POINT { x: wr.right, y: wr.bottom }];
                MapWindowPoints(None, Some(hwnd), &mut pts);
                SetTextColor(hdc, rgb_ref(&acc));
                let mut t: Vec<u16> = "✓ 保存しました".encode_utf16().collect();
                let mut r = RECT { left: px(PAGE_X) as i32, top: pts[0].y, right: pts[0].x - px(12) as i32, bottom: pts[1].y };
                DrawTextW(hdc, &mut t, &mut r, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
        }
        SelectObject(hdc, old);
    }
}

unsafe extern "system" fn dlg_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                // フォーカスが入った部品が表示領域の外なら、見える位置までスクロールする
                // （BN_SETFOCUS = 6、EN_SETFOCUS = 0x100、CBN_SETFOCUS = 3。ほかの通知でも、見えていれば何もしない）
                if code == 6 || code == 0x100 || code == 3 {
                    ensure_visible(hwnd, id);
                }
                on_command(hwnd, id, code);
                LRESULT(0)
            }
            WM_SIZE => {
                relayout(hwnd);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                // 小さくしすぎて、カードが崩れないように
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let scale = dpi as f32 / 96.0;
                let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
                let mut rc = RECT { left: 0, top: 0, right: (640.0 * scale) as i32, bottom: (360.0 * scale) as i32 };
                let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
                mmi.ptMinTrackSize = POINT { x: rc.right - rc.left, y: rc.bottom - rc.top };
                LRESULT(0)
            }
            // 大きさをドラッグで変えているあいだは、背景を軽く描く。終わったら、滑らかな描画に戻す
            0x0231 /* WM_ENTERSIZEMOVE */ => {
                SIZING.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            0x0232 /* WM_EXITSIZEMOVE */ => {
                SIZING.store(false, Ordering::Relaxed);
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // 1 ノッチ（120）で WHEEL_STEP だけ、ページをスクロールする
                let notches = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                scroll_by(hwnd, -notches * WHEEL_STEP / 120);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == SAVED_TIMER => {
                let _ = KillTimer(Some(hwnd), SAVED_TIMER);
                SAVED.store(false, Ordering::Relaxed);
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_CLOSE => {
                // ×：保存していない変更があれば確認する
                if confirm_unsaved(hwnd) {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                let hdc = HDC(wparam.0 as *mut _);
                // 標準の部品（チェックボックス）が、親の背景を借りに来た場合：原点が子の位置にずれていて、部品の分の小さな領域だけを求められる。
                // ダイアログ全体を描き直す（重い、かつ位置もずれる）かわりに、カードと同じ色で塗る（部品はカードの上にある）
                let mut org = POINT::default();
                let _ = GetViewportOrgEx(hdc, &mut org);
                let (mut clip, mut client) = (RECT::default(), RECT::default());
                GetClipBox(hdc, &mut clip);
                let _ = GetClientRect(hwnd, &mut client);
                let small = (clip.right - clip.left) as i64 * (clip.bottom - clip.top) as i64 * 2 < (client.right as i64 * client.bottom as i64);
                if (org.x != 0 || org.y != 0) && small {
                    FillRect(hdc, &clip, HBRUSH(CARD_BRUSH[DARK.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) as *mut _));
                    return LRESULT(1);
                }
                paint_bg(hwnd, hdc);
                LRESULT(1)
            }
            WM_NOTIFY => {
                // チェックボックスの題の文字色（ダークのテーマでも、部品の既定の黒にならないように）
                let nm = &*(lparam.0 as *const NMHDR);
                if nm.code == NM_CUSTOMDRAW && is_toggle(nm.idFrom as i32) {
                    let cd = &*(lparam.0 as *const NMCUSTOMDRAW);
                    if cd.dwDrawStage == CDDS_PREPAINT {
                        SetTextColor(cd.hdc, rgb_ref(&pal().text));
                    }
                    return LRESULT(CDRF_DODEFAULT as isize);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DRAWITEM => {
                let di = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if di.CtlType == ODT_LISTBOX {
                    draw_cat_item(di);
                    return LRESULT(1);
                }
                if di.CtlType == ODT_COMBOBOX {
                    draw_accent_item(di);
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let hdc = HDC(wparam.0 as *mut _);
                let (p, i) = (pal(), DARK.load(Ordering::Relaxed) as usize);
                if msg == WM_CTLCOLORLISTBOX {
                    SetTextColor(hdc, rgb_ref(&p.text));
                    SetBkColor(hdc, rgb_ref(&p.bg));
                    return LRESULT(BG_BRUSH[i].load(Ordering::Relaxed));
                }
                if msg == WM_CTLCOLOREDIT {
                    let bg = if i == 1 { rgb(0x1f, 0x1f, 0x1f) } else { rgb(0xff, 0xff, 0xff) };
                    SetTextColor(hdc, rgb_ref(&p.text));
                    SetBkColor(hdc, rgb_ref(&bg));
                    return LRESULT(EDIT_BRUSH[i].load(Ordering::Relaxed));
                }
                // 見出し・説明の文字：背景はそのまま見せる
                SetBkMode(hdc, TRANSPARENT);
                let is_title = HWND(lparam.0 as *mut _) == ctl(ID_TITLE);
                let is_check = msg == WM_CTLCOLORBTN && is_toggle(GetDlgCtrlID(HWND(lparam.0 as *mut _)));
                SetTextColor(hdc, rgb_ref(if is_title || is_check { &p.text } else { &p.dim }));
                // 静的な文字と、カードの上に重ねた透明なボタン（スイッチ・切り替え・色の見本）は、下のカードを塗りつぶさない。
                // 通常のボタン（保存・閉じる・＋・－ など）は、ダイアログの地の色で塗る（NULL だと、角に白い下地が出る）
                if msg == WM_CTLCOLORSTATIC {
                    // 題・説明の文字は、ページの背景の上にある。透明にすると前の文字が残るので、背景色で塗る
                    SetBkMode(hdc, OPAQUE);
                    SetBkColor(hdc, rgb_ref(&p.bg));
                    return LRESULT(BG_BRUSH[i].load(Ordering::Relaxed));
                }
                if msg == WM_CTLCOLORBTN {
                    let id = GetDlgCtrlID(HWND(lparam.0 as *mut _));
                    // チェックボックスは、背景をカードと同じ色で塗る（透明にはしない。標準の部品は自分の背景を塗るため）
                    if is_toggle(id) {
                        SetBkMode(hdc, OPAQUE);
                        SetBkColor(hdc, rgb_ref(&p.card));
                        return LRESULT(CARD_BRUSH[i].load(Ordering::Relaxed));
                    }
                }
                LRESULT(BG_BRUSH[i].load(Ordering::Relaxed))
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
