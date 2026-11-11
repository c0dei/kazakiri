//! メニューバー。

use std::path::PathBuf;

use encoding_rs::Encoding;

use crate::keymap::Keymap;
use windows::core::{Result, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ID_OPEN: u32 = 100;
pub const ID_RELOAD: u32 = 101;
pub const ID_EXIT: u32 = 102;
pub const ID_FOLLOW: u32 = 103;
pub const ID_NEW: u32 = 104;
pub const ID_SAVE: u32 = 105;
pub const ID_SAVE_AS: u32 = 106;
pub const ID_CLOSE_TAB: u32 = 107;
pub const ID_NEXT_TAB: u32 = 108;
pub const ID_PREV_TAB: u32 = 109;
pub const ID_KEYS: u32 = 112;
pub const ID_SPLIT: u32 = 113;
pub const ID_JOIN: u32 = 114;
pub const ID_COPY: u32 = 200;
pub const ID_SELECT_ALL: u32 = 201;
pub const ID_UNDO: u32 = 202;
pub const ID_REDO: u32 = 203;
pub const ID_CUT: u32 = 204;
pub const ID_PASTE: u32 = 205;
pub const ID_ADD_NEXT: u32 = 206;
pub const ID_FIND: u32 = 300;
pub const ID_FIND_NEXT: u32 = 301;
pub const ID_FIND_PREV: u32 = 302;
pub const ID_CASE: u32 = 303;
pub const ID_REGEX: u32 = 304;
pub const ID_GOTO: u32 = 305;
pub const ID_REPLACE: u32 = 306;
pub const ID_FIND_ALL: u32 = 307;
pub const ID_GREP: u32 = 308;
pub const ID_NEXT_RESULT: u32 = 309;
pub const ID_PREV_RESULT: u32 = 310;
pub const ID_CLOSE_RESULTS: u32 = 311;
pub const ID_GREP_REPLACE: u32 = 312;
pub const ID_GREP_BACKUP: u32 = 313;
pub const ID_RESULTS_TAB: u32 = 314;
pub const ID_CSV: u32 = 600;
pub const ID_CSV_HEADER: u32 = 601;
pub const ID_SORT_ASC: u32 = 602;
pub const ID_SORT_DESC: u32 = 603;
pub const ID_FILTER: u32 = 604;
pub const ID_WHITESPACE: u32 = 620;
pub const ID_LOG_COLORS: u32 = 621;
pub const ID_DARK: u32 = 622;
pub const ID_ZOOM_IN: u32 = 623;
pub const ID_ZOOM_OUT: u32 = 624;
pub const ID_ZOOM_RESET: u32 = 625;
pub const ID_WRAP: u32 = 626;
pub const ID_IME_INLINE: u32 = 627;
pub const ID_SYNTAX: u32 = 628;
/// 区切り文字の選択肢（DELIMS の並びと対応）
pub const ID_DELIM_BASE: u32 = 610;
pub const DELIMS: [(&str, char); 4] = [("カンマ ( , )", ','), ("タブ", '\t'), ("セミコロン ( ; )", ';'), ("縦棒 ( | )", '|')];

/// エンコードの選択肢（ENCODINGS の並びと対応）
pub const ID_ENC_BASE: u32 = 400;
/// 「文字コードを変えて保存」の選択肢（CONV_ENCODINGS の並びと対応）
pub const ID_CONV_ENC_BASE: u32 = 700;
/// 「改行コードを変えて保存」の選択肢（CONV_NEWLINES の並びと対応）
pub const ID_CONV_NL_BASE: u32 = 720;
/// (表示名, 文字コード, BOM を付ける)
pub const CONV_ENCODINGS: [(&str, &Encoding, bool); 6] = [
    ("UTF-8", encoding_rs::UTF_8, false),
    ("UTF-8 (BOM 付き)", encoding_rs::UTF_8, true),
    ("Shift_JIS (CP932)", encoding_rs::SHIFT_JIS, false),
    ("EUC-JP", encoding_rs::EUC_JP, false),
    ("UTF-16LE (BOM 付き)", encoding_rs::UTF_16LE, true),
    ("UTF-16BE (BOM 付き)", encoding_rs::UTF_16BE, true),
];
pub const CONV_NEWLINES: [(&str, &str); 3] = [("CRLF (Windows)", "\r\n"), ("LF (Unix)", "\n"), ("CR (旧 Mac)", "\r")];

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
    pub view: HMENU,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// メニュー項目。キーの表示（`\t` の後ろ）は keys.ini の設定に合わせる
unsafe fn item(km: &Keymap, menu: HMENU, id: u32, text: &str) -> Result<()> {
    let text = match km.label(id) {
        Some("") => text.split('\t').next().unwrap_or(text).to_string(),
        Some(k) => format!("{}\t{k}", text.split('\t').next().unwrap_or(text)),
        None => text.to_string(),
    };
    let w = wide(&text);
    unsafe { AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(w.as_ptr())) }
}

