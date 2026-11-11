//! ファイルの分割・結合。
//!
//! 分割は必ず行の境目で切る。数GBのファイルでも、元をメモリマップのまま少しずつ書き出すので、メモリは一定。
//! 結合は、ファイルを順につなぐだけ（BOM の重複と、改行で終わっていないファイルの継ぎ目を整える）。

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::document::Document;
use crate::search::SearchControl;

/// 分割の単位
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitBy {
    /// 1 つのファイルの行数
    Lines(u64),
    /// 1 つのファイルのおおよそのバイト数（行の境目で切るので、少し超える）
    Bytes(u64),
}

/// 「1000000」（行数）や「512MB」「2GB」「100KB」（サイズ）のような入力を読む
pub fn parse_split_spec(input: &str) -> Result<SplitBy, String> {
    let t: String = input.trim().replace([',', ' '], "").to_ascii_uppercase();
    if t.is_empty() {
        return Err("分割の単位を入力してください（例: 1000000 行、512MB）".into());
    }
    let (num, mult) = if let Some(n) = t.strip_suffix("GB").or_else(|| t.strip_suffix('G')) {
        (n, Some(1u64 << 30))
    } else if let Some(n) = t.strip_suffix("MB").or_else(|| t.strip_suffix('M')) {
        (n, Some(1u64 << 20))
    } else if let Some(n) = t.strip_suffix("KB").or_else(|| t.strip_suffix('K')) {
        (n, Some(1u64 << 10))
    } else {
        (t.as_str(), None)
    };
    let n: u64 = num.parse().map_err(|_| format!("読めません: {input}（例: 1000000 行、512MB）"))?;
    if n == 0 {
        return Err("0 は指定できません".into());
    }
    Ok(match mult {
        Some(m) => SplitBy::Bytes(n.checked_mul(m).ok_or("大きすぎます")?),
        None => SplitBy::Lines(n),
    })
}

/// 分割後のファイル名：`名前.part001.拡張子`
pub fn part_path(base: &Path, n: usize) -> PathBuf {
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let name = match base.extension() {
        Some(ext) => format!("{stem}.part{n:03}.{}", ext.to_string_lossy()),
        None => format!("{stem}.part{n:03}"),
    };
    base.with_file_name(name)
}

/// 分割する位置（各部分の先頭のバイト位置。最後に全体の長さを付ける）
pub fn cut_points(doc: &Document, by: SplitBy) -> Result<Vec<u64>, String> {
    if !doc.progress().done {
        return Err("索引の作成中は分割できません".into());
    }
    let start = doc.text_start() as u64;
    let len = doc.len();
    let mut cuts = vec![start];
    match by {
        SplitBy::Lines(n) => {
            if n == 0 {
                return Err("行数は 1 以上にしてください".into());
            }
            let total = doc.line_count().0;
            // 最後が空の行（ファイルが改行で終わる）は数えない
            let last_empty = total > 0 && doc.line_range(total - 1).is_some_and(|(s, e)| s == e);
            let lines = if last_empty { total - 1 } else { total };
            let mut k = n;
            while k < lines {
                let Some(p) = doc.line_start(k) else { break };
                cuts.push(p);
                k += n;
            }
        }
        SplitBy::Bytes(n) => {
            if n == 0 {
                return Err("サイズは 1 バイト以上にしてください".into());
            }
            let mut pos = start;
            loop {
                let target = pos.saturating_add(n);
                if target >= len {
                    break;
                }
                let next = doc.line_start_at_or_after(target);
                // 1 行が分割サイズより長いときは、その行の次で切る
                if next <= pos || next >= len {
                    break;
                }
                cuts.push(next);
                pos = next;
            }
        }
    }
    cuts.push(len);
    Ok(cuts)
}

