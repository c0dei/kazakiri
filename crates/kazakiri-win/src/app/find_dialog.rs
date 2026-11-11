//! 検索・置換・GREP のダイアログ（Ctrl+F / Ctrl+H / Ctrl+Shift+F）。
//!
//! 1 つのモードレスのダイアログに「検索」「置換」「GREP」のタブを持たせる。開いたまま本文を編集できる。
//! ネイティブのコントロール（コンボボックス・チェックボックス・ボタン・タブ）を並べた自前のウィンドウで、
//! Tab キーでの移動・Enter（既定のボタン）・Esc（閉じる）は `IsDialogMessage` に任せる。
//! ダークテーマのときは `WM_CTLCOLOR*` で色を付ける。
//! 操作は `App` の既存の機能（find / find_all / replace_next / replace_all / start_grep）を呼ぶだけ。

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Shell::{SHBrowseForFolderW, SHGetPathFromIDListW, BROWSEINFOW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS};

use super::*;

/// ダイアログのウィンドウ（0 ならまだない）。メッセージループの `IsDialogMessage` が見る
pub(super) static FIND_DLG: AtomicIsize = AtomicIsize::new(0);
/// 描画のメッセージ（WM_ERASEBKGND など）は、状態を更新している最中にも同期的に届くので、
/// 描画に使う値は RefCell の外に置く
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
static FIND_FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

const CLASS: PCWSTR = w!("KazakiriFindDialog");

// コントロールの ID
/// タブ（押し込み式のラジオボタン）の ID。+ タブの番号
const ID_TAB0: i32 = 140;
const ID_FIND: i32 = 101;
const ID_REPL: i32 = 102;
const ID_FOLDER: i32 = 103;
const ID_FILES: i32 = 104;
const ID_BROWSE: i32 = 105;
const ID_CASE: i32 = 110;
const ID_REGEX: i32 = 111;
const ID_SUBDIR: i32 = 112;
const ID_BACKUP: i32 = 113;
const ID_SELONLY: i32 = 114;
const ID_NEXT: i32 = 120;
const ID_PREV: i32 = 121;
const ID_ALL: i32 = 122;
const ID_REPL_NEXT: i32 = 123;
const ID_REPL_ALL: i32 = 124;
const ID_GREP: i32 = 125;
const ID_GREP_REPLACE: i32 = 126;
const ID_CLOSE: i32 = 130;

const TAB_FIND: usize = 0;
const TAB_REPLACE: usize = 1;
const TAB_GREP: usize = 2;

struct Ctrls {
    dlg: HWND,
    /// 画面の倍率
    scale: f32,
    cur_tab: usize,
    dark: bool,
    /// (ID, コントロール)。ID でも探せるよう全部入れる
    all: Vec<(i32, HWND)>,
}

