//! 2 つのファイルの行ごとの差分（左右に並べて見るための、行の対応づけ）。
//!
//! 各行をハッシュにして、共通の先頭・末尾を取り除く。大きいときは、両方に 1 回ずつしか出てこない行（時刻つきのログの行など）を
//! 目印（アンカー）にして区間に分け（patience 方式）、区間ごとに Myers の O(ND) 法で編集列を求める。
//! 区間ごとの違いが多すぎる（D が上限を超える）、または記録が大きくなりすぎるときは、そのことを返す。

use std::hash::{Hash, Hasher};

use crate::document::Document;

/// 左右の対応の 1 行
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// 左（A）の行番号（0 始まり）。なければ右にだけある行
    pub a: Option<u64>,
    /// 右（B）の行番号
    pub b: Option<u64>,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    /// 左にしかない
    Del,
    /// 右にしかない
    Ins,
    /// 両方にあるが違う
    Chg,
}

/// 違いの数の上限（これを超えたら諦める。目印で分けた区間ごとの数）
pub const MAX_D: usize = 20_000;

/// Myers の記録の大きさの上限（要素数。8 バイト × 6400 万 = 約 512MB）
const TRACE_LIMIT: usize = 64_000_000;

/// 比較のオプション
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opts {
    /// 行頭・行末の空白（スペース・タブ・全角スペース）を無視する
    pub ignore_edge_space: bool,
    /// 大文字と小文字を区別しない
    pub ignore_case: bool,
}

/// 1 行ごとのハッシュ（行末の CR は無視して、CRLF と LF の違いは差分にしない）
pub fn line_hashes(doc: &Document) -> Vec<u64> {
    line_hashes_with(doc, Opts::default())
}

/// オプションを反映した、1 行ごとのハッシュ（行の範囲を分けて、複数のスレッドで求める）
pub fn line_hashes_with(doc: &Document, opts: Opts) -> Vec<u64> {
    // 行索引はバックグラウンドで作られる。途中の行数で比べると、末尾が欠けるので、できあがるのを待つ
    while !doc.line_count().1 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let n = doc.line_count().0 as usize;
    let hash_line = |l: u64| -> Option<u64> {
        let (s, e) = doc.line_range(l)?;
        let bytes = doc.read(s, e);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(&bytes);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if opts == Opts::default() {
            bytes.hash(&mut h);
        } else {
            let text = doc.decode(bytes);
            let t = if opts.ignore_edge_space { text.trim_matches([' ', '\t', '\u{3000}']) } else { &text };
            if opts.ignore_case {
                t.to_lowercase().hash(&mut h);
            } else {
                t.hash(&mut h);
            }
        }
        Some(h.finish())
    };
    // 小さいうちはそのまま。大きいときは、行を等分して並列に
    let parts = if n < 50_000 { 1 } else { crate::search::threads() * 4 };
    let step = n.div_ceil(parts).max(1);
    let ranges: Vec<(usize, usize)> = (0..n).step_by(step).map(|a| (a, (a + step).min(n))).collect();
    let chunks = crate::search::run_parallel(&ranges, |&(a, b)| Some((a as u64..b as u64).map_while(&hash_line).collect::<Vec<u64>>()));
    let mut out = Vec::with_capacity(n);
    for c in chunks.into_iter().flatten() {
        out.extend(c);
    }
    // 最後が改行で終わるファイルの、最後の空行は行に数えない
    if let Some(last) = doc.line_count().0.checked_sub(1) {
        if doc.line_range(last).is_some_and(|(s, e)| s == e) && out.len() as u64 == doc.line_count().0 {
            out.pop();
        }
    }
    out
}

/// 差分を求める。None は違いが多すぎるとき
pub fn diff(a: &[u64], b: &[u64]) -> Option<Vec<Row>> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let ops = diff_ops(ma, mb, 0)?;
    let mut rows: Vec<Row> = (0..pre as u64).map(|i| Row { a: Some(i), b: Some(i), kind: Kind::Same }).collect();
    let (mut ai, mut bi) = (pre as u64, pre as u64);
    let mut dels: Vec<u64> = Vec::new();
    let mut inss: Vec<u64> = Vec::new();
    let flush = |rows: &mut Vec<Row>, dels: &mut Vec<u64>, inss: &mut Vec<u64>| {
        let k = dels.len().min(inss.len());
        for i in 0..k {
            rows.push(Row { a: Some(dels[i]), b: Some(inss[i]), kind: Kind::Chg });
        }
        for &d in &dels[k..] {
            rows.push(Row { a: Some(d), b: None, kind: Kind::Del });
        }
        for &i in &inss[k..] {
            rows.push(Row { a: None, b: Some(i), kind: Kind::Ins });
        }
        dels.clear();
        inss.clear();
    };
    for op in ops {
        match op {
            Op::Equal => {
                flush(&mut rows, &mut dels, &mut inss);
                rows.push(Row { a: Some(ai), b: Some(bi), kind: Kind::Same });
                ai += 1;
                bi += 1;
            }
            Op::Delete => {
                dels.push(ai);
                ai += 1;
            }
            Op::Insert => {
                inss.push(bi);
                bi += 1;
            }
        }
    }
    flush(&mut rows, &mut dels, &mut inss);
    let base = (a.len() - suf) as u64;
    for i in 0..suf as u64 {
        rows.push(Row { a: Some(base + i), b: Some((b.len() - suf) as u64 + i), kind: Kind::Same });
    }
    Some(rows)
}

