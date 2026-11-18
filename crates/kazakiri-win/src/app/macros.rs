//! マクロ（操作の記録と再生）。編集 → マクロ：記録の開始／停止（Ctrl+Shift+R）、再生（Ctrl+Shift+E）、回数を指定して再生。
//!
//! 記録するのは、キーボードでの操作：入力した文字、カーソル移動・Enter・BackSpace・Delete・Tab などのキー、
//! 編集に関するコマンド（行の操作・変換・挿入・コピー・貼り付け・元に戻すなど、`recordable`）、次／前を検索（検索語とオプションごと）。
//! 記録しないもの：マウス操作、ファイル・タブ・ダイアログ・入力欄を伴う操作、検索・置換の入力そのもの（置換は未対応）。
//! 記録したマクロは 1 つだけ持ち、`%APPDATA%\kazakiri\macro.txt` に保存して、次の起動でも使える。
//!
//! 再生は、検索（別スレッド）の完了を待ちながら順に進める。検索で見つからなかったら、そこで終える
//! （「0 回 = 見つからなくなるまで」で、検索して置き換える、のような繰り返しができる）。Esc で中止。

use std::path::Path;

use kazakiri_core::SearchOptions;

use super::*;

/// 記録の 1 手順
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// 編集のキー（Shift・Ctrl の状態つき）
    Key { vk: u16, ctrl: bool, shift: bool },
    /// 入力した文字（続けて入力したものは 1 つにまとめる）
    Text(String),
    /// メニュー・ショートカットのコマンド
    Cmd(u32),
    /// 次／前を検索（そのときの検索語とオプション）
    Find { forward: bool, text: String, case: bool, regex: bool, word: bool },
}

/// 再生中の状態
pub(super) struct Play {
    steps: Vec<Step>,
    pos: usize,
    /// 実行する回数（None は、検索で見つからなくなるまで）
    limit: Option<u32>,
    /// 終えた回数
    runs: u32,
}

/// 「見つからなくなるまで」の安全弁
const UNTIL_FAIL_MAX: u32 = 1_000_000;
/// 1 回の処理で進める手順の数（これを超えたら、いったん画面に制御を返す）
const SLICE: usize = 2_000;
/// 記録の長さの上限
const MAX_STEPS: usize = 100_000;

/// 記録できるコマンドか（編集に関するもので、入力欄・ダイアログ・ファイル・タブを伴わないもの）
pub(super) fn recordable(id: u32) -> bool {
    use menu::*;
    matches!(id,
        ID_LINE_DUP..=ID_INSERT_DATETIME
        | ID_INS_ABOVE | ID_INS_BELOW | ID_INS_TAB | ID_INS_CRLF | ID_INS_LF | ID_INS_CR
        | ID_COPY | ID_SELECT_ALL | ID_UNDO | ID_REDO | ID_CUT | ID_PASTE | ID_ADD_NEXT
        | ID_BM_TOGGLE | ID_BM_NEXT | ID_BM_PREV
    ) || (ID_CONV_BASE..ID_CONV_BASE + CONV_NAMES.len() as u32).contains(&id)
        || (ID_SORTK_BASE..ID_SORTK_BASE + SORT_NAMES.len() as u32).contains(&id)
}

/// 保存用の 1 行（`K 40 1 0` / `T 文字列` / `C 801` / `F f 101 検索語`）
fn encode_step(s: &Step) -> String {
    let esc = |t: &str| t.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
    match s {
        Step::Key { vk, ctrl, shift } => format!("K {vk} {} {}", u8::from(*ctrl), u8::from(*shift)),
        Step::Text(t) => format!("T {}", esc(t)),
        Step::Cmd(id) => format!("C {id}"),
        Step::Find { forward, text, case, regex, word } => format!("F {} {}{}{} {}", if *forward { 'f' } else { 'b' }, u8::from(*case), u8::from(*regex), u8::from(*word), esc(text)),
    }
}

