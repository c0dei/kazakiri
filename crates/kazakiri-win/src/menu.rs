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
pub const ID_SETTINGS: u32 = 115;
pub const ID_LINE_DUP: u32 = 800;
pub const ID_LINE_DEL: u32 = 801;
pub const ID_LINE_UP: u32 = 802;
pub const ID_LINE_DOWN: u32 = 803;
pub const ID_DEL_BOL: u32 = 804;
pub const ID_DEL_EOL: u32 = 805;
pub const ID_INDENT: u32 = 806;
pub const ID_UNINDENT: u32 = 807;
pub const ID_TRIM: u32 = 808;
pub const ID_DEDUPE: u32 = 809;
pub const ID_SORT_LINES_ASC: u32 = 810;
pub const ID_SORT_LINES_DESC: u32 = 811;
pub const ID_INSERT_DATE: u32 = 812;
pub const ID_INSERT_DATETIME: u32 = 813;
/// 「変換」メニューの項目名（app/lines.rs の CONVS の並びと対応）
pub const CONV_NAMES: [&str; 20] = [
    "大文字にする(&U)",
    "小文字にする(&L)",
    "全角 → 半角（英数字・記号）(&H)",
    "半角 → 全角（英数字・記号）(&Z)",
    "全角カタカナ → 半角カタカナ(&K)",
    "半角カタカナ → 全角カタカナ(&J)",
    "ひらがな → カタカナ(&A)",
    "カタカナ → ひらがな(&G)",
    "タブ → 空白(&S)",
    "行頭の空白 → タブ(&T)",
    "JSON を整形（展開）(&E)",
    "JSON を圧縮（短縮）(&M)",
    "行頭の空白を削除(&B)",
    "各単語の頭を大文字にする(&W)",
    "HTML の文字参照にする（& < > \" '）(&X)",
    "HTML の文字参照を文字に戻す(&Y)",
    "Base64 にする(&5)",
    "Base64 から戻す(&6)",
    "URL エンコード（%XX）(&Q)",
    "URL デコード(&D)",
];
/// 「変換」メニューの項目（CONV_NAMES の並びと対応）
pub const ID_BM_TOGGLE: u32 = 840;
pub const ID_BM_NEXT: u32 = 841;
pub const ID_BM_PREV: u32 = 842;
pub const ID_BM_CLEAR: u32 = 843;
pub const ID_BM_LIST: u32 = 844;
pub const ID_TOOLBAR: u32 = 846;
pub const ID_FKEYS: u32 = 847;
pub const ID_OVERWRITE: u32 = 848;
/// ステータスバーのタブ幅のメニュー（TAB_WIDTHS の並び）
pub const ID_TABW_BASE: u32 = 850;
pub const TAB_WIDTHS: [usize; 4] = [2, 3, 4, 8];
pub const ID_CLIP_BASE: u32 = 900;
pub const CLIP_MAX: u32 = 20;
pub const ID_COMPLETE: u32 = 849;
pub const ID_MARK_BASE: u32 = 860;
pub const ID_MARK_CLEAR: u32 = 866;
pub const ID_DIFF: u32 = 870;
pub const ID_SPLIT_VIEW: u32 = 871;
pub const ID_MD_SPLIT: u32 = 963;
pub const ID_MD_DESIGN: u32 = 964;
pub const ID_MD_EXPORT: u32 = 965;
pub const ID_FILTER_LINES: u32 = 872;
pub const ID_INS_ABOVE: u32 = 873;
pub const ID_INS_BELOW: u32 = 874;
pub const ID_INS_SEQ: u32 = 875;
pub const ID_INS_TAB: u32 = 876;
pub const ID_INS_CRLF: u32 = 877;
pub const ID_INS_LF: u32 = 878;
pub const ID_INS_CR: u32 = 879;
/// 行の並べ替え（app/lines.rs の SORT_KINDS と同じ並び）
pub const ID_SORTK_BASE: u32 = 880;
pub const SORT_NAMES: [&str; 10] = [
    "数値で昇順(&1)",
    "数値で降順(&2)",
    "文字数の少ない順(&3)",
    "文字数の多い順(&4)",
    "出現回数の少ない順(&5)",
    "出現回数の多い順(&6)",
    "IPv4 アドレスで昇順(&7)",
    "IPv4 アドレスで降順(&8)",
    "順序を逆にする(&R)",
    "ランダムに並べ替える(&M)",
];
pub const ID_LONGEST: u32 = 890;
pub const ID_SHORTEST: u32 = 891;
pub const ID_FREQUENT: u32 = 892;
pub const ID_BM_CUT: u32 = 893;
pub const ID_BM_COPY: u32 = 894;
pub const ID_BM_DELETE: u32 = 895;
pub const ID_BM_INVERT: u32 = 896;
pub const ID_BM_EXTRACT: u32 = 897;
pub const ID_DIFF_IGN_EDGE: u32 = 898;
pub const ID_DIFF_IGN_CASE: u32 = 899;
pub const ID_SPLIT_SYNC: u32 = 920;
pub const ID_CSV_LETTERS: u32 = 921;
pub const ID_DIFF_TAB_BASE: u32 = 1000;
pub const ID_MODEBAR: u32 = 922;
pub const ID_COLPICK: u32 = 923;
/// GREP の文字コードの選択肢（自動判定または指定）
pub const GREP_ENCODINGS: [(&str, Option<&Encoding>); 6] = [
    ("自動判定", None),
    ("UTF-8", Some(encoding_rs::UTF_8)),
    ("Shift_JIS (CP932)", Some(encoding_rs::SHIFT_JIS)),
    ("EUC-JP", Some(encoding_rs::EUC_JP)),
    ("UTF-16LE", Some(encoding_rs::UTF_16LE)),
    ("UTF-16BE", Some(encoding_rs::UTF_16BE)),
];
/// GREP の結果に出す内容
pub const GREP_OUTPUTS: [&str; 4] = ["ファイル名・行・一致した行", "ファイル名・行", "ファイル名だけ（一致したファイルの一覧）", "一致した行だけ"];
pub const ID_CELL_MODE: u32 = 925;
pub const ID_COL_FOLLOW: u32 = 924;
/// モードバーの「CSV」「TSV」ボタンと、区切り文字の入力欄
pub const ID_CSV_MODE: u32 = 926;
pub const ID_TSV: u32 = 927;
pub const ID_DELIM_FIELD: u32 = 928;
/// コマンドパレット（Ctrl+Shift+P）
pub const ID_PALETTE: u32 = 929;
/// ツールバー左端の ☰（メニューを開く）
pub const ID_MENU: u32 = 930;
/// 対象の列を選ぶメニューの項目（列番号 0 から 199）
pub const ID_COL_BASE: u32 = 1200;
pub const ID_TRAY_ACTIVATE: u32 = 950;
pub const ID_TRAY_NEW: u32 = 951;
pub const ID_TRAY_NEW_PASTE: u32 = 952;
pub const ID_TRAY_NEW_QUOTE: u32 = 953;
pub const ID_TRAY_OPEN: u32 = 954;
pub const ID_TRAY_GREP: u32 = 955;
pub const ID_TRAY_RESTORE: u32 = 956;
pub const ID_TRAY_SAVE_CLOSE: u32 = 957;
pub const ID_TRAY_CLOSE_ALL: u32 = 958;
pub const ID_TRAY_MESSAGE: u32 = 959;
pub const ID_TRAY_SETTINGS: u32 = 960;
pub const ID_TRAY_MINIMIZE: u32 = 961;
pub const ID_TRAY_REMOVE: u32 = 962;
pub const ID_TRAY_FOLDER_BASE: u32 = 970;
pub const ID_TRAY_TAB_BASE: u32 = 1100;
pub const ID_CONV_BASE: u32 = 1500;
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
pub const ID_RESULTS_SAVE: u32 = 315;
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
/// ステータスバーのメニューから、いまのファイルを書き換える変換（CONV_* と同じ並び）
pub const ID_HERE_ENC_BASE: u32 = 740;
pub const ID_HERE_NL_BASE: u32 = 760;
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
    pub csv: HMENU,
    /// 検索 → GREP のサブメニュー
    pub grep: HMENU,
    /// 編集 → クリップボード履歴
    pub clips: HMENU,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

