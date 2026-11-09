//! GREP：フォルダの中のファイルをまとめて検索する。
//!
//! ファイルごとに文字コードを判定し、その文字コードで検索語を探す
//! （Shift_JIS と UTF-8 のファイルが混ざっていても同じ検索語で見つかる）。
//! ファイル単位で複数スレッドに分けて探す。行インデックスは作らず、
//! 行番号は一致の位置までの改行を数えて求める。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::document::Document;
use crate::line_index::{next_line_start, Newline};
use crate::search::{Query, SearchControl, SearchOptions};

/// 結果に載せる 1 行の最大文字数
const MAX_TEXT_CHARS: usize = 400;

#[derive(Debug, Clone)]
pub struct GrepOptions {
    pub search: SearchOptions,
    /// 対象にするファイル名のパターン（`*.log` など）。空ならすべて
    pub include: Vec<String>,
    /// サブフォルダも探すか
    pub recursive: bool,
    /// 一致の最大件数（超えたら打ち切る）
    pub max_hits: usize,
}

#[derive(Debug, Clone)]
pub struct GrepHit {
    /// `GrepResult::files` の番号
    pub file: usize,
    /// 行番号（0 始まり）
    pub line: u64,
    pub start: u64,
    pub end: u64,
    /// その行の文字列（長ければ切り詰める）
    pub text: String,
}

#[derive(Debug, Default)]
pub struct GrepResult {
    pub files: Vec<PathBuf>,
    pub hits: Vec<GrepHit>,
    /// 一致があったファイルの数
    pub matched_files: usize,
    /// バイナリとみなして飛ばしたファイルの数
    pub skipped_binary: usize,
    /// 開けなかったファイルの数
    pub errors: usize,
    pub truncated: bool,
    pub cancelled: bool,
}

/// 進み具合（別スレッドから読む）とキャンセル
#[derive(Default)]
pub struct GrepProgress {
    pub files_total: AtomicUsize,
    pub files_done: AtomicUsize,
    pub hits: AtomicUsize,
    /// 検索のキャンセルと、読んだバイト数
    pub ctl: SearchControl,
}

impl GrepProgress {
    pub fn cancel(&self) {
        self.ctl.cancel.store(true, Ordering::Relaxed);
    }
}

/// `*` と `?` だけの簡単なパターン（大文字小文字は区別しない）
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// `*.log;*.txt` のような文字列をパターンの並びにする
pub fn parse_include(s: &str) -> Vec<String> {
    s.split([';', ',', ' ', '|']).map(str::trim).filter(|p| !p.is_empty() && *p != "*" && *p != "*.*").map(String::from).collect()
}