thread_local! {
    static CTRLS: RefCell<Option<Ctrls>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb_ref(c: &D2D1_COLOR_F) -> COLORREF {
    let b = |v: f32| (v * 255.0).round() as u32;
    COLORREF(b(c.r) | (b(c.g) << 8) | (b(c.b) << 16))
}

/// 入力した内容
#[derive(Default, Clone)]
struct Form {
    tab: usize,
    find: String,
    repl: String,
    folder: String,
    files: String,
    case: bool,
    regex: bool,
    subdir: bool,
    backup: bool,
    selonly: bool,
}

impl Ctrls {
    fn ctl(&self, id: i32) -> HWND {
        self.all.iter().find(|(i, _)| *i == id).map_or(HWND::default(), |(_, h)| *h)
    }

    fn text(&self, id: i32) -> String {
        let h = self.ctl(id);
        let mut buf = vec![0u16; 4096];
        let n = unsafe { GetWindowTextW(h, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
    }

    fn set_text(&self, id: i32, s: &str) {
        let w = wide(s);
        unsafe {
            let _ = SetWindowTextW(self.ctl(id), PCWSTR(w.as_ptr()));
        }
    }

    fn checked(&self, id: i32) -> bool {
        unsafe { SendMessageW(self.ctl(id), BM_GETCHECK, None, None) }.0 == BST_CHECKED.0 as isize
    }

    fn set_checked(&self, id: i32, on: bool) {
        unsafe {
            SendMessageW(self.ctl(id), BM_SETCHECK, Some(WPARAM(if on { BST_CHECKED.0 } else { BST_UNCHECKED.0 } as usize)), None);
        }
    }

    fn form(&self) -> Form {
        Form {
            tab: self.cur_tab,
            find: self.text(ID_FIND),
            repl: self.text(ID_REPL),
            folder: self.text(ID_FOLDER),
            files: self.text(ID_FILES),
            case: self.checked(ID_CASE),
            regex: self.checked(ID_REGEX),
            subdir: self.checked(ID_SUBDIR),
            backup: self.checked(ID_BACKUP),
            selonly: self.checked(ID_SELONLY),
        }
    }

    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    fn place(&self, id: i32, x: i32, y: i32, w: i32, h: i32, show: bool) {
        let hw = self.ctl(id);
        unsafe {
            let _ = SetWindowPos(hw, None, self.px(x), self.px(y), self.px(w), self.px(h), SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(hw, if show { SW_SHOW } else { SW_HIDE });
        }
    }

    /// タブに合わせて、コントロールの表示・位置・ダイアログの大きさを変える
    fn layout(&mut self, tab: usize) {
        self.cur_tab = tab;
        let (replace, grep) = (tab == TAB_REPLACE, tab == TAB_GREP);
        // 右の列のボタン
        let buttons: &[(i32, &str)] = match tab {
            TAB_FIND => &[(ID_NEXT, "次を検索(&N)"), (ID_PREV, "前を検索(&P)"), (ID_ALL, "すべて検索(&A)"), (ID_CLOSE, "閉じる")],
            TAB_REPLACE => &[(ID_NEXT, "次を検索(&N)"), (ID_REPL_NEXT, "置換して次へ(&R)"), (ID_REPL_ALL, "すべて置換(&A)"), (ID_ALL, "すべて検索(&L)"), (ID_CLOSE, "閉じる")],
            _ => &[(ID_GREP, "検索(&S)"), (ID_GREP_REPLACE, "結果を置換(&R)..."), (ID_CLOSE, "閉じる")],
        };
        for id in [ID_NEXT, ID_PREV, ID_ALL, ID_REPL_NEXT, ID_REPL_ALL, ID_GREP, ID_GREP_REPLACE, ID_CLOSE] {
            let pos = buttons.iter().position(|(b, _)| *b == id);
            match pos {
                Some(i) => {
                    let label = wide(buttons[i].1);
                    unsafe {
                        let _ = SetWindowTextW(self.ctl(id), PCWSTR(label.as_ptr()));
                    }
                    self.place(id, 344, 44 + 32 * i as i32, 112, 26, true);
                }
                None => self.place(id, 0, 0, 0, 0, false),
            }
        }
        // 左の入力欄
        let mut y = 44;
        self.place(ID_FIND, 20, y + 18, 306, 200, true);
        y += 52;
        self.place(ID_REPL, 20, y + 18, 306, 200, replace || grep);
        if grep {
            y += 52;
        }
        let (folder_y, files_y) = if grep { (y, y + 52) } else { (0, 0) };
        // GREP の場合は、置換後の文字列の下にフォルダとファイル
        self.place(ID_FOLDER, 20, folder_y + 18, 252, 24, grep);
        self.place(ID_BROWSE, 278, folder_y + 18, 48, 24, grep);
        self.place(ID_FILES, 20, files_y + 18, 306, 24, grep);
        let mut cy = match tab {
            TAB_FIND => y + 56 - 4,
            TAB_REPLACE => y + 56,
            _ => files_y + 56,
        };
        self.place(ID_CASE, 20, cy, 150, 22, true);
        self.place(ID_REGEX, 176, cy, 150, 22, true);
        cy += 26;
        self.place(ID_SELONLY, 20, cy, 306, 22, replace);
        self.place(ID_SUBDIR, 20, cy, 150, 22, grep);
        self.place(ID_BACKUP, 176, cy, 160, 22, grep);
        let content_bottom = cy + 30;
        let btn_bottom = 44 + 32 * buttons.len() as i32 + 8;
        let height = content_bottom.max(btn_bottom) + 8;
        // ラベル
        self.set_labels(tab);
        // タブのボタン
        for i in 0..3 {
            self.place(ID_TAB0 + i as i32, 8 + 84 * i as i32, 8, 80, 26, true);
            self.set_checked(ID_TAB0 + i as i32, i == tab);
        }
        // 大きさ（クライアント領域が 464 × height）
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongW(self.dlg, GWL_STYLE) as u32);
            let mut rc = RECT { left: 0, top: 0, right: self.px(464), bottom: self.px(height) };
            let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), GetDpiForWindow(self.dlg));
            let _ = SetWindowPos(self.dlg, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.set_title(tab);
    }

    fn set_title(&self, tab: usize) {
        let t = wide(["検索", "置換", "GREP（フォルダ内を検索）"][tab]);
        unsafe {
            let _ = SetWindowTextW(self.dlg, PCWSTR(t.as_ptr()));
        }
    }

    /// 入力欄の上のラベル（タブごとに位置と文言が変わる）
    fn set_labels(&self, tab: usize) {
        let grep = tab == TAB_GREP;
        let mut y = 44;
        let rows: Vec<(i32, &str, i32)> = {
            let mut v = vec![(1001, "検索する文字列(&F)", y)];
            y += 52;
            if tab != TAB_FIND {
                v.push((1002, "置換後の文字列(&W)", y));
                if grep {
                    y += 52;
                }
            }
            if grep {
                v.push((1003, "フォルダ(&D)（サブフォルダも含む）", y));
                v.push((1004, "対象のファイル(&T)（; で区切る）", y + 52));
            }
            v
        };
        for id in [1001, 1002, 1003, 1004] {
            match rows.iter().find(|(i, _, _)| *i == id) {
                Some((_, text, ly)) => {
                    let w = wide(text);
                    unsafe {
                        let hw = self.ctl(id);
                        let _ = SetWindowTextW(hw, PCWSTR(w.as_ptr()));
                    }
                    self.place(id, 20, *ly, 306, 18, true);
                }
                None => self.place(id, 0, 0, 0, 0, false),
            }
        }
    }
}

unsafe fn make_child(parent: HWND, class: PCWSTR, text: &str, style: u32, ex: u32, id: i32, font: HFONT) -> HWND {
    let t = wide(text);
    unsafe {
        let hw = CreateWindowExW(
            WINDOW_EX_STYLE(ex),
            class,
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as isize as *mut _)),
            GetModuleHandleW(None).ok().map(|h| h.into()),
            None,
        )
        .unwrap_or_default();
        SendMessageW(hw, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        hw
    }
}

/// ダイアログを（なければ作って）開く。`tab` はタブの番号
pub(super) unsafe fn show(owner: HWND, tab: usize) {
    unsafe {
        if FIND_DLG.load(Ordering::Relaxed) == 0 {
            create(owner);
        }
        let dlg = HWND(FIND_DLG.load(Ordering::Relaxed) as *mut _);
        if dlg.0.is_null() {
            return;
        }
        // 入力欄の初期値：選択中の短い 1 行の文字列、なければ前回の検索語
        let init = with_app(|app| app.dialog_init()).unwrap_or_default();
        CTRLS.with(|c| {
            if let Some(ct) = c.borrow_mut().as_mut() {
                ct.dark = init.dark;
                DARK.store(init.dark, Ordering::Relaxed);
                ct.refill_history(&init);
                ct.set_text(ID_FIND, &init.find);
                ct.set_text(ID_REPL, &init.repl);
                ct.set_text(ID_FOLDER, &init.folder);
                ct.set_text(ID_FILES, &init.files);
                ct.set_checked(ID_CASE, init.case);
                ct.set_checked(ID_REGEX, init.regex);
                ct.set_checked(ID_SUBDIR, true);
                ct.set_checked(ID_BACKUP, init.backup);
                ct.set_checked(ID_SELONLY, init.scope);
                let _ = EnableWindow(ct.ctl(ID_SELONLY), init.scope);
                ct.layout(tab);
            }
        });
        set_dark(init.dark);
        // 初めて開くときは、本文のウィンドウの左上あたりに出す
        if FIND_FIRST.swap(false, Ordering::Relaxed) {
            let mut r = RECT::default();
            let _ = GetWindowRect(owner, &mut r);
            let _ = SetWindowPos(dlg, None, r.left + 60, r.top + 110, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let first = dlg_ctl(ID_FIND);
        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
        let _ = SetFocus(Some(first));
        // コンボボックスの入力欄の文字を全部選んで、打てばすぐ置き換わるように
        SendMessageW(first, CB_SETEDITSEL, None, Some(LPARAM(-1_isize << 16)));
    }
}

/// ダイアログにフォーカスがあるときの Ctrl+F / Ctrl+H / Ctrl+Shift+F（別のタブへ移る）。処理したら true
pub(super) unsafe fn accelerator(main: HWND, msg: &MSG) -> bool {
    unsafe {
        if msg.message != WM_KEYDOWN {
            return false;
        }
        let dlg = HWND(FIND_DLG.load(Ordering::Relaxed) as *mut _);
        // メッセージの宛先が、ダイアログ自身かその部品（コンボボックスの入力欄など、孫を含む）のとき
        if dlg.0.is_null() || (msg.hwnd != dlg && !IsChild(dlg, msg.hwnd).as_bool()) {
            return false;
        }
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
        let tab = match (ctrl, shift, msg.wParam.0 as u16) {
            (true, false, k) if k == VK_F.0 => TAB_FIND,
            (true, false, k) if k == VK_H.0 => TAB_REPLACE,
            (true, true, k) if k == VK_F.0 => TAB_GREP,
            _ => return false,
        };
        show(main, tab);
        true
    }
}

fn dlg_ctl(id: i32) -> HWND {
    CTRLS.with(|c| c.borrow().as_ref().map_or(HWND::default(), |ct| ct.ctl(id)))
}

unsafe fn apply_dark_title(dlg: HWND, dark: bool) {
    let v = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(dlg, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
    }
}

/// テーマが変わったとき
pub(super) unsafe fn set_dark(dark: bool) {
    unsafe {
        let dlg = HWND(FIND_DLG.load(Ordering::Relaxed) as *mut _);
        if dlg.0.is_null() {
            return;
        }
        CTRLS.with(|c| {
            if let Some(ct) = c.borrow_mut().as_mut() {
                ct.dark = dark;
                DARK.store(dark, Ordering::Relaxed);
                for (_, h) in &ct.all {
                    let _ = SetWindowTheme(*h, if dark { w!("DarkMode_Explorer") } else { PCWSTR::null() }, PCWSTR::null());
                }
            }
        });
        apply_dark_title(dlg, dark);
        let _ = InvalidateRect(Some(dlg), None, true);
    }
}

pub(super) struct DialogInit {
    find: String,
    repl: String,
    folder: String,
    files: String,
    case: bool,
    regex: bool,
    backup: bool,
    /// 複数行を選んでいるか（「選択範囲内のみ」を使えるか）
    scope: bool,
    dark: bool,
    find_history: Vec<String>,
    repl_history: Vec<String>,
}

impl Default for DialogInit {
    fn default() -> Self {
        DialogInit {
            find: String::new(),
            repl: String::new(),
            folder: String::new(),
            files: String::new(),
            case: false,
            regex: false,
            backup: true,
            scope: false,
            dark: false,
            find_history: Vec::new(),
            repl_history: Vec::new(),
        }
    }
}

impl Ctrls {
    fn refill_history(&self, init: &DialogInit) {
        for (id, list) in [(ID_FIND, &init.find_history), (ID_REPL, &init.repl_history)] {
            let h = self.ctl(id);
            unsafe {
                SendMessageW(h, CB_RESETCONTENT, None, None);
                for w in list {
                    let t = wide(w);
                    SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(t.as_ptr() as isize)));
                }
            }
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
        let Ok(dlg) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("検索"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
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
        // 画面の標準のメッセージ用フォント
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), 0, dpi).is_ok() {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        };
        let st = |v: u32| v;
        let combo = st(CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32 | WS_VSCROLL.0 | WS_TABSTOP.0);
        let edit = st(ES_AUTOHSCROLL as u32 | WS_TABSTOP.0);
        let check = st(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0);
        let push = st(BS_PUSHBUTTON as u32 | WS_TABSTOP.0);
        let mut all: Vec<(i32, HWND)> = Vec::new();
        for (i, name) in ["検索", "置換", "GREP"].iter().enumerate() {
            let group = if i == 0 { WS_GROUP.0 } else { 0 };
            let id = ID_TAB0 + i as i32;
            all.push((id, make_child(dlg, w!("BUTTON"), name, BS_AUTORADIOBUTTON as u32 | BS_PUSHLIKE as u32 | WS_TABSTOP.0 | group, 0, id, font)));
        }
        for id in [1001, 1002, 1003, 1004] {
            all.push((id, make_child(dlg, w!("STATIC"), "", 0, 0, id, font)));
        }
        all.push((ID_FIND, make_child(dlg, w!("COMBOBOX"), "", combo, 0, ID_FIND, font)));
        all.push((ID_REPL, make_child(dlg, w!("COMBOBOX"), "", combo, 0, ID_REPL, font)));
        all.push((ID_FOLDER, make_child(dlg, w!("EDIT"), "", edit, WS_EX_CLIENTEDGE.0, ID_FOLDER, font)));
        all.push((ID_FILES, make_child(dlg, w!("EDIT"), "", edit, WS_EX_CLIENTEDGE.0, ID_FILES, font)));
        all.push((ID_BROWSE, make_child(dlg, w!("BUTTON"), "参照...", push, 0, ID_BROWSE, font)));
        for (id, text) in [
            (ID_CASE, "大文字と小文字を区別(&C)"),
            (ID_REGEX, "正規表現(&E)"),
            (ID_SUBDIR, "サブフォルダも検索(&U)"),
            (ID_BACKUP, "置換前にバックアップ(.bak)"),
            (ID_SELONLY, "選択範囲の中だけ(&S)"),
        ] {
            all.push((id, make_child(dlg, w!("BUTTON"), text, check, 0, id, font)));
        }
        for id in [ID_NEXT, ID_PREV, ID_ALL, ID_REPL_NEXT, ID_REPL_ALL, ID_GREP, ID_GREP_REPLACE, ID_CLOSE] {
            all.push((id, make_child(dlg, w!("BUTTON"), "", push, 0, id, font)));
        }
        let (bg, edit_bg) = (CreateSolidBrush(rgb_ref(&super::theme::DARK.gutter_bg)), CreateSolidBrush(rgb_ref(&super::theme::DARK.bg)));
        let mut ct = Ctrls { dlg, scale, cur_tab: 0, dark: false, all };
        ct.layout(TAB_FIND);
        BG_BRUSH.store(bg.0 as isize, Ordering::Relaxed);
        EDIT_BRUSH.store(edit_bg.0 as isize, Ordering::Relaxed);
        CTRLS.with(|c| *c.borrow_mut() = Some(ct));
        FIND_DLG.store(dlg.0 as isize, Ordering::Relaxed);
    }
}

fn switch_tab(tab: usize) {
    CTRLS.with(|c| {
        if let Ok(mut b) = c.try_borrow_mut() {
            let Some(ct) = b.as_mut() else { return };
            // 入力した内容はタブをまたいで残す（レイアウトを変えるだけ）
            ct.layout(tab);
        }
    });
}

fn current_form() -> Option<Form> {
    CTRLS.with(|c| c.try_borrow().ok().and_then(|b| b.as_ref().map(|ct| ct.form())))
}

fn hide(dlg: HWND) {
    unsafe {
        let _ = ShowWindow(dlg, SW_HIDE);
    }
}

/// ボタンの押下。本文のウィンドウに操作を渡す
unsafe fn on_command(dlg: HWND, id: i32) {
    unsafe {
        let owner = GetWindow(dlg, GW_OWNER).unwrap_or_default();
        if id == ID_CLOSE || id == IDCANCEL.0 {
            return hide(dlg);
        }
        if id == ID_BROWSE {
            if let Some(path) = pick_folder(dlg) {
                CTRLS.with(|c| {
                    if let Some(ct) = c.borrow().as_ref() {
                        ct.set_text(ID_FOLDER, &path);
                    }
                });
            }
            return;
        }
        let Some(form) = current_form() else { return };
        // Enter は、タブごとの既定の操作
        let id = if id == IDOK.0 { if form.tab == TAB_GREP { ID_GREP } else { ID_NEXT } } else { id };
        if !matches!(id, ID_NEXT | ID_PREV | ID_ALL | ID_REPL_NEXT | ID_REPL_ALL | ID_GREP | ID_GREP_REPLACE) {
            return;
        }
        let deferred = with_app(|app| app.dialog_action(id, &form)).flatten();
        if let Some(d) = deferred {
            run_deferred(owner, d);
        }
        // 見つかった所を見えるように、本文のウィンドウへ描き直しを促す
        let _ = InvalidateRect(Some(owner), None, false);
    }
}

unsafe fn pick_folder(owner: HWND) -> Option<String> {
    unsafe {
        let title = wide("GREP するフォルダを選んでください");
        let bi = BROWSEINFOW { hwndOwner: owner, lpszTitle: PCWSTR(title.as_ptr()), ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE, ..Default::default() };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, &mut buf).as_bool();
        windows::Win32::System::Com::CoTaskMemFree(Some(pidl as *const _));
        ok.then(|| String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]))
    }
}