thread_local! {
    /// コマンドパレットに出す、メニューのコマンド一覧（名前, キー, ID）。メニューを作るときに集める
    static PALETTE_ITEMS: std::cell::RefCell<Vec<(String, String, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// コマンドパレット用の一覧
pub fn palette_items() -> Vec<(String, String, u32)> {
    PALETTE_ITEMS.with(|p| p.borrow().clone())
}

/// メニューの文字から、パレットに出す名前を作る（「(&X)」や「&」、末尾の「...」を除く）
fn palette_name(text: &str) -> String {
    let t = text.split('\t').next().unwrap_or(text);
    let mut out = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' && chars.peek() == Some(&'&') {
            // 「(&X)」を読み飛ばす
            for d in chars.by_ref() {
                if d == ')' {
                    break;
                }
            }
        } else if c != '&' {
            out.push(c);
        }
    }
    out.trim_end_matches("...").trim().to_string()
}

/// メニュー項目。キーの表示（タブ文字の後ろ）は keys.ini の設定に合わせる
unsafe fn item(km: &Keymap, menu: HMENU, id: u32, text: &str) -> Result<()> {
    PALETTE_ITEMS.with(|p| {
        let mut p = p.borrow_mut();
        let name = palette_name(text);
        if !name.is_empty() && !p.iter().any(|e| e.2 == id) {
            let key = match km.label(id) {
                Some(k) => k.to_string(),
                None => text.split('\t').nth(1).unwrap_or("").to_string(),
            };
            p.push((name, key, id));
        }
    });
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
        item(km, file, ID_SETTINGS, "設定(&P)...")?;
        item(km, file, ID_KEYS, "キー割り当て(&K)...")?;
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
        sep(edit)?;
        let lines = CreatePopupMenu()?;
        item(km, lines, ID_LINE_DUP, "行を複製(&D)\tCtrl+Shift+D")?;
        item(km, lines, ID_LINE_DEL, "行を削除(&K)\tCtrl+Shift+K")?;
        item(km, lines, ID_LINE_UP, "行を上へ移動(&U)\tCtrl+Shift+↑")?;
        item(km, lines, ID_LINE_DOWN, "行を下へ移動(&N)\tCtrl+Shift+↓")?;
        sep(lines)?;
        item(km, lines, ID_DEL_BOL, "行頭まで削除(&B)")?;
        item(km, lines, ID_DEL_EOL, "行末まで削除(&E)")?;
        sep(lines)?;
        item(km, lines, ID_INDENT, "インデント(&I)\tTab（複数行を選択）")?;
        item(km, lines, ID_UNINDENT, "逆インデント(&O)\tShift+Tab")?;
        sep(lines)?;
        item(km, lines, ID_TRIM, "行末の空白を削除(&T)")?;
        item(km, lines, ID_DEDUPE, "重複した行を削除(&P)")?;
        item(km, lines, ID_SORT_LINES_ASC, "行を昇順に並べ替え(&A)")?;
        item(km, lines, ID_SORT_LINES_DESC, "行を降順に並べ替え(&Z)")?;
        let sortk = CreatePopupMenu()?;
        for (i, name) in SORT_NAMES.iter().enumerate() {
            item(km, sortk, ID_SORTK_BASE + i as u32, name)?;
        }
        popup(lines, sortk, "並べ替えの種類(&S)")?;
        popup(edit, lines, "行の操作(&L)")?;
        let insert = CreatePopupMenu()?;
        item(km, insert, ID_INSERT_DATE, "日付(&D)")?;
        item(km, insert, ID_INSERT_DATETIME, "日付と時刻(&T)")?;
        sep(insert)?;
        item(km, insert, ID_INS_ABOVE, "上に空行を挿入(&A)\tCtrl+Shift+Enter")?;
        item(km, insert, ID_INS_BELOW, "下に空行を挿入(&W)\tCtrl+Enter")?;
        item(km, insert, ID_INS_SEQ, "連番を挿入(&N)...")?;
        sep(insert)?;
        item(km, insert, ID_INS_CRLF, "CR+LF(&R)")?;
        item(km, insert, ID_INS_LF, "LF のみ(&L)")?;
        item(km, insert, ID_INS_CR, "CR のみ(&Y)")?;
        item(km, insert, ID_INS_TAB, "タブ(&B)")?;
        popup(edit, insert, "挿入(&I)")?;
        let clips = CreatePopupMenu()?;
        let w = wide("（履歴なし）");
        AppendMenuW(clips, MF_STRING | MF_GRAYED, 0, PCWSTR(w.as_ptr()))?;
        popup(edit, clips, "クリップボード履歴(&H)")?;
        item(km, edit, ID_COMPLETE, "単語を補完(&W)\tCtrl+Space")?;
        popup(bar, edit, "編集(&E)")?;

        let convert = CreatePopupMenu()?;
        for (i, name) in CONV_NAMES.iter().enumerate() {
            if matches!(i, 2 | 4 | 8 | 10 | 12 | 14 | 18) {
                sep(convert)?;
            }
            item(km, convert, ID_CONV_BASE + i as u32, name)?;
        }
        popup(bar, convert, "変換(&C)")?;

        let search = CreatePopupMenu()?;
        item(km, search, ID_FIND, "検索(&F)...\tCtrl+F")?;
        item(km, search, ID_FIND_NEXT, "次を検索(&N)\tF3")?;
        item(km, search, ID_FIND_PREV, "前を検索(&P)\tShift+F3")?;
        item(km, search, ID_FIND_ALL, "すべて検索して一覧(&L)\t検索欄で Alt+Enter")?;
        item(km, search, ID_REPLACE, "置換(&H)...\tCtrl+H")?;
        sep(search)?;
        item(km, search, ID_CASE, "大文字と小文字を区別(&C)\tAlt+C")?;
        item(km, search, ID_REGEX, "正規表現(&R)\tAlt+R")?;
        item(km, search, ID_LONGEST, "最長の行を検索(&2)")?;
        item(km, search, ID_SHORTEST, "最短の行（空行を除く）を検索(&3)")?;
        item(km, search, ID_FREQUENT, "頻出する行を抽出（新しいタブ）(&4)")?;
        item(km, search, ID_FILTER_LINES, "行フィルター（一致した行だけを新しいタブに）(&I)\tCtrl+Shift+L")?;
        sep(search)?;
        // フォルダ内を検索（GREP）
        let grep = CreatePopupMenu()?;
        item(km, grep, ID_GREP, "フォルダ内を検索(&G)...\tCtrl+Shift+F")?;
        item(km, grep, ID_GREP_REPLACE, "結果を置換(&R)...")?;
        item(km, grep, ID_GREP_BACKUP, "置換の前にバックアップ (.bak) を作る(&B)")?;
        popup(search, grep, "GREP（フォルダ内を検索）(&E)")?;
        // 検索結果の一覧
        let results = CreatePopupMenu()?;
        item(km, results, ID_NEXT_RESULT, "次の結果(&N)\tF8")?;
        item(km, results, ID_PREV_RESULT, "前の結果(&P)\tShift+F8")?;
        sep(results)?;
        item(km, results, ID_RESULTS_TAB, "新しいタブに出す(&T)")?;
        item(km, results, ID_RESULTS_SAVE, "ファイルに保存(&S)...")?;
        item(km, results, ID_CLOSE_RESULTS, "閉じる(&C)\tEsc")?;
        popup(search, results, "検索結果(&X)")?;
        sep(search)?;
        item(km, search, ID_GOTO, "行へ移動(&J)...\tCtrl+G")?;
        let bm = CreatePopupMenu()?;
        item(km, bm, ID_BM_TOGGLE, "設定／解除(&T)\tCtrl+F2")?;
        item(km, bm, ID_BM_NEXT, "次のブックマーク(&N)\tF2")?;
        item(km, bm, ID_BM_PREV, "前のブックマーク(&P)\tShift+F2")?;
        item(km, bm, ID_BM_CLEAR, "すべて解除(&C)\tCtrl+Shift+F2")?;
        item(km, bm, ID_BM_LIST, "一覧(&L)")?;
        item(km, bm, ID_BM_INVERT, "ブックマークを反転(&I)")?;
        sep(bm)?;
        item(km, bm, ID_BM_CUT, "ブックマークした行を切り取り(&X)")?;
        item(km, bm, ID_BM_COPY, "ブックマークした行をコピー(&O)")?;
        item(km, bm, ID_BM_DELETE, "ブックマークした行を削除(&D)")?;
        item(km, bm, ID_BM_EXTRACT, "ブックマークした行を新しいタブに抽出(&E)")?;
        popup(search, bm, "ブックマーク(&M)")?;
        let mk = CreatePopupMenu()?;
        for (i, name) in ["黄", "緑", "青", "ピンク", "オレンジ", "紫"].iter().enumerate() {
            item(km, mk, ID_MARK_BASE + i as u32, &format!("選んだ文字列を{name}でマーク(&{})", i + 1))?;
        }
        sep(mk)?;
        item(km, mk, ID_MARK_CLEAR, "マークをすべて外す(&C)")?;
        popup(search, mk, "カラーマーカー(&K)")?;
        sep(search)?;
        item(km, search, ID_DIFF, "他のタブと比較（差分表示）(&D)")?;
        item(km, search, ID_DIFF_IGN_EDGE, "比較で行頭・行末の空白を無視(&G)")?;
        item(km, search, ID_DIFF_IGN_CASE, "比較で大文字と小文字を区別しない(&I)")?;
        popup(bar, search, "検索(&S)")?;

        // CSV / TSV
        let csv = CreatePopupMenu()?;
        item(km, csv, ID_CSV, "CSV/TSV を表として表示(&C)\tCtrl+Shift+C")?;
        item(km, csv, ID_CSV_HEADER, "1 行目は見出し(&H)")?;
        item(km, csv, ID_CELL_MODE, "セル選択モード（ドラッグでセルの四角を選ぶ）(&M)")?;
        item(km, csv, ID_COLPICK, "対象の列を選ぶ（並べ替え・列フィルタ）(&T)...")?;
        item(km, csv, ID_CSV_LETTERS, "列の位置を A, B, C… で表示(&L)")?;
        sep(csv)?;
        for (i, (name, _)) in DELIMS.iter().enumerate() {
            item(km, csv, ID_DELIM_BASE + i as u32, &format!("区切り: {name}"))?;
        }
        sep(csv)?;
        item(km, csv, ID_SORT_ASC, "カーソルの列で昇順に並べ替え(&A)")?;
        item(km, csv, ID_SORT_DESC, "カーソルの列で降順に並べ替え(&D)")?;
        item(km, csv, ID_FILTER, "列フィルタ(&F)...")?;
        popup(bar, csv, "CSV(&D)")?;

        let view = CreatePopupMenu()?;
        item(km, view, ID_WHITESPACE, "空白・タブ・改行を表示(&W)\tCtrl+Shift+W")?;
        item(km, view, ID_WRAP, "折り返して表示(&R)")?;
        item(km, view, ID_SYNTAX, "構文の色分け（JSON・SQL・XML など）(&Y)")?;
        item(km, view, ID_LOG_COLORS, "ログレベルの色分け(&L)")?;
        item(km, view, ID_IME_INLINE, "IME の変換中の文字をエディタ内に表示(&M)")?;
        sep(view)?;
        item(km, view, ID_SPLIT_VIEW, "分割表示（同じファイルを左右に並べて参照）(&P)")?;
        item(km, view, ID_SPLIT_SYNC, "分割表示で同期スクロール(&Q)")?;
        item(km, view, ID_MD_SPLIT, "Markdown プレビュー（右に並べる）(&V)\tCtrl+Shift+V")?;
        item(km, view, ID_MD_DESIGN, "Markdown デザインビュー（整形して表示）(&G)\tCtrl+Shift+G")?;
        item(km, view, ID_MD_EXPORT, "Markdown を HTML に書き出し(&H)...")?;
        item(km, view, ID_PALETTE, "コマンドパレット(&P)...\tCtrl+Shift+P")?;
        sep(view)?;
        item(km, view, ID_TOOLBAR, "ツールバーを表示(&B)")?;
        item(km, view, ID_MODEBAR, "モードバー（CSV の区切りなど）を表示(&D)")?;
        item(km, view, ID_FKEYS, "機能ボタンの列を表示(&U)")?;
        item(km, view, ID_DARK, "ダークテーマ(&K)")?;
        sep(view)?;
        item(km, view, ID_ZOOM_IN, "拡大(&I)\tCtrl++")?;
        item(km, view, ID_ZOOM_OUT, "縮小(&O)\tCtrl+-")?;
        item(km, view, ID_ZOOM_RESET, "標準の大きさ(&0)\tCtrl+0")?;
        sep(view)?;
        item(km, view, ID_NEXT_TAB, "次のタブ(&N)\tCtrl+Tab")?;
        item(km, view, ID_PREV_TAB, "前のタブ(&T)\tCtrl+Shift+Tab")?;
        popup(bar, view, "表示(&V)")?;

        let encoding = CreatePopupMenu()?;
        for (i, (name, _)) in ENCODINGS.iter().enumerate() {
            if i == 1 {
                sep(encoding)?;
            }
            item(km, encoding, ID_ENC_BASE + i as u32, &format!("{name} で開き直す"))?;
        }
        popup(bar, encoding, "エンコード(&N)")?;

        Ok(Menus { bar, file, search, encoding, recent, view, csv, grep, clips })
    }
}