fn decode_step(line: &str) -> Option<Step> {
    let unesc = |t: &str| {
        let mut out = String::new();
        let mut it = t.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        }
        out
    };
    let (kind, rest) = line.split_once(' ')?;
    match kind {
        "K" => {
            let mut p = rest.split(' ');
            Some(Step::Key { vk: p.next()?.parse().ok()?, ctrl: p.next()? == "1", shift: p.next()? == "1" })
        }
        "T" => Some(Step::Text(unesc(rest))),
        "C" => Some(Step::Cmd(rest.trim().parse().ok()?)),
        "F" => {
            let mut p = rest.splitn(3, ' ');
            let dir = p.next()?;
            let flags = p.next()?.as_bytes();
            if flags.len() != 3 {
                return None;
            }
            Some(Step::Find { forward: dir == "f", case: flags[0] == b'1', regex: flags[1] == b'1', word: flags[2] == b'1', text: unesc(p.next().unwrap_or("")) })
        }
        _ => None,
    }
}

pub(super) fn encode_steps(steps: &[Step]) -> String {
    steps.iter().map(|s| encode_step(s) + "\n").collect()
}

pub(super) fn decode_steps(text: &str) -> Vec<Step> {
    text.lines().filter_map(decode_step).collect()
}

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(Path::new(&base).join("kazakiri").join("macro.txt"))
}

pub(super) fn load_macro() -> Vec<Step> {
    store_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| decode_steps(&t)).unwrap_or_default()
}

fn save_macro(steps: &[Step]) {
    if let Some(p) = store_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, encode_steps(steps));
    }
}

impl App {
    pub(super) fn macro_recording(&self) -> bool {
        self.macro_rec.is_some()
    }

    fn macro_playing(&self) -> bool {
        self.macro_play.is_some()
    }

    /// 記録中なら、手順を足す（入力した文字は、直前の入力につなげる）
    pub(super) fn macro_push(&mut self, step: Step) {
        if self.macro_playing() {
            return;
        }
        let Some(rec) = &mut self.macro_rec else { return };
        if rec.len() >= MAX_STEPS {
            return;
        }
        match (rec.last_mut(), &step) {
            (Some(Step::Text(prev)), Step::Text(t)) => prev.push_str(t),
            _ => rec.push(step),
        }
    }

    /// コマンドが実行されるときに呼ぶ。記録できるものは記録し、そうでないものは数えておく（マクロ自身の操作は除く）
    pub(super) fn macro_note_command(&mut self, id: u32) {
        if self.macro_rec.is_none() || self.macro_playing() {
            return;
        }
        if recordable(id) {
            self.macro_push(Step::Cmd(id));
        } else if !matches!(id, menu::ID_MACRO_REC | menu::ID_MACRO_PLAY | menu::ID_MACRO_REPEAT | menu::ID_FIND_NEXT | menu::ID_FIND_PREV) {
            self.macro_skipped += 1;
        }
    }

    /// 次／前を検索するときに呼ぶ（検索語とオプションごと記録する）
    pub(super) fn macro_note_find(&mut self, dir: Direction) {
        if self.macro_rec.is_none() || self.last_find.is_empty() {
            return;
        }
        let o = self.opts;
        self.macro_push(Step::Find { forward: dir == Direction::Forward, text: self.last_find.clone(), case: o.case_sensitive, regex: o.regex, word: o.whole_word });
    }

    /// 記録の開始／停止
    pub(super) fn macro_toggle_record(&mut self) {
        if self.macro_playing() {
            self.message = "マクロの再生中です（Esc で中止）".into();
            return self.invalidate();
        }
        match self.macro_rec.take() {
            None => {
                self.macro_rec = Some(Vec::new());
                self.macro_skipped = 0;
                self.message = "マクロの記録を始めました（もう一度 Ctrl+Shift+R で終了。マウス操作は記録されません）".into();
            }
            Some(steps) => {
                let skipped = self.macro_skipped;
                if steps.is_empty() {
                    self.message = "記録を終えました（何も記録されませんでした。前のマクロは残っています）".into();
                } else {
                    self.message = format!("記録を終えました: {} 手順{}", steps.len(), if skipped > 0 { format!("（記録の対象外の操作 {skipped} 件は含まれません）") } else { String::new() });
                    save_macro(&steps);
                    self.macro_steps = steps;
                }
            }
        }
        self.invalidate();
    }

    /// 再生を始める。`limit` は回数（None は、検索で見つからなくなるまで）
    pub(super) fn macro_start(&mut self, limit: Option<u32>) {
        if self.macro_rec.is_some() {
            self.message = "記録中は再生できません（先に Ctrl+Shift+R で終了）".into();
            return self.invalidate();
        }
        if self.macro_playing() {
            return;
        }
        if self.macro_steps.is_empty() {
            self.message = "再生するマクロがありません（Ctrl+Shift+R で記録）".into();
            return self.invalidate();
        }
        if limit.is_none() && !self.macro_steps.iter().any(|s| matches!(s, Step::Find { .. })) {
            self.message = "「見つからなくなるまで」は、検索を含むマクロでだけ使えます。回数を指定してください".into();
            return self.invalidate();
        }
        self.macro_play = Some(Play { steps: self.macro_steps.clone(), pos: 0, limit, runs: 0 });
        self.macro_run();
    }

