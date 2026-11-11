//! 検索語・置換語の履歴（%APPDATA%\kazakiri\history.txt）。
//!
//! 検索欄で ↑↓ を押すと呼び出せる。新しいものが先頭。同じ語は 1 つにまとめる。

use std::path::{Path, PathBuf};

const MAX: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// 検索語（検索・置換の検索語・GREP）
    Find,
    /// 置換後の文字列
    Replace,
}

#[derive(Default, Debug, PartialEq)]
pub struct History {
    find: Vec<String>,
    replace: Vec<String>,
}

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(Path::new(&base).join("kazakiri").join("history.txt"))
}

impl History {
    pub fn load() -> History {
        store_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| History::parse(&t)).unwrap_or_default()
    }

    fn parse(text: &str) -> History {
        let mut h = History::default();
        for line in text.lines() {
            // 「f<TAB>語」「r<TAB>語」。タブと改行は語の中に入れない
            match line.split_once('\t') {
                Some(("f", w)) if !w.is_empty() => h.find.push(w.to_string()),
                Some(("r", w)) if !w.is_empty() => h.replace.push(w.to_string()),
                _ => {}
            }
        }
        h.find.truncate(MAX);
        h.replace.truncate(MAX);
        h
    }

    fn to_text(&self) -> String {
        let mut s = String::new();
        for w in &self.find {
            s.push_str(&format!("f\t{w}\n"));
        }
        for w in &self.replace {
            s.push_str(&format!("r\t{w}\n"));
        }
        s
    }

    pub fn list(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Find => &self.find,
            Kind::Replace => &self.replace,
        }
    }

    /// 先頭に追加して保存する（保存の失敗は無視：使い勝手に関わるだけなので）
    pub fn push(&mut self, kind: Kind, word: &str) {
        // 改行やタブを含む語は、1 行 1 件の保存形式に入らないので覚えない
        if word.is_empty() || word.contains(['\n', '\r', '\t']) {
            return;
        }
        let list = match kind {
            Kind::Find => &mut self.find,
            Kind::Replace => &mut self.replace,
        };
        if list.first().is_some_and(|w| w == word) {
            return;
        }
        list.retain(|w| w != word);
        list.insert(0, word.to_string());
        list.truncate(MAX);
        self.save();
    }

    fn save(&self) {
        let Some(store) = store_path() else { return };
        if let Some(dir) = store.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(store, self.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_dedupe() {
        let mut h = History::default();
        // push は保存するので、ここでは内部だけを使う
        for w in ["a", "b", "a", "日本語"] {
            h.find.retain(|x| x != w);
            h.find.insert(0, w.to_string());
        }
        h.replace.push("x y".into());
        assert_eq!(h.find, ["日本語", "a", "b"]);
        assert_eq!(History::parse(&h.to_text()), h);
    }

    #[test]
    fn rejects_unsavable_words_and_broken_lines() {
        let mut h = History::default();
        h.push(Kind::Find, "");
        h.push(Kind::Find, "a\tb");
        h.push(Kind::Find, "a\nb");
        assert!(h.list(Kind::Find).is_empty());
        let h = History::parse("x\ty\nf\t\nf\tok\nbroken\nr\tz\n");
        assert_eq!(h.list(Kind::Find), ["ok"]);
        assert_eq!(h.list(Kind::Replace), ["z"]);
    }
}