/// 本文を右クリックしたときのメニュー（EmEditor を参考にした）。使い終わったら呼び出し側が DestroyMenu する
pub unsafe fn build_context(km: &Keymap) -> Result<HMENU> {
    unsafe {
        let m = CreatePopupMenu()?;
        item(km, m, ID_MARK_BASE, "カラーマーカーに追加／解除(&M)")?;
        item(km, m, ID_FILTER_LINES, "行フィルター（一致した行だけを新しいタブに）(&F)")?;
        item(km, m, ID_ADD_NEXT, "次の同じ文字列を選択に追加(&X)\tCtrl+D")?;
        sep(m)?;
        item(km, m, ID_UNDO, "元に戻す(&U)\tCtrl+Z")?;
        item(km, m, ID_REDO, "やり直し(&R)\tCtrl+Y")?;
        sep(m)?;
        item(km, m, ID_CUT, "切り取り(&T)\tCtrl+X")?;
        item(km, m, ID_COPY, "コピー(&C)\tCtrl+C")?;
        item(km, m, ID_PASTE, "貼り付け(&P)\tCtrl+V")?;
        item(km, m, ID_SELECT_ALL, "すべて選択(&A)\tCtrl+A")?;
        sep(m)?;
        let convert = CreatePopupMenu()?;
        for (i, name) in CONV_NAMES.iter().enumerate() {
            if matches!(i, 2 | 4 | 8 | 10 | 12 | 14 | 18) {
                sep(convert)?;
            }
            item(km, convert, ID_CONV_BASE + i as u32, name)?;
        }
        popup(m, convert, "選択範囲の変換(&L)")?;
        let lines = CreatePopupMenu()?;
        item(km, lines, ID_LINE_DUP, "行を複製(&D)\tCtrl+Shift+D")?;
        item(km, lines, ID_LINE_DEL, "行を削除(&K)\tCtrl+Shift+K")?;
        item(km, lines, ID_LINE_UP, "行を上へ移動(&U)")?;
        item(km, lines, ID_LINE_DOWN, "行を下へ移動(&N)")?;
        sep(lines)?;
        item(km, lines, ID_INDENT, "インデント(&I)")?;
        item(km, lines, ID_UNINDENT, "逆インデント(&O)\tShift+Tab")?;
        sep(lines)?;
        item(km, lines, ID_TRIM, "行末の空白を削除(&T)")?;
        item(km, lines, ID_DEDUPE, "重複した行を削除(&P)")?;
        item(km, lines, ID_SORT_LINES_ASC, "行を昇順に並べ替え(&A)")?;
        item(km, lines, ID_SORT_LINES_DESC, "行を降順に並べ替え(&Z)")?;
        let sortk = CreatePopupMenu()?;
        for (i, name) in SORT_NAMES.iter().enumerate() {
            item(km, sortk, ID_SORTK_BASE + i as u32, name)?;
        }
        popup(lines, sortk, "並べ替えの種類(&S)")?;
        popup(m, lines, "行の操作(&N)")?;
        let bm = CreatePopupMenu()?;
        item(km, bm, ID_BM_TOGGLE, "設定／解除(&T)\tCtrl+F2")?;
        item(km, bm, ID_BM_NEXT, "次のブックマーク(&N)\tF2")?;
        item(km, bm, ID_BM_PREV, "前のブックマーク(&P)\tShift+F2")?;
        item(km, bm, ID_BM_CLEAR, "すべて解除(&C)\tCtrl+Shift+F2")?;
        item(km, bm, ID_BM_LIST, "一覧(&L)")?;
        item(km, bm, ID_BM_INVERT, "ブックマークを反転(&I)")?;
        sep(bm)?;
        item(km, bm, ID_BM_CUT, "ブックマークした行を切り取り(&X)")?;
        item(km, bm, ID_BM_COPY, "ブックマークした行をコピー(&O)")?;
        item(km, bm, ID_BM_DELETE, "ブックマークした行を削除(&D)")?;
        item(km, bm, ID_BM_EXTRACT, "ブックマークした行を新しいタブに抽出(&E)")?;
        popup(m, bm, "ブックマーク(&B)")?;
        let csv = CreatePopupMenu()?;
        item(km, csv, ID_CSV, "CSV/TSV を表として表示(&C)")?;
        item(km, csv, ID_CSV_HEADER, "1 行目は見出し(&H)")?;
        sep(csv)?;
        item(km, csv, ID_SORT_ASC, "カーソルの列で昇順に並べ替え(&A)")?;
        item(km, csv, ID_SORT_DESC, "カーソルの列で降順に並べ替え(&D)")?;
        item(km, csv, ID_FILTER, "列フィルタ(&F)...")?;
        popup(m, csv, "CSV(&V)")?;
        let ins = CreatePopupMenu()?;
        item(km, ins, ID_INSERT_DATE, "日付(&D)")?;
        item(km, ins, ID_INSERT_DATETIME, "日付と時刻(&T)")?;
        item(km, ins, ID_INS_ABOVE, "上に空行を挿入(&A)")?;
        item(km, ins, ID_INS_BELOW, "下に空行を挿入(&W)")?;
        item(km, ins, ID_INS_SEQ, "連番を挿入(&N)...")?;
        popup(m, ins, "挿入(&I)")?;
        sep(m)?;
        item(km, m, ID_FIND, "検索(&F)...\tCtrl+F")?;
        item(km, m, ID_REPLACE, "置換(&H)...\tCtrl+H")?;
        item(km, m, ID_GOTO, "行へ移動(&G)...\tCtrl+G")?;
        Ok(m)
    }
}