unsafe fn sep(menu: HMENU) -> Result<()> {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
}

unsafe fn popup(bar: HMENU, sub: HMENU, text: &str) -> Result<()> {
    let w = wide(text);
    unsafe { AppendMenuW(bar, MF_POPUP, sub.0 as usize, PCWSTR(w.as_ptr())) }
}

pub unsafe fn build(km: &Keymap) -> Result<Menus> {
    unsafe {
        let bar = CreateMenu()?;

        let file = CreatePopupMenu()?;
        let recent = CreatePopupMenu()?;
        item(km, file, ID_NEW, "新しいタブ(&N)\tCtrl+N")?;
        item(km, file, ID_OPEN, "開く(&O)...\tCtrl+O")?;
        item(km, file, ID_SAVE, "上書き保存(&S)\tCtrl+S")?;
        item(km, file, ID_SAVE_AS, "名前を付けて保存(&A)...\tCtrl+Shift+S")?;
        let conv = CreatePopupMenu()?;
        for (i, (name, _, _)) in CONV_ENCODINGS.iter().enumerate() {
            item(km, conv, ID_CONV_ENC_BASE + i as u32, &format!("文字コードを {name} にして保存..."))?;
        }
        sep(conv)?;
        for (i, (name, _)) in CONV_NEWLINES.iter().enumerate() {
            item(km, conv, ID_CONV_NL_BASE + i as u32, &format!("改行コードを {name} にして保存..."))?;
        }
        popup(file, conv, "文字コード・改行コードを変えて保存(&V)")?;
        sep(file)?;
        item(km, file, ID_RELOAD, "再読み込み(&R)\tF5")?;
        item(km, file, ID_FOLLOW, "末尾を追従(&T)\tCtrl+T")?;
        sep(file)?;
        popup(file, recent, "最近使ったファイル(&F)")?;
        sep(file)?;
        item(km, file, ID_CLOSE_TAB, "タブを閉じる(&C)\tCtrl+W")?;
        item(km, file, ID_SPLIT, "ファイルを分割(&L)...")?;
        item(km, file, ID_JOIN, "ファイルを結合(&J)...")?;
        item(km, file, ID_KEYS, "キー割り当てを編集(&K)...")?;
        item(km, file, ID_EXIT, "終了(&X)")?;
        popup(bar, file, "ファイル(&F)")?;

        let edit = CreatePopupMenu()?;
        item(km, edit, ID_UNDO, "元に戻す(&U)\tCtrl+Z")?;
        item(km, edit, ID_REDO, "やり直し(&R)\tCtrl+Y")?;
        sep(edit)?;
        item(km, edit, ID_CUT, "切り取り(&T)\tCtrl+X")?;
        item(km, edit, ID_COPY, "コピー(&C)\tCtrl+C")?;
        item(km, edit, ID_PASTE, "貼り付け(&P)\tCtrl+V")?;
        item(km, edit, ID_SELECT_ALL, "すべて選択(&A)\tCtrl+A")?;
        sep(edit)?;
        item(km, edit, ID_ADD_NEXT, "次の同じ文字列を選択に追加(&D)\tCtrl+D")?;
        popup(bar, edit, "編集(&E)")?;

        let search = CreatePopupMenu()?;
        item(km, search, ID_FIND, "検索(&F)...\tCtrl+F")?;
        item(km, search, ID_FIND_NEXT, "次を検索(&N)\tF3")?;
        item(km, search, ID_FIND_PREV, "前を検索(&P)\tShift+F3")?;
        item(km, search, ID_FIND_ALL, "すべて検索して一覧(&L)\t検索欄で Alt+Enter")?;
        item(km, search, ID_REPLACE, "置換(&H)...\tCtrl+H")?;
        sep(search)?;
        item(km, search, ID_GREP, "フォルダ内を検索 (GREP)(&G)...\tCtrl+Shift+F")?;
        item(km, search, ID_GREP_REPLACE, "GREP の結果を置換(&E)...")?;
        item(km, search, ID_GREP_BACKUP, "GREP 置換の前にバックアップ (.bak) を作る(&B)")?;
        item(km, search, ID_NEXT_RESULT, "次の結果(&X)\tF8")?;
        item(km, search, ID_PREV_RESULT, "前の結果(&V)\tShift+F8")?;
        item(km, search, ID_RESULTS_TAB, "結果の一覧を新しいタブに出す(&T)")?;
        item(km, search, ID_CLOSE_RESULTS, "結果一覧を閉じる\tEsc")?;
        sep(search)?;
        item(km, search, ID_CASE, "大文字と小文字を区別(&C)\tAlt+C")?;
        item(km, search, ID_REGEX, "正規表現(&R)\tAlt+R")?;
        sep(search)?;
        item(km, search, ID_GOTO, "行へ移動(&G)...\tCtrl+G")?;
        popup(bar, search, "検索(&S)")?;

        let view = CreatePopupMenu()?;
        item(km, view, ID_CSV, "CSV/TSV モード(&C)\tCtrl+Shift+C")?;
        sep(view)?;
        for (i, (name, _)) in DELIMS.iter().enumerate() {
            item(km, view, ID_DELIM_BASE + i as u32, &format!("区切り: {name}"))?;
        }
        item(km, view, ID_CSV_HEADER, "1 行目は見出し(&H)")?;
        sep(view)?;
        item(km, view, ID_SORT_ASC, "カーソルの列で昇順に並べ替え(&A)")?;
        item(km, view, ID_SORT_DESC, "カーソルの列で降順に並べ替え(&D)")?;
        item(km, view, ID_FILTER, "列フィルタ(&F)...")?;
        sep(view)?;
        item(km, view, ID_WHITESPACE, "空白・タブ・改行を表示(&W)\tCtrl+Shift+W")?;
        item(km, view, ID_WRAP, "折り返して表示(&R)")?;
        item(km, view, ID_IME_INLINE, "IME の変換中の文字をエディタ内に表示(&M)")?;
        item(km, view, ID_SYNTAX, "構文の色分け（JSON・SQL・XML など）(&Y)")?;
        item(km, view, ID_LOG_COLORS, "ログレベルの色分け(&L)")?;
        item(km, view, ID_DARK, "ダークテーマ(&K)")?;
        sep(view)?;
        item(km, view, ID_ZOOM_IN, "拡大(&I)\tCtrl++")?;
        item(km, view, ID_ZOOM_OUT, "縮小(&O)\tCtrl+-")?;
        item(km, view, ID_ZOOM_RESET, "標準の大きさ(&0)\tCtrl+0")?;
        sep(view)?;
        item(km, view, ID_NEXT_TAB, "次のタブ(&N)\tCtrl+Tab")?;
        item(km, view, ID_PREV_TAB, "前のタブ(&P)\tCtrl+Shift+Tab")?;
        popup(bar, view, "表示(&V)")?;

        let encoding = CreatePopupMenu()?;
        for (i, (name, _)) in ENCODINGS.iter().enumerate() {
            if i == 1 {
                sep(encoding)?;
            }
            item(km, encoding, ID_ENC_BASE + i as u32, &format!("{name} で開き直す"))?;
        }
        popup(bar, encoding, "エンコード(&N)")?;

        Ok(Menus { bar, file, search, encoding, recent, view })
    }
}