    fn macro_finish(&mut self, why: &str) {
        if let Some(p) = self.macro_play.take() {
            self.message = format!("マクロを終えました（{} 回実行。{why}）", p.runs);
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_MACRO);
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// 再生を中止する（Esc）
    pub(super) fn macro_cancel(&mut self) -> bool {
        if self.macro_play.is_none() {
            return false;
        }
        self.cancel_search();
        self.macro_finish("中止しました");
        true
    }

    /// タイマー：続きを再生する
    pub(super) fn macro_tick(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_MACRO);
        }
        if self.search.is_none() {
            self.macro_run();
        }
    }

    /// 検索が終わった。見つかっていれば続きを再生し、見つからなければ終える
    pub(super) fn macro_after_search(&mut self, found: bool) {
        if self.macro_play.is_none() {
            return;
        }
        if found {
            self.macro_run();
        } else {
            let msg = self.message.clone();
            self.macro_finish("検索で見つからなかったため");
            self.message = format!("{msg}  ※マクロを終えました");
        }
    }

    /// 手順を順に実行する。検索を始めたら、その完了まで待つ。1 回の処理で進める数に上限がある
    fn macro_run(&mut self) {
        for _ in 0..SLICE {
            let Some(p) = self.macro_play.as_mut() else { return };
            if p.pos >= p.steps.len() {
                p.runs += 1;
                let done = match p.limit {
                    Some(n) => p.runs >= n,
                    None => p.runs >= UNTIL_FAIL_MAX,
                };
                if done {
                    return self.macro_finish("終わりまで");
                }
                p.pos = 0;
                continue;
            }
            let step = p.steps[p.pos].clone();
            p.pos += 1;
            match step {
                Step::Key { vk, ctrl, shift } => {
                    let _ = unsafe { self.on_key_mods(VIRTUAL_KEY(vk), ctrl, shift) };
                }
                Step::Text(t) => self.type_text(&t, EditKind::Typing),
                Step::Cmd(id) => {
                    let _ = unsafe { self.command(id) };
                }
                Step::Find { forward, text, case, regex, word } => {
                    self.last_find = text;
                    self.opts = SearchOptions { case_sensitive: case, regex, whole_word: word };
                    self.find(if forward { Direction::Forward } else { Direction::Backward });
                    if self.search.is_some() {
                        return; // 完了を待つ（on_search_done → macro_after_search）
                    }
                    return self.macro_finish("検索を始められなかったため");
                }
            }
        }
        // 長く続くときは、画面を更新できるよう、いったん制御を返す
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_MACRO, 1, None);
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip_through_text() {
        let steps = vec![
            Step::Key { vk: 40, ctrl: true, shift: false },
            Step::Text("a\tb\\c\nd日本語".into()),
            Step::Cmd(801),
            Step::Find { forward: false, text: "foo bar".into(), case: true, regex: false, word: true },
            Step::Find { forward: true, text: String::new(), case: false, regex: true, word: false },
        ];
        let text = encode_steps(&steps);
        assert_eq!(text.lines().count(), steps.len(), "1 手順 = 1 行: {text:?}");
        assert_eq!(decode_steps(&text), steps);
        // 壊れた行は読み飛ばす
        assert_eq!(decode_steps("X 1\nK 1\nC abc\nC 5\n"), vec![Step::Cmd(5)]);
    }

    #[test]
    fn recordable_commands_exclude_dialogs_and_files() {
        assert!(recordable(menu::ID_LINE_DUP) && recordable(menu::ID_PASTE) && recordable(menu::ID_UNDO));
        assert!(recordable(menu::ID_CONV_BASE));
        for id in [menu::ID_OPEN, menu::ID_SAVE, menu::ID_CLOSE_TAB, menu::ID_SETTINGS, menu::ID_FIND, menu::ID_GOTO, menu::ID_REPLACE, menu::ID_INS_SEQ, menu::ID_PALETTE] {
            assert!(!recordable(id), "{id} は記録しない");
        }
    }
}