impl Menus {
    /// 表示メニューのチェック状態（`csv` は CSV モードのときの (区切り, 見出しあり)）
    pub unsafe fn refresh_view(&self, csv: Option<(char, bool)>, split: bool, flags: [bool; 5], md: (bool, bool), s: &crate::settings::Settings) {
        unsafe {
            let check = |on: bool| if on { MF_CHECKED.0 } else { MF_UNCHECKED.0 };
            CheckMenuItem(self.view, ID_WHITESPACE, check(s.whitespace));
            CheckMenuItem(self.view, ID_LOG_COLORS, check(s.log_colors));
            CheckMenuItem(self.view, ID_SYNTAX, check(s.syntax));
            CheckMenuItem(self.view, ID_SPLIT_VIEW, check(split));
            CheckMenuItem(self.view, ID_MD_SPLIT, check(md.0));
            CheckMenuItem(self.view, ID_MD_DESIGN, check(md.1));
            // flags: 比較で空白を無視 / 大文字小文字を無視 / 同期スクロール / 列を A,B,C で表示
            CheckMenuItem(self.search, ID_DIFF_IGN_EDGE, check(flags[0]));
            CheckMenuItem(self.search, ID_DIFF_IGN_CASE, check(flags[1]));
            CheckMenuItem(self.view, ID_SPLIT_SYNC, check(flags[2]));
            CheckMenuItem(self.csv, ID_CSV_LETTERS, check(flags[3]));
            CheckMenuItem(self.csv, ID_CELL_MODE, check(flags[4]));
            CheckMenuItem(self.view, ID_TOOLBAR, check(s.toolbar));
            CheckMenuItem(self.view, ID_MODEBAR, check(s.modebar));
            CheckMenuItem(self.view, ID_FKEYS, check(s.fkeys));
            CheckMenuItem(self.view, ID_WRAP, check(s.wrap));
            CheckMenuItem(self.view, ID_IME_INLINE, check(s.ime_inline));
            CheckMenuItem(self.grep, ID_GREP_BACKUP, check(s.grep_backup));
            CheckMenuItem(self.view, ID_DARK, check(s.dark));
            CheckMenuItem(self.csv, ID_CSV, check(csv.is_some()));
            for (i, (_, d)) in DELIMS.iter().enumerate() {
                let id = ID_DELIM_BASE + i as u32;
                CheckMenuItem(self.csv, id, check(csv.is_some_and(|(cd, _)| cd == *d)));
            }
            CheckMenuItem(self.csv, ID_CSV_HEADER, check(csv.is_some_and(|(_, h)| h)));
            let en = if csv.is_some() { MF_ENABLED } else { MF_GRAYED };
            for id in [ID_CSV_HEADER, ID_SORT_ASC, ID_SORT_DESC] {
                let _ = EnableMenuItem(self.csv, id, en);
            }
        }
    }