fn collect_files(root: &Path, opts: &GrepOptions, prog: &GrepProgress) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if prog.ctl.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                // 隠しフォルダ（.git など）は見ない
                if opts.recursive && !name.starts_with('.') {
                    stack.push(e.path());
                }
            } else if ft.is_file() && (opts.include.is_empty() || opts.include.iter().any(|p| glob_match(p, &name))) {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// [a, b) にある改行の数
fn count_newlines(doc: &Document, a: u64, b: u64) -> u64 {
    if b <= a {
        return 0;
    }
    let buf = doc.read(a, b);
    match doc.newline() {
        Newline::Lf => memchr::memchr_iter(b'\n', &buf).count() as u64,
        Newline::Cr => memchr::memchr_iter(b'\r', &buf).count() as u64,
        kind => {
            // UTF-16 は a が偶数（文字の境目）なので、buf の中の偶奇もそのまま使える
            let mut n = 0;
            let mut pos = 0;
            while let Some(next) = next_line_start(&buf, pos, kind) {
                n += 1;
                pos = next;
            }
            n
        }
    }
}

/// 一致を含む行の文字列
fn line_text(doc: &Document, s: u64) -> String {
    let ls = doc.line_start_at_or_before(s);
    let le = doc.line_start_at_or_after(s + 1).min(ls + 4096);
    let bytes = doc.read(ls, le);
    let text = doc.decode(&bytes);
    let t = text.trim_end_matches(['\r', '\n']);
    let mut out: String = t.chars().take(MAX_TEXT_CHARS).collect();
    if t.chars().count() > MAX_TEXT_CHARS {
        out.push('…');
    }
    out
}

/// バイナリファイルか（先頭に NUL がある。UTF-16 は除く）
fn is_binary(doc: &Document) -> bool {
    if doc.encoding().is_utf16() {
        return false;
    }
    let head = doc.read(0, 8000);
    memchr::memchr(0, &head).is_some()
}

/// ファイル 1 つを探す
fn grep_file(path: &Path, file: usize, pattern: &str, opts: &GrepOptions, prog: &GrepProgress, cache: &mut HashMap<&'static str, Query>) -> Result<Vec<GrepHit>, FileError> {
    let meta = std::fs::metadata(path).map_err(|_| FileError::Open)?;
    if meta.len() == 0 {
        return Ok(Vec::new());
    }
    let doc = Document::open_scan(path).map_err(|_| FileError::Open)?;
    if is_binary(&doc) {
        return Err(FileError::Binary);
    }
    let enc = doc.encoding().encoding.name();
    if !cache.contains_key(enc) {
        let q = Query::new(&doc, pattern, opts.search).map_err(|_| FileError::Open)?;
        cache.insert(enc, q);
    }
    let q = &cache[enc];
    let remaining = opts.max_hits.saturating_sub(prog.hits.load(Ordering::Relaxed));
    let found = q.find_all(&doc, None, remaining + 1, &prog.ctl);
    let mut hits = Vec::with_capacity(found.ranges.len());
    let (mut pos, mut line) = (doc.text_start() as u64, 0u64);
    let mut last_line = None;
    for &(s, e) in &found.ranges {
        line += count_newlines(&doc, pos, s);
        pos = s;
        // 同じ行に何度も一致したら 1 行だけ載せる
        if last_line == Some(line) {
            continue;
        }
        last_line = Some(line);
        hits.push(GrepHit { file, line, start: s, end: e, text: line_text(&doc, s) });
    }
    prog.hits.fetch_add(hits.len(), Ordering::Relaxed);
    Ok(hits)
}

enum FileError {
    Open,
    Binary,
}

/// `root` の下のファイルを探す
pub fn grep(root: &Path, pattern: &str, opts: &GrepOptions, prog: &GrepProgress) -> Result<GrepResult, String> {
    // 検索語の誤り（正規表現など）は最初に知らせる
    Query::new(&Document::from_bytes(Vec::new()), pattern, opts.search)?;
    if !root.is_dir() {
        return Err(format!("フォルダが見つかりません: {}", root.display()));
    }
    let files = collect_files(root, opts, prog);
    prog.files_total.store(files.len(), Ordering::Relaxed);
    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let binary = AtomicU64::new(0);
    let errors = AtomicU64::new(0);
    let truncated = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<Vec<GrepHit>>();
    let threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);

    thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let (files, next) = (files.clone(), next.clone());
            let (binary, errors, truncated) = (&binary, &errors, &truncated);
            s.spawn(move || {
                let mut cache = HashMap::new();
                loop {
                    if prog.ctl.cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    if prog.hits.load(Ordering::Relaxed) > opts.max_hits {
                        truncated.store(true, Ordering::Relaxed);
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { return };
                    match grep_file(path, i, pattern, opts, prog, &mut cache) {
                        Ok(h) if !h.is_empty() => {
                            let _ = tx.send(h);
                        }
                        Ok(_) => {}
                        Err(FileError::Binary) => {
                            binary.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(FileError::Open) => {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    prog.files_done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    drop(tx);
    let mut per_file: Vec<Vec<GrepHit>> = rx.into_iter().collect();
    per_file.sort_by_key(|h| h[0].file);
    let matched_files = per_file.len();
    let mut hits: Vec<GrepHit> = per_file.into_iter().flatten().collect();
    let mut truncated = truncated.load(Ordering::Relaxed);
    if hits.len() > opts.max_hits {
        hits.truncate(opts.max_hits);
        truncated = true;
    }
    Ok(GrepResult {
        files: Arc::try_unwrap(files).unwrap_or_else(|a| (*a).clone()),
        hits,
        matched_files,
        skipped_binary: binary.load(Ordering::Relaxed) as usize,
        errors: errors.load(Ordering::Relaxed) as usize,
        truncated,
        cancelled: prog.ctl.cancel.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*.log", "app.LOG"));
        assert!(glob_match("app-??.txt", "app-01.txt"));
        assert!(!glob_match("*.log", "app.log.1"));
        assert!(glob_match("*.log*", "app.log.1"));
        assert_eq!(parse_include("*.log; *.txt,*.csv"), ["*.log", "*.txt", "*.csv"]);
        assert!(parse_include("*.*").is_empty());
    }

    #[test]
    fn grep_mixed_encodings() {
        let dir = std::env::temp_dir().join(format!("kazakiri-grep-{}", std::process::id()));
        let sub = dir.join("sub");
        let hidden = dir.join(".git");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(dir.join("a.log"), "one\ntwo エラー here\nthree\nエラー again エラー\n").unwrap();
        std::fs::write(sub.join("b.log"), encoding_rs::SHIFT_JIS.encode("x\r\ny\r\nz エラー\r\n").0).unwrap();
        let mut u16 = vec![0xFF, 0xFE];
        u16.extend("l1\r\nl2\r\nエラー!\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        std::fs::write(sub.join("c.log"), u16).unwrap();
        std::fs::write(dir.join("d.txt"), "エラー but not a log\n").unwrap();
        std::fs::write(dir.join("e.log"), b"bin\0ary \xE3\x82\xA8\xE3\x83\xA9\xE3\x83\xBC").unwrap();
        std::fs::write(hidden.join("f.log"), "エラー in hidden\n").unwrap();

        let opts = GrepOptions {
            search: SearchOptions { case_sensitive: true, regex: false },
            include: parse_include("*.log"),
            recursive: true,
            max_hits: 1000,
        };
        let prog = GrepProgress::default();
        let r = grep(&dir, "エラー", &opts, &prog).unwrap();
        let mut got: Vec<(String, u64, String)> = r
            .hits
            .iter()
            .map(|h| (r.files[h.file].file_name().unwrap().to_string_lossy().into_owned(), h.line, h.text.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("a.log".into(), 1, "two エラー here".into()),
                ("a.log".into(), 3, "エラー again エラー".into()),
                ("b.log".into(), 2, "z エラー".into()),
                ("c.log".into(), 2, "エラー!".into()),
            ]
        );
        assert_eq!(r.matched_files, 3);
        assert_eq!(r.skipped_binary, 1);
        // サブフォルダなし
        let r = grep(&dir, "エラー", &GrepOptions { recursive: false, ..opts.clone() }, &GrepProgress::default()).unwrap();
        assert_eq!(r.hits.len(), 2);
        // 正規表現の誤りは最初に返す
        let bad = GrepOptions { search: SearchOptions { case_sensitive: true, regex: true }, ..opts };
        assert!(grep(&dir, "(", &bad, &GrepProgress::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
