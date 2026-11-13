//! 行の操作（複製・削除・移動・インデント・並べ替えなど）と、文字の変換（全角⇔半角、大文字⇔小文字など）。
//!
//! 選択範囲（なければカーソルのある行）の文字列を取り出し、`kazakiri_core::textops` の関数で作り直して、
//! 1 回の Undo で戻せる置き換えとして適用する。

use kazakiri_core::textops::{self, Conv, SortKey};
use kazakiri_core::EditKind;

use super::*;

/// 1 回の操作で扱える大きさの上限
const REGION_MAX: u64 = 64 << 20;

/// 「変換」メニューの項目の変換（ID は ID_CONV_BASE から、menu::CONV_NAMES と同じ並びで）
const CONVS: [Conv; 20] = [
    Conv::Upper,
    Conv::Lower,
    Conv::ZenToHanAscii,
    Conv::HanToZenAscii,
    Conv::ZenKanaToHan,
    Conv::HanKanaToZen,
    Conv::HiraToKata,
    Conv::KataToHira,
    Conv::TabToSpace(0),
    Conv::SpaceToTab(0),
    Conv::JsonPretty(2),
    Conv::JsonMinify,
    Conv::TrimLeading,
    Conv::Capitalize,
    Conv::HtmlEscape,
    Conv::HtmlUnescape,
    Conv::Base64Encode,
    Conv::Base64Decode,
    Conv::UrlEncode,
    Conv::UrlDecode,
];

/// 行の並べ替え（menu::SORT_NAMES と同じ並び）。(基準, 降順か)
const SORT_KINDS: [(SortKey, bool); 10] = [
    (SortKey::Number, false),
    (SortKey::Number, true),
    (SortKey::Length, false),
    (SortKey::Length, true),
    (SortKey::Frequency, false),
    (SortKey::Frequency, true),
    (SortKey::Ip, false),
    (SortKey::Ip, true),
    (SortKey::Reverse, false),
    (SortKey::Random, false),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// 選択（またはカーソル）のある行まるごと
    Lines,
    /// 選択範囲。なければファイル全体
    SelOrAll,
}

#[derive(Clone, Copy)]
enum After {
    /// 選択があれば置き換えた範囲を選び、なければカーソルを長さの増減に合わせて動かす
    Auto,
    /// もとの選択（カーソル）をこの分だけずらす
    Shift(i64),
    /// 置き換えた範囲の先頭にカーソル
    Start,
}

impl App {
    /// 置き換える範囲。`Lines` は行まるごと（末尾の改行を含む）
    fn region(&self, doc: &Document, scope: Scope) -> Option<(u64, u64)> {
        match scope {
            Scope::SelOrAll => Some(self.selection().unwrap_or((0, doc.len()))),
            Scope::Lines => {
                let (a, b) = self.sel.map(|(x, y)| (x.min(y), x.max(y)))?;
                let l1 = doc.line_of_offset(a)?;
                let l2 = doc.line_of_offset(if b > a { b - 1 } else { a })?;
                let start = doc.line_range(l1)?.0;
                let end = doc.line_range(l2 + 1).map_or(doc.len(), |(s, _)| s);
                Some((start, end))
            }
        }
    }

