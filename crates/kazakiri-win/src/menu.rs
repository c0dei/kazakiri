//! メニューバー。

use std::path::PathBuf;

use encoding_rs::Encoding;
use windows::core::{Result, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ID_OPEN: u32 = 100;
pub const ID_RELOAD: u32 = 101;
pub const ID_EXIT: u32 = 102;
pub const ID_FOLLOW: u32 = 103;
pub const ID_COPY: u32 = 200;
pub const ID_SELECT_ALL: u32 = 201;
pub const ID_FIND: u32 = 300;
pub const ID_FIND_NEXT: u32 = 301;
pub const ID_FIND_PREV: u32 = 302;
pub const ID_CASE: u32 = 303;
pub const ID_REGEX: u32 = 304;
pub const ID_GOTO: u32 = 305;
/// エンコードの選択肢（ENCODINGS の並びと対応）
pub const ID_ENC_BASE: u32 = 400;
/// 最近使ったファイル
pub const ID_RECENT_BASE: u32 = 500;
pub const RECENT_MAX: usize = 10;

/// (表示名, 文字コード)。None は自動判定
pub const ENCODINGS: [(&str, Option<&'static Encoding>); 6] = [
    ("自動判定", None),
    ("UTF-8", Some(encoding_rs::UTF_8)),
    ("Shift_JIS (CP932)", Some(encoding_rs::SHIFT_JIS)),
    ("EUC-JP", Some(encoding_rs::EUC_JP)),
    ("UTF-16LE", Some(encoding_rs::UTF_16LE)),
    ("UTF-16BE", Some(encoding_rs::UTF_16BE)),
];

pub struct Menus {
    pub bar: HMENU,
    pub file: HMENU,
    pub search: HMENU,
    pub encoding: HMENU,
    pub recent: HMENU,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn item(menu: HMENU, id: u32, text: &str) -> Result<()> {
    let w = wide(text);
    unsafe { AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(w.as_ptr())) }
}

unsafe fn sep(menu: HMENU) -> Result<()> {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
}

unsafe fn popup(bar: HMENU, sub: HMENU, text: &str) -> Result<()> {
    let w = wide(text);
    unsafe { AppendMenuW(bar, MF_POPUP, sub.0 as usize, PCWSTR(w.as_ptr())) }
}

pub unsafe fn build() -> Result<Menus> {
    unsafe {
        let bar = CreateMenu()?;

        let file = CreatePopupMenu()?;
        let recent = CreatePopupMenu()?;
        item(file, ID_OPEN, "開く(&O)...\tCtrl+O")?;
        item(file, ID_RELOAD, "再読み込み(&R)\tF5")?;
        item(file, ID_FOLLOW, "末尾を追従(&T)\tCtrl+T")?;
        sep(file)?;
        popup(file, recent, "最近使ったファイル(&F)")?;
        sep(file)?;
        item(file, ID_EXIT, "終了(&X)")?;
        popup(bar, file, "ファイル(&F)")?;

        let edit = CreatePopupMenu()?;
        item(edit, ID_COPY, "コピー(&C)\tCtrl+C")?;
        item(edit, ID_SELECT_ALL, "すべて選択(&A)\tCtrl+A")?;
        popup(bar, edit, "編集(&E)")?;

        let search = CreatePopupMenu()?;
        item(search, ID_FIND, "検索(&F)...\tCtrl+F")?;
        item(search, ID_FIND_NEXT, "次を検索(&N)\tF3")?;
        item(search, ID_FIND_PREV, "前を検索(&P)\tShift+F3")?;
        sep(search)?;
        item(search, ID_CASE, "大文字と小文字を区別(&C)\tAlt+C")?;
        item(search, ID_REGEX, "正規表現(&R)\tAlt+R")?;
        sep(search)?;
        item(search, ID_GOTO, "行へ移動(&G)...\tCtrl+G")?;
        popup(bar, search, "検索(&S)")?;

        let encoding = CreatePopupMenu()?;
        for (i, (name, _)) in ENCODINGS.iter().enumerate() {
            if i == 1 {
                sep(encoding)?;
            }
            item(encoding, ID_ENC_BASE + i as u32, &format!("{name} で開き直す"))?;
        }
        popup(bar, encoding, "エンコード(&N)")?;

        Ok(Menus { bar, file, search, encoding, recent })
    }
}

impl Menus {
    /// メニューを開く直前にチェック状態などを最新にする
    pub unsafe fn refresh(&self, case: bool, regex: bool, follow: bool, forced: Option<Option<&'static Encoding>>, recent: &[PathBuf]) {
        unsafe {
            let check = |on: bool| if on { MF_CHECKED.0 } else { MF_UNCHECKED.0 };
            CheckMenuItem(self.file, ID_FOLLOW, check(follow));
            CheckMenuItem(self.search, ID_CASE, check(case));
            CheckMenuItem(self.search, ID_REGEX, check(regex));
            for i in 0..ENCODINGS.len() as u32 {
                let _ = EnableMenuItem(self.encoding, ID_ENC_BASE + i, if forced.is_some() { MF_ENABLED } else { MF_GRAYED });
            }
            if let Some(forced) = forced {
                let idx = ENCODINGS.iter().position(|(_, e)| *e == forced).unwrap_or(0) as u32;
                let _ = CheckMenuRadioItem(
                    self.encoding,
                    ID_ENC_BASE,
                    ID_ENC_BASE + ENCODINGS.len() as u32 - 1,
                    ID_ENC_BASE + idx,
                    MF_BYCOMMAND.0,
                );
            }
            // 最近使ったファイルは作り直す
            while GetMenuItemCount(Some(self.recent)) > 0 {
                if DeleteMenu(self.recent, 0, MF_BYPOSITION).is_err() {
                    break;
                }
            }
            if recent.is_empty() {
                let _ = item(self.recent, 0, "（なし）");
                let _ = EnableMenuItem(self.recent, 0, MF_BYPOSITION | MF_GRAYED);
            }
            for (i, p) in recent.iter().enumerate() {
                let label = format!("&{} {}", (i + 1) % 10, p.display());
                let _ = item(self.recent, ID_RECENT_BASE + i as u32, &label);
            }
        }
    }
}
