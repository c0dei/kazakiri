//! 行の統計：最長・最短の行を探す、頻出する行を抜き出す（検索メニュー）。

use super::*;

/// 一度に調べる行数の上限
const MAX_LINES: u64 = 5_000_000;

impl App {
    /// 最長（最短）の行へ移動する。最短は空行を除く。同じ長さなら先頭に近い行
    pub(super) fn find_line_by_length(&mut self, longest: bool) {
        let Some(doc) = self.doc.clone() else { return };
        let (n, exact) = doc.line_count();
        if !exact || n > MAX_LINES {
            self.message = "行数が多い（または索引の作成中）ため調べられません（500 万行まで）".into();
            return self.invalidate();
        }
        let mut best: Option<(u64, u64)> = None; // (長さ, 行)
        for l in 0..n {
            let Some((s, e)) = doc.line_range(l) else { break };
            let len = e - s;
            if !longest && len == 0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((b, _)) => if longest { len > b } else { len < b },
            };
            if better {
                best = Some((len, l));
            }
        }
        let Some((len, line)) = best else {
            self.message = "該当する行がありません".into();
            return self.invalidate();
        };
        if let Some((s, e)) = doc.line_range(line) {
            self.goto_range(s, e);
        }
        self.message = format!("{}の行: {} 行目（{} バイト）", if longest { "最長" } else { "最短" }, group(line + 1), group(len));
        self.invalidate();
    }

    /// 同じ行の出現回数を数え、多い順に、新しいタブへ「回数<TAB>行」で出す（上位 1,000 件）
    pub(super) fn frequent_lines(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        let (n, exact) = doc.line_count();
        if !exact || n > MAX_LINES {
            self.message = "行数が多い（または索引の作成中）ため調べられません（500 万行まで）".into();
            return self.invalidate();
        }
        let hashes = kazakiri_core::diff::line_hashes(&doc);
        // ハッシュごとの (回数, 最初の行)
        let mut count: HashMap<u64, (u64, u64)> = HashMap::new();
        for (i, h) in hashes.iter().enumerate() {
            let e = count.entry(*h).or_insert((0, i as u64));
            e.0 += 1;
        }
        let mut v: Vec<(u64, u64)> = count.into_values().collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut out = String::new();
        for &(c, first) in v.iter().take(1000) {
            let text = doc.lines(first, 1).first().map_or(String::new(), |l| l.text.clone());
            out.push_str(&format!("{c}\t{text}\n"));
        }
        let kinds = v.len();
        self.new_tab();
        let nl = self.newline_str();
        let text = if nl == "\n" { out } else { out.replace('\n', nl) };
        self.type_text(&text, EditKind::Other);
        self.sel = Some((0, 0));
        self.top = 0;
        self.message = format!("行の種類 {} のうち、多い順に {} 件を出しました（回数<TAB>行）", group(kinds as u64), group(kinds.min(1000) as u64));
        self.invalidate();
    }
}