    fn transform(&mut self, label: &str, scope: Scope, after: After, f: &dyn Fn(&str, &str) -> String) {
        if !self.check_editable() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let Some((a, b)) = self.region(&doc, scope).filter(|(a, b)| a < b) else {
            self.message = "対象の文字がありません".into();
            return self.invalidate();
        };
        if b - a > REGION_MAX {
            self.message = "範囲が大きすぎます（64MB まで）。範囲を選んでから実行してください".into();
            return self.invalidate();
        }
        let old = doc.decode(&doc.read(a, b)).into_owned();
        let new = f(&old, self.newline_str());
        if new == old {
            self.message = "変更はありません".into();
            return self.invalidate();
        }
        if !doc.can_encode(&new) {
            self.message = format!("この文字は {} で表せないため変換できません", doc.encoding().name());
            return self.invalidate();
        }
        let delta = doc.encode(&new).len() as i64 - (b - a) as i64;
        let had_sel = self.selection().is_some();
        let sel = self.sel.unwrap_or((0, 0));
        let caret = sel.1;
        drop(doc);
        match self.with_editor(|ed| ed.replace(a, b, &new, EditKind::Other, sel)).unwrap() {
            Ok((s, e)) => {
                self.sel = Some(match after {
                    After::Start => (s, s),
                    After::Shift(d) => ((sel.0 as i64 + d).max(0) as u64, (sel.1 as i64 + d).max(0) as u64),
                    After::Auto if had_sel => (s, e),
                    After::Auto => {
                        let c = ((caret as i64 + delta).max(s as i64) as u64).min(e);
                        (c, c)
                    }
                });
                self.goal_x = None;
                self.message = if scope == Scope::SelOrAll && !had_sel { format!("{label}（選択がないので、ファイル全体）") } else { label.to_string() };
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    /// 現在の行の上（下）に、空行を入れてカーソルをそこへ置く
    fn insert_blank_line(&mut self, above: bool) {
        if !self.check_editable() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some((s, e)) = doc.line_of_offset(c).and_then(|l| doc.line_range(l)) else { return };
        let nl = self.newline_str();
        let (at, text) = if above { (s, nl.to_string()) } else { (e, nl.to_string()) };
        let sel = self.sel.unwrap_or((0, 0));
        drop(doc);
        if let Some(Ok((st, en))) = self.with_editor(|ed| ed.replace(at, at, &text, EditKind::Other, sel)) {
            // 上に入れたら、カーソルは新しい空行（元の行の位置）、下に入れたら、新しい空行の先頭
            let pos = if above { st } else { en };
            self.sel = Some((pos, pos));
            self.goal_x = None;
            self.after_edit();
        }
    }

    /// 連番を挿入する（入力: 開始値,増分,桁数。選んだ行の先頭に 1 行ずつ入れる）
    pub(super) fn insert_sequence(&mut self, spec: &str) {
        let nums: Vec<i64> = spec.split([',', ' ', '\u{3000}', '、']).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
        let (start, step, width) = (nums.first().copied().unwrap_or(1), nums.get(1).copied().unwrap_or(1), nums.get(2).copied().unwrap_or(0).clamp(0, 20) as usize);
        let counter = std::cell::Cell::new(start);
        self.transform("連番を挿入しました", Scope::Lines, After::Auto, &move |t, e| {
            let (lines, ends) = textops::split_lines(t, e);
            let v: Vec<String> = lines
                .iter()
                .map(|l| {
                    let n = counter.get();
                    counter.set(n + step);
                    format!("{:0>width$}{l}", n, width = width)
                })
                .collect();
            textops::join_lines(&v, e, ends)
        });
    }

    /// 選んだ行を、上（下）の行と入れ替える
    fn move_lines(&mut self, up: bool) {
        if !self.check_editable() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let Some((a, b)) = self.region(&doc, Scope::Lines) else { return };
        let eol = self.newline_str();
        let (Some(l1), Some(l2)) = (doc.line_of_offset(a), doc.line_of_offset(b.saturating_sub(1).max(a))) else { return };
        let (from, to) = if up {
            if l1 == 0 {
                self.message = "先頭の行です".into();
                return self.invalidate();
            }
            (doc.line_range(l1 - 1).map_or(0, |r| r.0), b)
        } else {
            if b >= doc.len() {
                self.message = "末尾の行です".into();
                return self.invalidate();
            }
            (a, doc.line_range(l2 + 2).map_or(doc.len(), |(s, _)| s))
        };
        if to - from > REGION_MAX {
            return;
        }
        let text = doc.decode(&doc.read(from, to)).into_owned();
        let new = textops::rotate_neighbor(&text, eol, up);
        if new == text || !doc.can_encode(&new) {
            return;
        }
        // 動かした行（選択）の新しい位置
        let sel_text = doc.decode(&doc.read(a, b)).into_owned();
        let nl = doc.encode(eol).len() as u64;
        let neighbor_text = if up { doc.decode(&doc.read(from, a)).into_owned() } else { doc.decode(&doc.read(b, to)).into_owned() };
        let sel_len = doc.encode(&sel_text).len() as u64;
        let (sel_start, sel_len) = if up {
            // 選択の最後の行が改行なしなら、となりの行が後ろに付くので改行が付く
            (from, sel_len + if sel_text.ends_with(eol) { 0 } else { nl })
        } else {
            let lacked = !neighbor_text.ends_with(eol);
            (from + doc.encode(&neighbor_text).len() as u64 + if lacked { nl } else { 0 }, sel_len - if lacked { nl } else { 0 })
        };
        let had_sel = self.selection().is_some();
        let caret = self.caret();
        let sel = self.sel.unwrap_or((0, 0));
        drop(doc);
        match self.with_editor(|ed| ed.replace(from, to, &new, EditKind::Other, sel)).unwrap() {
            Ok(_) => {
                self.sel = Some(if had_sel {
                    (sel_start, sel_start + sel_len)
                } else {
                    let c = sel_start + caret.saturating_sub(a).min(sel_len);
                    (c, c)
                });
                self.goal_x = None;
                self.after_edit();
            }
            Err(e) => {
                self.message = e.to_string();
                self.invalidate();
            }
        }
    }

    /// カーソルから行頭（行末）までを削除する
    fn delete_to_line_edge(&mut self, to_end: bool) {
        if !self.check_editable() {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        let c = self.caret();
        let Some((s, e)) = doc.line_of_offset(c).and_then(|l| doc.line_range(l)) else { return };
        let (a, b) = if to_end { (c, e) } else { (s, c) };
        if a >= b {
            return;
        }
        let sel = self.sel.unwrap_or((0, 0));
        drop(doc);
        if let Ok(Ok((st, _))) = self.with_editor(|ed| ed.replace(a, b, "", EditKind::Other, sel)).ok_or(()) {
            self.sel = Some((st, st));
            self.goal_x = None;
            self.after_edit();
        }
    }

    /// Tab / Shift+Tab：複数行を選んでいるときはインデント。処理したら true
    pub(super) fn tab_key(&mut self, shift: bool) -> bool {
        let multi_line = self.doc.as_ref().is_some_and(|doc| {
            self.selection().is_some_and(|(a, b)| doc.line_of_offset(a) != doc.line_of_offset(b - 1))
        });
        if shift {
            self.command_lines(menu::ID_UNINDENT);
            return true;
        }
        if multi_line {
            self.command_lines(menu::ID_INDENT);
            return true;
        }
        false
    }

    /// 行の操作・変換のコマンド。処理したら true
    pub(super) fn command_lines(&mut self, id: u32) -> bool {
        let tab = self.settings.tab_width.max(1);
        match id {
            menu::ID_LINE_DUP => {
                let shift = self.doc.clone().and_then(|d| self.region(&d, Scope::Lines)).map_or(0, |(a, b)| (b - a) as i64);
                self.transform("行を複製しました", Scope::Lines, After::Shift(shift), &|t, e| textops::duplicate(t, e));
            }
            menu::ID_LINE_DEL => self.transform("行を削除しました", Scope::Lines, After::Start, &|_, _| String::new()),
            menu::ID_LINE_UP => self.move_lines(true),
            menu::ID_LINE_DOWN => self.move_lines(false),
            menu::ID_DEL_BOL => self.delete_to_line_edge(false),
            menu::ID_DEL_EOL => self.delete_to_line_edge(true),
            menu::ID_INDENT => self.transform("インデントしました", Scope::Lines, After::Auto, &|t, e| textops::indent(t, e, "\t")),
            menu::ID_UNINDENT => self.transform("逆インデントしました", Scope::Lines, After::Auto, &move |t, e| textops::unindent(t, e, tab)),
            menu::ID_TRIM => self.transform("行末の空白を削除しました", Scope::SelOrAll, After::Auto, &|t, e| textops::trim_trailing(t, e)),
            menu::ID_DEDUPE => self.transform("重複した行を削除しました", Scope::SelOrAll, After::Auto, &|t, e| textops::dedupe(t, e)),
            menu::ID_SORT_LINES_ASC => self.transform("行を昇順に並べ替えました", Scope::SelOrAll, After::Auto, &|t, e| textops::sort(t, e, false)),
            menu::ID_SORT_LINES_DESC => self.transform("行を降順に並べ替えました", Scope::SelOrAll, After::Auto, &|t, e| textops::sort(t, e, true)),
            id if (menu::ID_SORTK_BASE..menu::ID_SORTK_BASE + SORT_KINDS.len() as u32).contains(&id) => {
                let (key, desc) = SORT_KINDS[(id - menu::ID_SORTK_BASE) as usize];
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
                let name = menu::SORT_NAMES[(id - menu::ID_SORTK_BASE) as usize].split('(').next().unwrap_or("").to_string();
                self.transform(&format!("行を「{name}」で並べ替えました"), Scope::SelOrAll, After::Auto, &move |t, e| textops::sort_by(t, e, key, desc, seed));
            }
            menu::ID_INS_ABOVE | menu::ID_INS_BELOW => self.insert_blank_line(id == menu::ID_INS_ABOVE),
            menu::ID_INS_SEQ => self.start_prompt(Prompt::Sequence),
            menu::ID_INS_TAB => self.type_text("\t", EditKind::Other),
            menu::ID_INS_CRLF => self.type_text("\r\n", EditKind::Other),
            menu::ID_INS_LF => self.type_text("\n", EditKind::Other),
            menu::ID_INS_CR => self.type_text("\r", EditKind::Other),
            menu::ID_INSERT_DATE | menu::ID_INSERT_DATETIME => {
                let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
                let s = if id == menu::ID_INSERT_DATE {
                    format!("{:04}/{:02}/{:02}", t.wYear, t.wMonth, t.wDay)
                } else {
                    format!("{:04}/{:02}/{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
                };
                self.type_text(&s, EditKind::Other);
            }
            id if (menu::ID_CONV_BASE..menu::ID_CONV_BASE + CONVS.len() as u32).contains(&id) => {
                let (name, conv) = (menu::CONV_NAMES[(id - menu::ID_CONV_BASE) as usize], CONVS[(id - menu::ID_CONV_BASE) as usize]);
                let conv = match conv {
                    Conv::TabToSpace(_) => Conv::TabToSpace(tab),
                    Conv::SpaceToTab(_) => Conv::SpaceToTab(tab),
                    c => c,
                };
                let label = name.split('(').next().unwrap_or(name).to_string() + "を実行しました";
                self.transform(&label, Scope::SelOrAll, After::Auto, &move |t, e| {
                    let r = textops::convert(t, conv);
                    if matches!(conv, Conv::JsonPretty(_)) && e != "\n" { r.replace('\n', e) } else { r }
                });
            }
            _ => return false,
        }
        true
    }
}