/// 区間の最小の大きさ（これより小さければ、目印を探さず Myers にかける）
const ANCHOR_MIN: usize = 1_000;
/// 大きい区間で、まず Myers だけで試す違いの数（これで収まれば、目印を探さない）
const QUICK_D: usize = 500;
/// 目印で区間に分けるのを繰り返す深さの上限
const ANCHOR_DEPTH: usize = 12;

/// 編集列を求める。大きいときは、両方に 1 回ずつしかない行を目印に区間へ分けて、区間ごとに求める
fn diff_ops(a: &[u64], b: &[u64], depth: usize) -> Option<Vec<Op>> {
    // 共通の先頭・末尾は、そのまま「同じ」
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops = vec![Op::Equal; pre];
    if ma.len().min(mb.len()) < ANCHOR_MIN || depth >= ANCHOR_DEPTH {
        ops.extend(myers(ma, mb, MAX_D)?);
    } else if let Some(m) = myers(ma, mb, QUICK_D) {
        // 違いが少なければ、目印を探さずに Myers だけで済む（速い）
        ops.extend(m);
    } else {
        let anchors = unique_anchors(ma, mb);
        if anchors.is_empty() {
            ops.extend(myers(ma, mb, MAX_D)?);
        } else {
            let (mut pa, mut pb) = (0, 0);
            for (ai, bi) in anchors {
                ops.extend(diff_ops(&ma[pa..ai], &mb[pb..bi], depth + 1)?);
                ops.push(Op::Equal);
                (pa, pb) = (ai + 1, bi + 1);
            }
            ops.extend(diff_ops(&ma[pa..], &mb[pb..], depth + 1)?);
        }
    }
    ops.extend(std::iter::repeat_n(Op::Equal, suf));
    Some(ops)
}

/// 両方に 1 回ずつしか出てこない行の位置の組のうち、順序が両方で保たれるもの（最長増加部分列）
fn unique_anchors(a: &[u64], b: &[u64]) -> Vec<(usize, usize)> {
    use std::collections::HashMap;
    // 値 → （A での位置、B での位置、A に出た回数、B に出た回数）
    let mut seen: HashMap<u64, (usize, usize, u32, u32)> = HashMap::with_capacity(a.len().min(b.len()) * 2);
    for (i, &h) in a.iter().enumerate() {
        let e = seen.entry(h).or_insert((i, 0, 0, 0));
        e.2 += 1;
    }
    for (j, &h) in b.iter().enumerate() {
        if let Some(e) = seen.get_mut(&h) {
            e.1 = j;
            e.3 += 1;
        }
    }
    // A の順に、B での位置を並べる
    let mut pairs: Vec<(usize, usize)> = seen.values().filter(|e| e.2 == 1 && e.3 == 1).map(|e| (e.0, e.1)).collect();
    pairs.sort_unstable();
    // B での位置の最長増加部分列（patience 法）
    let mut tails: Vec<usize> = Vec::new(); // 長さ k+1 の列の、最後の要素の位置（pairs の添字）
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    for (idx, &(_, bj)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].1 < bj);
        if pos > 0 {
            prev[idx] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(idx);
        } else {
            tails[pos] = idx;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(pairs[i]);
        cur = prev[i];
    }
    out.reverse();
    out
}