unsafe extern "system" fn dlg_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                // ボタンの押下と、IsDialogMessage が送る IDOK / IDCANCEL（コンボボックスなどからの通知は除く）
                if code == BN_CLICKED && (ID_TAB0..ID_TAB0 + 3).contains(&id) {
                    switch_tab((id - ID_TAB0) as usize);
                    return LRESULT(0);
                }
                let is_check = matches!(id, ID_CASE | ID_REGEX | ID_SUBDIR | ID_BACKUP | ID_SELONLY);
                if (code == BN_CLICKED || id == IDOK.0 || id == IDCANCEL.0) && !is_check {
                    on_command(hwnd, id);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                hide(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                let hdc = HDC(wparam.0 as *mut _);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                if DARK.load(Ordering::Relaxed) {
                    FillRect(hdc, &rc, HBRUSH(BG_BRUSH.load(Ordering::Relaxed) as *mut _));
                } else {
                    FillRect(hdc, &rc, GetSysColorBrush(COLOR_BTNFACE));
                }
                LRESULT(1)
            }
            WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                if !DARK.load(Ordering::Relaxed) {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let hdc = HDC(wparam.0 as *mut _);
                let t = &super::theme::DARK;
                let editable = msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX;
                SetTextColor(hdc, rgb_ref(&t.text));
                SetBkColor(hdc, rgb_ref(if editable { &t.bg } else { &t.gutter_bg }));
                LRESULT(if editable { EDIT_BRUSH.load(Ordering::Relaxed) } else { BG_BRUSH.load(Ordering::Relaxed) })
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl App {
    /// ダイアログを開くときの入力欄の初期値
    pub(super) fn dialog_init(&mut self) -> DialogInit {
        // 選択中の短い 1 行の文字列があれば検索語にする
        let selected = self.selected_text(1024).filter(|s| !s.contains('\n'));
        // 複数行を選んでいるときは、置換を選択範囲の中だけに絞れる
        self.dlg_scope = self.selection().filter(|&(a, b)| {
            self.doc.as_ref().is_some_and(|d| d.line_of_offset(a) != d.line_of_offset(b.saturating_sub(1)))
        });
        DialogInit {
            find: selected.unwrap_or_else(|| self.last_find.clone()),
            repl: self.last_replace.clone(),
            folder: self.default_grep_folder(),
            files: self.grep_files.clone(),
            case: self.opts.case_sensitive,
            regex: self.opts.regex,
            backup: self.settings.grep_backup,
            scope: self.dlg_scope.is_some(),
            dark: self.settings.dark,
            find_history: self.history.list(crate::history::Kind::Find).to_vec(),
            repl_history: self.history.list(crate::history::Kind::Replace).to_vec(),
        }
    }

    /// ダイアログのボタンに対応する操作
    fn dialog_action(&mut self, id: i32, f: &Form) -> Option<Deferred> {
        self.last_find = f.find.clone();
        self.last_replace = f.repl.clone();
        self.opts = SearchOptions { case_sensitive: f.case, regex: f.regex };
        self.history.push(crate::history::Kind::Find, &f.find);
        if f.tab != TAB_FIND {
            self.history.push(crate::history::Kind::Replace, &f.repl);
        }
        if f.tab == TAB_GREP {
            self.grep_folder = f.folder.clone();
            self.grep_files = f.files.clone();
            self.grep_recursive = f.subdir;
            self.settings.grep_backup = f.backup;
            self.settings.save();
        }
        if f.find.is_empty() {
            self.message = "検索する文字列を入力してください".into();
            self.invalidate();
            return None;
        }
        match id {
            ID_NEXT => self.find(Direction::Forward),
            ID_PREV => self.find(Direction::Backward),
            ID_ALL => self.find_all(),
            ID_REPL_NEXT => self.replace_next(true),
            ID_REPL_ALL => {
                self.replace_scope = if f.selonly { self.dlg_scope } else { None };
                self.replace_all();
            }
            ID_GREP => self.start_grep(),
            ID_GREP_REPLACE => {
                if !matches!(self.results.as_ref().map(|r| &r.kind), Some(results::ResultKind::Grep { .. })) {
                    self.message = "GREP の結果がありません。先に「検索」を押してください".into();
                    self.invalidate();
                    return None;
                }
                if let Some(q) = self.grep_replace_question() {
                    return Some(Deferred::ConfirmGrepReplace(q));
                }
            }
            _ => {}
        }
        None
    }
}