/// `doc` を分割して `base` の隣に書き出す。書き出したファイルを返す。
/// `repeat_header` なら、2 つ目以降の先頭に 1 行目（CSV の見出し）を付ける
pub fn split_file(doc: &Document, base: &Path, by: SplitBy, repeat_header: bool, written: &AtomicU64, ctl: &SearchControl) -> Result<Vec<PathBuf>, String> {
    let cuts = cut_points(doc, by)?;
    let bom = doc.read(0, doc.text_start() as u64).into_owned();
    let header = if repeat_header && cuts.len() > 2 {
        let end = doc.line_start(1).unwrap_or(0).min(cuts[1]);
        doc.read(cuts[0], end).into_owned()
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    for (i, w) in cuts.windows(2).enumerate() {
        let path = part_path(base, i + 1);
        let r = write_part(doc, &path, w[0], w[1], i > 0, &bom, &header, written, ctl);
        if let Err(e) = r {
            // 途中で失敗・中止したら、ここまでに作ったファイルも消す
            let _ = std::fs::remove_file(&path);
            for p in &out {
                let _ = std::fs::remove_file(p);
            }
            return Err(e.to_string());
        }
        out.push(path);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn write_part(doc: &Document, path: &Path, a: u64, b: u64, not_first: bool, bom: &[u8], header: &[u8], written: &AtomicU64, ctl: &SearchControl) -> io::Result<()> {
    const BLOCK: u64 = 4 << 20;
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
    // 先頭の部分は BOM つきで始まる範囲（a は本文の先頭）なので、2 つ目以降にだけ BOM を付ける
    if not_first {
        w.write_all(bom)?;
        w.write_all(header)?;
    } else {
        w.write_all(bom)?;
    }
    let mut pos = a;
    while pos < b {
        if ctl.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "分割を中止しました"));
        }
        let end = (pos + BLOCK).min(b);
        let buf = doc.read(pos, end);
        w.write_all(&buf)?;
        written.fetch_add(buf.len() as u64, Ordering::Relaxed);
        ctl.scanned.fetch_add(buf.len() as u64, Ordering::Relaxed);
        pos = end;
    }
    w.flush()?;
    Ok(())
}