    /// クリップボード履歴のサブメニューを作り直す
    pub unsafe fn refresh_clips(&self, clips: &[String]) {
        unsafe {
            while GetMenuItemCount(Some(self.clips)) > 0 {
                if DeleteMenu(self.clips, 0, MF_BYPOSITION).is_err() {
                    break;
                }
            }
            if clips.is_empty() {
                let w = wide("（履歴なし）");
                let _ = AppendMenuW(self.clips, MF_STRING | MF_GRAYED, 0, PCWSTR(w.as_ptr()));
            }
            for (i, c) in clips.iter().take(CLIP_MAX as usize).enumerate() {
                // 1 行に縮める（改行・タブは空白に、長いものは省略）
                let one: String = c.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).take(48).collect();
                let more = if c.chars().count() > 48 { "…" } else { "" };
                let label = format!("&{} {}{}", if i < 9 { (i + 1).to_string() } else { "0".into() }, one.replace('&', "&&"), more);
                let w = wide(&label);
                let _ = AppendMenuW(self.clips, MF_STRING, (ID_CLIP_BASE + i as u32) as usize, PCWSTR(w.as_ptr()));
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

#[cfg(test)]
mod palette_tests {
    use super::*;

    /// パレットに出す名前：「(&X)」「&」「...」とキーの表示を除く
    #[test]
    fn palette_names_drop_mnemonics() {
        assert_eq!(palette_name("新しいタブ(&N)\tCtrl+N"), "新しいタブ");
        assert_eq!(palette_name("開く(&O)...\tCtrl+O"), "開く");
        assert_eq!(palette_name("&Save"), "Save");
        assert_eq!(palette_name("コマンドパレット(&P)...\tCtrl+Shift+P"), "コマンドパレット");
        assert_eq!(palette_name(""), "");
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    /// 範囲で受けるコマンド（BASE + 個数）が、ほかのコマンドの ID と重ならないこと
    #[test]
    fn id_ranges_do_not_overlap() {
        let ranges: [(&str, u32, u32); 16] = [
            ("ENC", ID_ENC_BASE, ENCODINGS.len() as u32),
            ("RECENT", ID_RECENT_BASE, RECENT_MAX as u32),
            ("DELIM", ID_DELIM_BASE, DELIMS.len() as u32),
            ("CONV_ENC", ID_CONV_ENC_BASE, CONV_ENCODINGS.len() as u32),
            ("CONV_NL", ID_CONV_NL_BASE, CONV_NEWLINES.len() as u32),
            ("HERE_ENC", ID_HERE_ENC_BASE, CONV_ENCODINGS.len() as u32),
            ("HERE_NL", ID_HERE_NL_BASE, CONV_NEWLINES.len() as u32),
            ("CONV", ID_CONV_BASE, CONV_NAMES.len() as u32),
            ("TABW", ID_TABW_BASE, TAB_WIDTHS.len() as u32),
            ("MARK", ID_MARK_BASE, 6),
            ("SORTK", ID_SORTK_BASE, SORT_NAMES.len() as u32),
            ("CLIP", ID_CLIP_BASE, CLIP_MAX),
            ("TRAY_FOLDER", ID_TRAY_FOLDER_BASE, 10),
            ("DIFF_TAB", ID_DIFF_TAB_BASE, 64),
            ("TRAY_TAB", ID_TRAY_TAB_BASE, 20),
            ("COL", ID_COL_BASE, 200),
        ];
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                assert!(a.1 + a.2 <= b.1 || b.1 + b.2 <= a.1, "{} と {} が重なっています", a.0, b.0);
            }
        }
        // 単独のコマンドの ID が、範囲の中に入っていないこと
        let single = [("TOOLBAR", ID_TOOLBAR), ("FKEYS", ID_FKEYS), ("OVERWRITE", ID_OVERWRITE), ("COMPLETE", ID_COMPLETE), ("MODEBAR", ID_MODEBAR), ("COLPICK", ID_COLPICK), ("MARK_CLEAR", ID_MARK_CLEAR), ("DIFF", ID_DIFF), ("SPLIT_VIEW", ID_SPLIT_VIEW), ("FILTER_LINES", ID_FILTER_LINES), ("CSV_MODE", ID_CSV_MODE), ("TSV", ID_TSV), ("DELIM_FIELD", ID_DELIM_FIELD), ("PALETTE", ID_PALETTE), ("MENU", ID_MENU)];
        for (n, id) in single {
            for r in &ranges {
                assert!(!(r.1..r.1 + r.2).contains(&id), "{n} ({id}) が {} の範囲に入っています", r.0);
            }
        }
    }
}
