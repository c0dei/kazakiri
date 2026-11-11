//! キー割り当て（%APPDATA%\kazakiri\keys.ini で変えられる）。
//!
//! 書式は `コマンド名 = キー` の行（`#` から行末まではコメント）。キーは `Ctrl+Shift+F`、`F3` のように書く。
//! 複数割り当てるときはカンマで区切り、割り当てを外すときは `none` と書く。
//! 書いたコマンドだけが既定の割り当てを置き換える。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::menu::*;

/// (コマンド名, メニューの ID)
const COMMANDS: &[(&str, u32)] = &[
    ("new", ID_NEW),
    ("open", ID_OPEN),
    ("save", ID_SAVE),
    ("save_as", ID_SAVE_AS),
    ("reload", ID_RELOAD),
    ("follow", ID_FOLLOW),
    ("close_tab", ID_CLOSE_TAB),
    ("next_tab", ID_NEXT_TAB),
    ("prev_tab", ID_PREV_TAB),
    ("exit", ID_EXIT),
    ("undo", ID_UNDO),
    ("redo", ID_REDO),
    ("cut", ID_CUT),
    ("copy", ID_COPY),
    ("paste", ID_PASTE),
    ("select_all", ID_SELECT_ALL),
    ("find", ID_FIND),
    ("find_next", ID_FIND_NEXT),
    ("find_prev", ID_FIND_PREV),
    ("find_all", ID_FIND_ALL),
    ("replace", ID_REPLACE),
    ("grep", ID_GREP),
    ("grep_replace", ID_GREP_REPLACE),
    ("next_result", ID_NEXT_RESULT),
    ("prev_result", ID_PREV_RESULT),
    ("close_results", ID_CLOSE_RESULTS),
    ("case", ID_CASE),
    ("regex", ID_REGEX),
    ("goto", ID_GOTO),
    ("csv", ID_CSV),
    ("csv_header", ID_CSV_HEADER),
    ("sort_asc", ID_SORT_ASC),
    ("sort_desc", ID_SORT_DESC),
    ("column_filter", ID_FILTER),
    ("whitespace", ID_WHITESPACE),
    ("log_colors", ID_LOG_COLORS),
    ("dark", ID_DARK),
    ("zoom_in", ID_ZOOM_IN),
    ("zoom_out", ID_ZOOM_OUT),
    ("zoom_reset", ID_ZOOM_RESET),
];

/// 既定の割り当て（同じコマンドを複数行に書いてよい）
const DEFAULTS: &[(&str, &str)] = &[
    ("new", "Ctrl+N"),
    ("open", "Ctrl+O"),
    ("save", "Ctrl+S"),
    ("save_as", "Ctrl+Shift+S"),
    ("reload", "F5"),
    ("follow", "Ctrl+T"),
    ("close_tab", "Ctrl+W"),
    ("next_tab", "Ctrl+Tab"),
    ("prev_tab", "Ctrl+Shift+Tab"),
    ("undo", "Ctrl+Z"),
    ("redo", "Ctrl+Y"),
    ("redo", "Ctrl+Shift+Z"),
    ("cut", "Ctrl+X"),
    ("cut", "Shift+Delete"),
    ("copy", "Ctrl+C"),
    ("copy", "Ctrl+Insert"),
    ("paste", "Ctrl+V"),
    ("paste", "Shift+Insert"),
    ("select_all", "Ctrl+A"),
    ("find", "Ctrl+F"),
    ("find_next", "F3"),
    ("find_prev", "Shift+F3"),
    ("replace", "Ctrl+H"),
    ("grep", "Ctrl+Shift+F"),
    ("next_result", "F8"),
    ("prev_result", "Shift+F8"),
    ("goto", "Ctrl+G"),
    ("csv", "Ctrl+Shift+C"),
    ("whitespace", "Ctrl+Shift+W"),
    ("zoom_in", "Ctrl+Plus"),
    ("zoom_out", "Ctrl+Minus"),
    ("zoom_reset", "Ctrl+0"),
];

/// 仮想キーコード
mod vk {
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const NUMPAD0: u16 = 0x60;
    pub const ADD: u16 = 0x6B;
    pub const SUBTRACT: u16 = 0x6D;
    pub const F1: u16 = 0x70;
    pub const OEM_PLUS: u16 = 0xBB;
    pub const OEM_MINUS: u16 = 0xBD;
}