/// ファイルを順につなげて `out` に書く。書いたバイト数を返す。
/// 2 つ目以降の先頭の BOM は、最初のファイルと同じなら取り除く。改行で終わっていないファイルの後ろには改行を足す
pub fn join_files(paths: &[PathBuf], out: &Path, written: &AtomicU64, ctl: &SearchControl) -> io::Result<u64> {
    use std::io::Read;
    if paths.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "結合するファイルがありません"));
    }
    let temp = out.with_file_name(format!(".{}.kazakiri-join", out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let result = (|| -> io::Result<u64> {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&temp)?);
        let mut total = 0u64;
        let mut first_bom: Vec<u8> = Vec::new();
        let mut last_byte: Option<u8> = None;
        let mut utf16: Option<bool> = None; // Some(true)=LE
        let mut buf = vec![0u8; 4 << 20];
        for (i, p) in paths.iter().enumerate() {
            let mut f = File::open(p)?;
            let mut head = [0u8; 3];
            let n = f.read(&mut head)?;
            let bom_len = if head[..n].starts_with(&[0xEF, 0xBB, 0xBF]) {
                3
            } else if head[..n].starts_with(&[0xFF, 0xFE]) || head[..n].starts_with(&[0xFE, 0xFF]) {
                2
            } else {
                0
            };
            let bom = head[..bom_len].to_vec();
            if i == 0 {
                first_bom = bom.clone();
                utf16 = match first_bom.as_slice() {
                    [0xFF, 0xFE] => Some(true),
                    [0xFE, 0xFF] => Some(false),
                    _ => None,
                };
            }
            // 継ぎ目：前のファイルが改行で終わっていなければ改行を足す
            if i > 0 && last_byte.is_some_and(|b| b != b'\n' && b != b'\r') {
                let nl: &[u8] = match utf16 {
                    Some(true) => b"\n\0",
                    Some(false) => b"\0\n",
                    None => b"\n",
                };
                w.write_all(nl)?;
                total += nl.len() as u64;
            }
            // 先頭の BOM は、最初のファイルだけ残す（同じ BOM なら取り除く）
            let keep_from = if i == 0 || bom != first_bom { 0 } else { bom_len };
            if keep_from < n {
                w.write_all(&head[keep_from..n])?;
                total += (n - keep_from) as u64;
                last_byte = Some(head[n - 1]);
            }
            loop {
                if ctl.cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "結合を中止しました"));
                }
                let m = f.read(&mut buf)?;
                if m == 0 {
                    break;
                }
                w.write_all(&buf[..m])?;
                total += m as u64;
                last_byte = Some(buf[m - 1]);
                written.fetch_add(m as u64, Ordering::Relaxed);
            }
        }
        w.flush()?;
        Ok(total)
    })();
    match result {
        Ok(n) => {
            std::fs::rename(&temp, out).inspect_err(|_| {
                let _ = std::fs::remove_file(&temp);
            })?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(bytes: Vec<u8>) -> Document {
        let d = Document::from_bytes(bytes);
        while !d.progress().done {
            std::thread::yield_now();
        }
        d
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kazakiri-split-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn split_by_lines_and_join_roundtrip() {
        let dir = tmpdir("lines");
        let text: String = (1..=10).map(|i| format!("line{i}\r\n")).collect();
        let d = ready(text.clone().into_bytes());
        let base = dir.join("log.txt");
        let w = AtomicU64::new(0);
        let parts = split_file(&d, &base, SplitBy::Lines(4), false, &w, &SearchControl::default()).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].file_name().unwrap(), "log.part001.txt");
        assert_eq!(std::fs::read_to_string(&parts[0]).unwrap(), "line1\r\nline2\r\nline3\r\nline4\r\n");
        assert_eq!(std::fs::read_to_string(&parts[2]).unwrap(), "line9\r\nline10\r\n");
        // 結合すると元に戻る
        let out = dir.join("joined.txt");
        let n = join_files(&parts, &out, &w, &SearchControl::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), text);
        assert_eq!(n, text.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_by_bytes_cuts_at_line_boundaries() {
        let dir = tmpdir("bytes");
        let text: String = (1..=100).map(|i| format!("row-{i:03}\n")).collect(); // 8 バイト×100
        let d = ready(text.clone().into_bytes());
        let parts = split_file(&d, &dir.join("a.csv"), SplitBy::Bytes(100), false, &AtomicU64::new(0), &SearchControl::default()).unwrap();
        assert!(parts.len() >= 7 && parts.len() <= 9, "{}", parts.len());
        let mut joined = String::new();
        for p in &parts {
            let s = std::fs::read_to_string(p).unwrap();
            assert!(s.ends_with('\n') && s.starts_with("row-"), "行の途中で切れている: {s:?}");
            joined.push_str(&s);
        }
        assert_eq!(joined, text);
        // 1 行が分割サイズより長くても、行の途中では切らない
        let d = ready(b"aaaaaaaaaaaaaaaaaaaa\nbb\n".to_vec());
        let parts = split_file(&d, &dir.join("b.txt"), SplitBy::Bytes(5), false, &AtomicU64::new(0), &SearchControl::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&parts[0]).unwrap(), "aaaaaaaaaaaaaaaaaaaa\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn header_and_bom_are_repeated() {
        let dir = tmpdir("header");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"id,name\n1,a\n2,b\n3,c\n4,d\n");
        let d = ready(bytes);
        let parts = split_file(&d, &dir.join("t.csv"), SplitBy::Lines(2), true, &AtomicU64::new(0), &SearchControl::default()).unwrap();
        assert_eq!(parts.len(), 3);
        // 1 つ目の行数には見出しを含む（見出し + 1 行）。2 つ目以降は BOM と見出しが付く
        let p0 = std::fs::read(&parts[0]).unwrap();
        assert_eq!(&p0[..3], &[0xEF, 0xBB, 0xBF]);
        let p1 = std::fs::read(&parts[1]).unwrap();
        assert!(p1.starts_with(b"\xEF\xBB\xBFid,name\n"), "{:?}", String::from_utf8_lossy(&p1));
        // 結合すると、2 つ目以降の BOM は取り除かれる（見出しは重複する）
        let out = dir.join("j.csv");
        join_files(&parts, &out, &AtomicU64::new(0), &SearchControl::default()).unwrap();
        let j = std::fs::read(&out).unwrap();
        assert_eq!(j.iter().filter(|w| **w == 0xEF).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_parsing() {
        assert_eq!(parse_split_spec("1000000"), Ok(SplitBy::Lines(1_000_000)));
        assert_eq!(parse_split_spec("1,000 "), Ok(SplitBy::Lines(1000)));
        assert_eq!(parse_split_spec("512mb"), Ok(SplitBy::Bytes(512 << 20)));
        assert_eq!(parse_split_spec("2 GB"), Ok(SplitBy::Bytes(2 << 30)));
        assert_eq!(parse_split_spec("100K"), Ok(SplitBy::Bytes(100 << 10)));
        for bad in ["", "abc", "0", "0MB", "-5", "1.5MB"] {
            assert!(parse_split_spec(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn join_adds_missing_newline_and_rejects_empty() {
        let dir = tmpdir("join");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "one\ntwo").unwrap(); // 改行で終わっていない
        std::fs::write(&b, "three\n").unwrap();
        let out = dir.join("o.txt");
        join_files(&[a, b], &out, &AtomicU64::new(0), &SearchControl::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "one\ntwo\nthree\n");
        assert!(join_files(&[], &out, &AtomicU64::new(0), &SearchControl::default()).is_err());
        assert!(cut_points(&ready(b"x\n".to_vec()), SplitBy::Lines(0)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
