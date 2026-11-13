//! 通知領域（タスクトレイ）のアイコン。左クリックで「新規作成・開く・最近使ったファイル」などのメニュー、
//! 右クリックで「最後のメッセージを表示・設定・開いているタブ・最小化」などのメニューを出す（EmEditor を参考にした）。

use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};

use super::*;

/// トレイのアイコンからの通知（lParam の下位がマウスのメッセージ）
pub(super) const WM_TRAY: u32 = WM_APP + 9;
const TRAY_ID: u32 = 1;
/// 「最近使ったフォルダー」に出す件数
const FOLDERS_MAX: usize = 10;

fn fill(dst: &mut [u16], s: &str) {
    for (i, c) in s.encode_utf16().take(dst.len() - 1).enumerate() {
        dst[i] = c;
    }
}

impl App {
    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: TRAY_ID, ..Default::default() };
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        unsafe {
            if let Ok(h) = GetModuleHandleW(None) {
                if let Ok(icon) = LoadIconW(Some(h.into()), PCWSTR(std::ptr::without_provenance::<u16>(1))) {
                    nid.hIcon = icon;
                }
            }
        }
        fill(&mut nid.szTip, APP_NAME);
        nid
    }

    pub(super) fn tray_add(&self) {
        let nid = self.tray_data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    pub(super) fn tray_remove(&self) {
        let nid = self.tray_data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    /// 設定のオン／オフ
    pub(super) fn set_tray(&mut self, on: bool) {
        self.settings.tray = on;
        if on {
            self.tray_add();
        } else {
            self.tray_remove();
        }
        self.settings.save();
    }

    /// バルーン通知で、最後のメッセージを見せる
    fn tray_balloon(&self, text: &str) {
        let mut nid = self.tray_data();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        fill(&mut nid.szInfoTitle, APP_NAME);
        fill(&mut nid.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// ウィンドウを前面に出す（最小化していれば元に戻す）
    fn activate(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// トレイのメニュー（`right` なら右クリック用）。使い終わったら呼び出し側が DestroyMenu する
    pub(super) unsafe fn build_tray_menu(&self, right: bool) -> Option<HMENU> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let add = |menu: HMENU, id: u32, label: &str, enabled: bool| {
                let w = wide(label);
                let _ = AppendMenuW(menu, MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED }, id as usize, PCWSTR(w.as_ptr()));
            };
            let sub = |menu: HMENU, child: HMENU, label: &str, enabled: bool| {
                let w = wide(label);
                let _ = AppendMenuW(menu, MF_POPUP | if enabled { MF_ENABLED } else { MF_GRAYED }, child.0 as usize, PCWSTR(w.as_ptr()));
            };
            let sep = |menu: HMENU| {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            };
            if right {
                add(m, menu::ID_TRAY_MESSAGE, "最後のメッセージを表示(&S)", !self.last_message.is_empty());
                sep(m);
                add(m, menu::ID_TRAY_SETTINGS, "設定(&Z)...", true);
                sep(m);
                add(m, menu::ID_TRAY_MINIMIZE, if IsIconic(self.hwnd).as_bool() { "元に戻す(&M)" } else { "最小化(&M)" }, true);
                sep(m);
                // 開いているタブの一覧（選ぶと、そのタブへ）
                let n = self.tabs.len().min(20);
                for i in 0..n {
                    let w = wide(&format!("&{} {}", if i < 9 { (i + 1).to_string() } else { "0".into() }, self.tab_title(i).replace('&', "&&")));
                    let flags = MF_STRING | if i == self.active { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(m, flags, (menu::ID_TRAY_TAB_BASE + i as u32) as usize, PCWSTR(w.as_ptr()));
                }
                sep(m);
                add(m, menu::ID_TRAY_REMOVE, "トレイ アイコンを閉じる(&T)", true);
            } else {
                add(m, menu::ID_TRAY_ACTIVATE, "アクティブにする(&A)", true);
                sep(m);
                add(m, menu::ID_TRAY_NEW, "新規作成(&N)", true);
                add(m, menu::ID_TRAY_NEW_PASTE, "新規作成して貼り付け(&P)", true);
                add(m, menu::ID_TRAY_NEW_QUOTE, "新規作成して引用付き貼り付け(&Q)", true);
                add(m, menu::ID_TRAY_OPEN, "開く(&O)...", true);
                add(m, menu::ID_TRAY_GREP, "ファイルから検索(&I)...", true);
                sep(m);
                // 最近使ったファイル
                let files = CreatePopupMenu().ok()?;
                for (i, p) in self.recent.iter().take(menu::RECENT_MAX).enumerate() {
                    add(files, menu::ID_RECENT_BASE + i as u32, &format!("&{} {}", (i + 1) % 10, p.display().to_string().replace('&', "&&")), true);
                }
                sub(m, files, "最近使ったファイル(&R)", !self.recent.is_empty());
                // 最近使ったフォルダー（最近のファイルのあるフォルダー）
                let folders = CreatePopupMenu().ok()?;
                let dirs = self.recent_folders();
                for (i, d) in dirs.iter().enumerate() {
                    add(folders, menu::ID_TRAY_FOLDER_BASE + i as u32, &format!("&{} {}", (i + 1) % 10, d.display().to_string().replace('&', "&&")), true);
                }
                sub(m, folders, "最近使ったフォルダー(&F)", !dirs.is_empty());
                let ws = CreatePopupMenu().ok()?;
                add(ws, menu::ID_TRAY_RESTORE, "前回終了時のタブを開く(&R)", true);
                sub(m, ws, "ワークスペース(&K)", true);
                sep(m);
                add(m, menu::ID_TRAY_SAVE_CLOSE, "すべて保存して閉じる(&V)", true);
                add(m, menu::ID_TRAY_CLOSE_ALL, "すべて閉じる(&C)", true);
                let _ = SetMenuDefaultItem(m, menu::ID_TRAY_NEW, 0);
            }
            Some(m)
        }
    }

    /// 最近使ったファイルのあるフォルダー（重複なし、新しい順）
    fn recent_folders(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for p in &self.recent {
            if let Some(d) = p.parent().map(|d| d.to_path_buf()) {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
            if out.len() >= FOLDERS_MAX {
                break;
            }
        }
        out
    }

    /// トレイのメニューのコマンド。処理したら Some（中身は続きの操作）
    pub(super) unsafe fn command_tray(&mut self, id: u32) -> Option<Option<Deferred>> {
        let r = match id {
            menu::ID_TRAY_ACTIVATE => {
                self.activate();
                None
            }
            menu::ID_TRAY_NEW => {
                self.activate();
                self.new_tab();
                None
            }
            menu::ID_TRAY_NEW_PASTE | menu::ID_TRAY_NEW_QUOTE => {
                self.activate();
                self.new_tab();
                if id == menu::ID_TRAY_NEW_PASTE {
                    self.paste();
                } else if let Some(text) = clipboard_text() {
                    let nl = self.newline_str();
                    let quoted: Vec<String> = text.replace("\r\n", "\n").replace('\r', "\n").lines().map(|l| format!("> {l}")).collect();
                    let mut s = quoted.join(nl);
                    s.push_str(nl);
                    self.type_text(&s, EditKind::Other);
                }
                self.sel = Some((0, 0));
                self.top = 0;
                self.invalidate();
                None
            }
            menu::ID_TRAY_OPEN => {
                self.activate();
                self.guarded(Pending::OpenDialog)
            }
            menu::ID_TRAY_GREP => {
                self.activate();
                Some(Deferred::ShowFind(2))
            }
            menu::ID_TRAY_RESTORE => {
                self.activate();
                let (paths, _) = crate::session::load();
                for p in paths {
                    self.open(p, None);
                }
                None
            }
            menu::ID_TRAY_SAVE_CLOSE => {
                self.activate();
                self.guarded(Pending::SaveCloseAll)
            }
            menu::ID_TRAY_CLOSE_ALL => {
                self.activate();
                self.guarded(Pending::CloseAll)
            }
            menu::ID_TRAY_MESSAGE => {
                let m = self.last_message.clone();
                self.tray_balloon(&m);
                None
            }
            menu::ID_TRAY_SETTINGS => {
                self.activate();
                Some(Deferred::ShowSettings)
            }
            menu::ID_TRAY_MINIMIZE => {
                unsafe {
                    if IsIconic(self.hwnd).as_bool() {
                        let _ = ShowWindow(self.hwnd, SW_RESTORE);
                    } else {
                        let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                    }
                }
                None
            }
            menu::ID_TRAY_REMOVE => {
                self.set_tray(false);
                self.message = "トレイ アイコンを閉じました（設定ダイアログで、また出せます）".into();
                self.invalidate();
                None
            }
            id if (menu::ID_TRAY_FOLDER_BASE..menu::ID_TRAY_FOLDER_BASE + FOLDERS_MAX as u32).contains(&id) => {
                // そのフォルダーを開くダイアログの初期位置にする
                self.activate();
                let dir = self.recent_folders().get((id - menu::ID_TRAY_FOLDER_BASE) as usize).cloned();
                self.open_dir = dir;
                self.guarded(Pending::OpenDialog)
            }
            id if (menu::ID_TRAY_TAB_BASE..menu::ID_TRAY_TAB_BASE + 20).contains(&id) => {
                self.activate();
                self.switch_to((id - menu::ID_TRAY_TAB_BASE) as usize);
                None
            }
            _ => return None,
        };
        Some(r)
    }
}