#[derive(Clone, Copy)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Myers の差分。編集列（A → B）を返す
fn myers(a: &[u64], b: &[u64], limit: usize) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 {
        return Some(vec![Op::Insert; b.len()]);
    }
    if m == 0 {
        return Some(vec![Op::Delete; a.len()]);
    }
    let max = ((n + m) as usize).min(limit);
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // 各 d の前の状態のうち、使う斜線（-d-1 ..= d+1）だけを記録する（戻るときに使う）。全体を複製すると、d が大きいときに数 GB になる
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut recorded = 0usize;
    let mut found = None;
    'outer: for d in 0..=max as isize {
        let (lo, hi) = ((off - d - 1) as usize, (off + d + 1) as usize);
        trace.push(v[lo..=hi].to_vec());
        recorded += hi - lo + 1;
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]) { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
        // 記録が大きくなりすぎる前に諦める（約 512MB。d の 2 乗で増える）
        if recorded > TRACE_LIMIT {
            return None;
        }
    }
    let d_end = found?;
    // 戻って編集列を作る
    let mut ops: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let vv = &trace[d as usize];
        // vv[i] は斜線 i - d - 1 の値
        let at = |k: isize| vv[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert);
                y -= 1;
            } else {
                ops.push(Op::Delete);
                x -= 1;
            }
        }
    }
    ops.reverse();
    Some(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u64> {
        s.chars().map(|c| c as u64).collect()
    }

    fn kinds(rows: &[Row]) -> String {
        rows.iter().map(|r| match r.kind { Kind::Same => '=', Kind::Del => '-', Kind::Ins => '+', Kind::Chg => '~' }).collect()
    }

    #[test]
    fn basic_diffs() {
        assert_eq!(kinds(&diff(&h("abc"), &h("abc")).unwrap()), "===");
        assert_eq!(kinds(&diff(&h("abc"), &h("abxc")).unwrap()), "==+=");
        assert_eq!(kinds(&diff(&h("abxc"), &h("abc")).unwrap()), "==-=");
        assert_eq!(kinds(&diff(&h("abc"), &h("aXc")).unwrap()), "=~=");
        assert_eq!(kinds(&diff(&h(""), &h("ab")).unwrap()), "++");
        assert_eq!(kinds(&diff(&h("ab"), &h("")).unwrap()), "--");
        // 行番号の対応
        let r = diff(&h("abcd"), &h("acXd")).unwrap();
        assert_eq!(kinds(&r), "=-=+=");
        let same: Vec<(u64, u64)> = r.iter().filter(|x| x.kind == Kind::Same).map(|x| (x.a.unwrap(), x.b.unwrap())).collect();
        assert_eq!(same, vec![(0, 0), (2, 1), (3, 3)]);
    }

    #[test]
    fn options_ignore_space_and_case() {
        let a = Document::from_bytes(b"Alpha\n  beta \nGAMMA\n".to_vec());
        let b = Document::from_bytes(b"alpha\nbeta\ngamma\n".to_vec());
        let plain = diff(&line_hashes(&a), &line_hashes(&b)).unwrap();
        assert!(plain.iter().any(|r| r.kind != Kind::Same));
        let opts = Opts { ignore_edge_space: true, ignore_case: true };
        let ign = diff(&line_hashes_with(&a, opts), &line_hashes_with(&b, opts)).unwrap();
        assert!(ign.iter().all(|r| r.kind == Kind::Same));
    }

    /// 行の対応が正しいか：A・B の行が順に 1 回ずつ現れ、「同じ」の行はハッシュが等しい。違いの数（Chg は 1）も返す
    fn check(a: &[u64], b: &[u64], rows: &[Row]) -> usize {
        let (mut ai, mut bi, mut edits) = (0u64, 0u64, 0);
        for r in rows {
            if let Some(x) = r.a {
                assert_eq!(x, ai);
                ai += 1;
            }
            if let Some(y) = r.b {
                assert_eq!(y, bi);
                bi += 1;
            }
            match r.kind {
                Kind::Same => assert_eq!(a[r.a.unwrap() as usize], b[r.b.unwrap() as usize]),
                _ => edits += 1,
            }
        }
        assert_eq!((ai as usize, bi as usize), (a.len(), b.len()));
        edits
    }

    #[test]
    fn anchored_diff_handles_many_scattered_changes() {
        // 100 万行のうち 2 万行が違う（以前は「違いが多すぎる」で諦めていた）
        let n = 1_000_000u64;
        let a: Vec<u64> = (0..n).collect();
        let mut b = a.clone();
        for i in (0..n as usize).step_by(50) {
            b[i] = 10_000_000 + i as u64;
        }
        let rows = diff(&a, &b).expect("anchors should split the work");
        assert_eq!(check(&a, &b, &rows), 20_000);
        // 挿入・削除が混ざっても、行の対応は正しい
        let mut c: Vec<u64> = Vec::new();
        for i in 0..200_000u64 {
            match i % 97 {
                0 => {}
                1 => c.extend([i, 5_000_000 + i]),
                _ => c.push(i),
            }
        }
        let a2: Vec<u64> = (0..200_000).collect();
        let rows = diff(&a2, &c).unwrap();
        check(&a2, &c, &rows);
    }

    #[test]
    fn repeated_lines_without_anchors_still_work() {
        // 同じ行ばかり（目印がない）でも、Myers にかかる
        let a: Vec<u64> = (0..5_000).map(|i| i % 3).collect();
        let b: Vec<u64> = (0..5_000).map(|i| (i + 1) % 3).collect();
        let rows = diff(&a, &b).unwrap();
        check(&a, &b, &rows);
    }

    #[test]
    fn too_many_changes_gives_up() {
        let a: Vec<u64> = (0..30_000).collect();
        let b: Vec<u64> = (100_000..130_000).collect();
        assert!(diff(&a, &b).is_none());
    }
}