const NAMED: &[(&str, &[u16])] = &[
    ("Tab", &[vk::TAB]),
    ("Enter", &[vk::RETURN]),
    ("Space", &[vk::SPACE]),
    ("PageUp", &[vk::PRIOR]),
    ("PageDown", &[vk::NEXT]),
    ("End", &[vk::END]),
    ("Home", &[vk::HOME]),
    ("Left", &[vk::LEFT]),
    ("Up", &[vk::UP]),
    ("Right", &[vk::RIGHT]),
    ("Down", &[vk::DOWN]),
    ("Insert", &[vk::INSERT]),
    ("Delete", &[vk::DELETE]),
    ("Plus", &[vk::OEM_PLUS, vk::ADD]),
    ("Minus", &[vk::OEM_MINUS, vk::SUBTRACT]),
    ("0", &[0x30, vk::NUMPAD0]),
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Combo {
    ctrl: bool,
    shift: bool,
    vk: u16,
}

#[derive(Default)]
pub struct Keymap {
    map: HashMap<Combo, u32>,
    /// コマンドの ID → 表示用のキー名（割り当てを外したコマンドは空）
    labels: HashMap<u32, String>,
    /// 設定ファイルの読み込みで見つけた問題
    pub problems: Vec<String>,
}

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("kazakiri").join("keys.ini"))
}

/// 「Ctrl+Shift+F」のような 1 つのキーを読む
fn parse_key(s: &str) -> Result<(bool, bool, Vec<u16>), String> {
    let (mut ctrl, mut shift) = (false, false);
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        // 「Ctrl++」のように + 自体を指すときは、空の部分が 2 つ続く
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" => return Err("Alt は使えません".into()),
            "" => continue,
            _ => {
                if key.is_some() {
                    return Err(format!("キーが 2 つ書かれています: {s}"));
                }
                key = Some(part.to_string());
            }
        }
    }
    let key = match key {
        Some(k) => k,
        None if s.trim_end().ends_with('+') => "Plus".into(),
        None => return Err(format!("キーがありません: {s}")),
    };
    let upper = key.to_ascii_uppercase();
    let vks: Vec<u16> = if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()).filter(|n| (1..=12).contains(n)) {
        vec![vk::F1 + n - 1]
    } else if let Some(&(_, v)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(&key)) {
        v.to_vec()
    } else if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphanumeric() {
        vec![upper.as_bytes()[0] as u16]
    } else {
        return Err(format!("知らないキー名: {key}"));
    };
    // 文字のキーが修飾なしだと、入力と区別できない
    let is_f = (vk::F1..vk::F1 + 12).contains(&vks[0]);
    if !ctrl && !is_f && !(shift && matches!(vks[0], vk::INSERT | vk::DELETE)) {
        return Err(format!("Ctrl を付けてください: {s}"));
    }
    Ok((ctrl, shift, vks))
}

fn label_of(ctrl: bool, shift: bool, key_text: &str) -> String {
    format!("{}{}{}", if ctrl { "Ctrl+" } else { "" }, if shift { "Shift+" } else { "" }, key_text)
}

impl Keymap {
    /// 既定の割り当て＋ keys.ini の内容
    pub fn load() -> Keymap {
        let text = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        Keymap::from_text(&text)
    }

    pub fn from_text(text: &str) -> Keymap {
        let mut km = Keymap::default();
        // コマンド名 → 割り当てのキー（文字列）
        let mut bindings: Vec<(&str, Vec<String>)> = Vec::new();
        for (name, _) in COMMANDS {
            bindings.push((name, DEFAULTS.iter().filter(|(n, _)| n == name).map(|(_, k)| k.to_string()).collect()));
        }
        for (no, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, keys)) = line.split_once('=') else {
                km.problems.push(format!("{} 行目: 「コマンド名 = キー」の形で書いてください", no + 1));
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let Some(slot) = bindings.iter_mut().find(|(n, _)| *n == name) else {
                km.problems.push(format!("{} 行目: 知らないコマンド名: {name}", no + 1));
                continue;
            };
            let keys = keys.trim();
            slot.1 = if keys.eq_ignore_ascii_case("none") || keys.is_empty() {
                Vec::new()
            } else {
                // 「Ctrl++」のカンマ区切りを壊さないよう、カンマだけで分ける
                keys.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect()
            };
        }
        for (name, keys) in bindings {
            let id = COMMANDS.iter().find(|(n, _)| *n == name).map(|&(_, id)| id).unwrap();
            let mut labels = Vec::new();
            for k in keys {
                match parse_key(&k) {
                    Ok((ctrl, shift, vks)) => {
                        for &v in &vks {
                            km.map.insert(Combo { ctrl, shift, vk: v }, id);
                        }
                        labels.push(canonical(&k, ctrl, shift));
                    }
                    Err(e) => km.problems.push(format!("{name}: {e}")),
                }
            }
            km.labels.insert(id, labels.join(" / "));
        }
        km
    }

    /// キーに割り当てられたコマンドの ID
    pub fn lookup(&self, ctrl: bool, shift: bool, vk: u16) -> Option<u32> {
        self.map.get(&Combo { ctrl, shift, vk }).copied()
    }

    /// メニューに出すキーの表示。このキーマップが扱わないコマンドは None
    pub fn label(&self, id: u32) -> Option<&str> {
        self.labels.get(&id).map(String::as_str)
    }
}