impl Menus {
    /// 表示メニューのチェック状態（`csv` は CSV モードのときの (区切り, 見出しあり)）
    pub unsafe fn refresh_view(&self, csv: Option<(char, bool)>, s: &crate::settings::Settings) {
        unsafe {
            let check = |on: bool| if on { MF_CHECKED.0 } else { MF_UNCHECKED.0 };
            CheckMenuItem(self.view, ID_WHITESPACE, check(s.whitespace));
            CheckMenuItem(self.view, ID_LOG_COLORS, check(s.log_colors));
            CheckMenuItem(self.view, ID_SYNTAX, check(s.syntax));
            CheckMenuItem(self.view, ID_WRAP, check(s.wrap));
            CheckMenuItem(self.view, ID_IME_INLINE, check(s.ime_inline));
            CheckMenuItem(self.search, ID_GREP_BACKUP, check(s.grep_backup));
            CheckMenuItem(self.view, ID_DARK, check(s.dark));
            CheckMenuItem(self.view, ID_CSV, check(csv.is_some()));
            for (i, (_, d)) in DELIMS.iter().enumerate() {
                let id = ID_DELIM_BASE + i as u32;
                CheckMenuItem(self.view, id, check(csv.is_some_and(|(cd, _)| cd == *d)));
            }
            CheckMenuItem(self.view, ID_CSV_HEADER, check(csv.is_some_and(|(_, h)| h)));
            let en = if csv.is_some() { MF_ENABLED } else { MF_GRAYED };
            for id in [ID_CSV_HEADER, ID_SORT_ASC, ID_SORT_DESC] {
                let _ = EnableMenuItem(self.view, id, en);
            }
        }
    }

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
                let _ = item(&Keymap::default(), self.recent, 0, "（なし）");
                let _ = EnableMenuItem(self.recent, 0, MF_BYPOSITION | MF_GRAYED);
            }
            for (i, p) in recent.iter().enumerate() {
                let label = format!("&{} {}", (i + 1) % 10, p.display());
                let _ = item(&Keymap::default(), self.recent, ID_RECENT_BASE + i as u32, &label);
            }
        }
    }
}
