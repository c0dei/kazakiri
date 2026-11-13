//! 2 つのファイルの行ごとの差分（左右に並べて見るための、行の対応づけ）。
//!
//! 各行をハッシュにして、共通の先頭・末尾を取り除いてから、Myers の O(ND) 法で最短の編集列を求める。
//! 違いが多すぎる（D が上限を超える）ときは、そのことを返す。

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

/// 違いの数の上限（これを超えたら諦める）
pub const MAX_D: usize = 20_000;

/// 1 行ごとのハッシュ（行末の CR は無視して、CRLF と LF の違いは差分にしない）
pub fn line_hashes(doc: &Document) -> Vec<u64> {
    let n = doc.line_count().0;
    let mut out = Vec::with_capacity(n as usize);
    for l in 0..n {
        let Some((s, e)) = doc.line_range(l) else { break };
        let bytes = doc.read(s, e);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(&bytes);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        out.push(h.finish());
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
    let ops = myers(ma, mb)?;
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

#[derive(Clone, Copy)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Myers の差分。編集列（A → B）を返す
fn myers(a: &[u64], b: &[u64]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 {
        return Some(vec![Op::Insert; b.len()]);
    }
    if m == 0 {
        return Some(vec![Op::Delete; a.len()]);
    }
    let max = ((n + m) as usize).min(MAX_D);
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=max as isize {
        // 各 d の前の状態を記録（戻るときに使う）
        trace.push(v.clone());
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
        // 記録が大きくなりすぎる前に諦める（d が大きいと v のコピーが重い）
        if trace.len() > 4096 && (trace.len() * v.len()) > 400_000_000 {
            return None;
        }
    }
    let d_end = found?;
    // 戻って編集列を作る
    let mut ops: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let vv = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && vv[(off + k - 1) as usize] < vv[(off + k + 1) as usize]) { k + 1 } else { k - 1 };
        let prev_x = vv[(off + prev_k) as usize];
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
    fn too_many_changes_gives_up() {
        let a: Vec<u64> = (0..30_000).collect();
        let b: Vec<u64> = (100_000..130_000).collect();
        assert!(diff(&a, &b).is_none());
    }
}