/// 表示用に書き方をそろえる（`ctrl+shift+f` → `Ctrl+Shift+F`）
fn canonical(k: &str, ctrl: bool, shift: bool) -> String {
    let key = k.rsplit('+').find(|p| !p.trim().is_empty()).map(str::trim).unwrap_or("");
    let key = if k.trim_end().ends_with('+') { "Plus".to_string() } else { key.to_string() };
    let shown = match NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(&key)) {
        Some((n, _)) => match *n {
            "Plus" => "+".to_string(),
            "Minus" => "-".to_string(),
            n => n.to_string(),
        },
        None => key.to_ascii_uppercase(),
    };
    label_of(ctrl, shift, &shown)
}

/// keys.ini の雛形（既定の割り当てを書き出す）
pub fn template() -> String {
    let mut s = String::from(
        "# kazakiri のキー割り当て\n# 「コマンド名 = キー」。書いたコマンドだけが既定の割り当てを置き換える。保存して再起動すると反映される。\n\
         # 複数はカンマ区切り（redo = Ctrl+Y, Ctrl+Shift+Z）。外すときは none。\n# 使えるキー: Ctrl+A〜Z, Ctrl+0〜9, F1〜F12, Tab, Enter, Home, End, PageUp, PageDown, 矢印, Insert, Delete, Plus, Minus\n# 文字キーは Ctrl を付ける（Alt は使えない）\n\n",
    );
    for (name, _) in COMMANDS {
        let keys: Vec<&str> = DEFAULTS.iter().filter(|(n, _)| n == name).map(|(_, k)| *k).collect();
        s.push_str(&format!("# {name} = {}\n", if keys.is_empty() { "none".to_string() } else { keys.join(", ") }));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let km = Keymap::from_text("");
        assert!(km.problems.is_empty(), "{:?}", km.problems);
        assert_eq!(km.lookup(true, false, b'F' as u16), Some(ID_FIND));
        assert_eq!(km.lookup(true, true, b'F' as u16), Some(ID_GREP));
        assert_eq!(km.lookup(false, true, 0x72), Some(ID_FIND_PREV));
        assert_eq!(km.lookup(true, false, vk::ADD), Some(ID_ZOOM_IN));
        assert_eq!(km.lookup(true, false, vk::OEM_MINUS), Some(ID_ZOOM_OUT));
        assert_eq!(km.label(ID_FIND), Some("Ctrl+F"));
        assert_eq!(km.label(ID_ZOOM_IN), Some("Ctrl++"));
        assert_eq!(km.label(ID_REDO), Some("Ctrl+Y / Ctrl+Shift+Z"));
        assert_eq!(km.label(ID_DARK), Some(""));
    }

    #[test]
    fn override_and_unbind() {
        let km = Keymap::from_text("find = Ctrl+E  # 変更\nnew = none\ndark = ctrl+shift+d, F9\n");
        assert!(km.problems.is_empty(), "{:?}", km.problems);
        assert_eq!(km.lookup(true, false, b'F' as u16), None);
        assert_eq!(km.lookup(true, false, b'E' as u16), Some(ID_FIND));
        assert_eq!(km.lookup(true, false, b'N' as u16), None);
        assert_eq!(km.lookup(true, true, b'D' as u16), Some(ID_DARK));
        assert_eq!(km.lookup(false, false, 0x78), Some(ID_DARK));
        assert_eq!(km.label(ID_FIND), Some("Ctrl+E"));
        assert_eq!(km.label(ID_DARK), Some("Ctrl+Shift+D / F9"));
    }

    #[test]
    fn problems_are_reported_not_fatal() {
        let km = Keymap::from_text("bogus = Ctrl+Q\nfind = Alt+F\nopen = A\nsave Ctrl+S\nundo = Ctrl+Z\n");
        assert_eq!(km.problems.len(), 4, "{:?}", km.problems);
        assert_eq!(km.lookup(true, false, b'Z' as u16), Some(ID_UNDO));
    }

    #[test]
    fn template_parses_to_defaults() {
        let t = template();
        let km = Keymap::from_text(&t);
        assert!(km.problems.is_empty());
        // コメントなので既定のまま
        assert_eq!(km.lookup(true, false, b'O' as u16), Some(ID_OPEN));
    }
}
