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
    /// 文字コードを指定する（`None` なら、ファイルごとに自動判定）
    pub encoding: Option<&'static encoding_rs::Encoding>,
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
    /// 一致の位置（`text` の中の UTF-16 の位置と長さ。切り詰めた分は含めない）
    pub col: usize,
    pub col_len: usize,
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
    let doc = Document::open_scan_with(path, opts.encoding).map_err(|_| FileError::Open)?;
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
        let ls = doc.line_start_at_or_before(s);
        let col = doc.decode(&doc.read(ls, s)).encode_utf16().count();
        let col_len = doc.decode(&doc.read(s, e.min(ls + 4096))).encode_utf16().count();
        hits.push(GrepHit { file, line, start: s, end: e, text: line_text(&doc, s), col, col_len });
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

/// バックアップの保存先：`名前.拡張子.bak`。すでにあれば `.bak2`、`.bak3` … と、上書きしない名前にする
pub fn backup_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let mut n = 1u32;
    loop {
        let suffix = if n == 1 { ".bak".to_string() } else { format!(".bak{n}") };
        let candidate = path.with_file_name(format!("{name}{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// GREP 置換の結果
#[derive(Debug, Default)]
pub struct ReplaceReport {
    /// 書き換えたファイル
    pub files_changed: usize,
    /// 置換した件数
    pub replaced: usize,
    /// 作ったバックアップの数
    pub backups: usize,
    /// 書き換えられなかったファイルと理由
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

/// `files` のそれぞれで `pattern` に一致する箇所をすべて `replacement` に置き換えて、ファイルを書き換える。
/// 一致の位置は GREP の結果ではなく、書き換える直前にファイルを探し直して求める
/// （ファイルが変わっていても正しい箇所だけが変わる）。文字コードと改行コードは各ファイルのまま。
/// `replacement` は正規表現のときは `$1` などを含められる（`\n` などの展開は呼び出し側で済ませる）
pub fn grep_replace(files: &[PathBuf], pattern: &str, search: SearchOptions, replacement: &str, backup: bool, prog: &GrepProgress) -> ReplaceReport {
    use crate::editor::Editor;
    let mut report = ReplaceReport::default();
    let backups = AtomicUsize::new(0);
    prog.files_total.store(files.len(), Ordering::Relaxed);
    for path in files {
        if prog.ctl.cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let result = (|| -> Result<usize, String> {
            let doc = Document::open(path).map_err(|e| e.to_string())?;
            // 編集には行の索引が必要
            while !doc.progress().done {
                if prog.ctl.cancel.load(Ordering::Relaxed) {
                    return Err("中止".into());
                }
                thread::sleep(std::time::Duration::from_millis(1));
            }
            if is_binary(&doc) {
                return Err("バイナリのため対象外".into());
            }
            let q = Query::new(&doc, pattern, search)?;
            let found = q.find_all(&doc, Some(replacement), usize::MAX, &prog.ctl);
            if found.cancelled {
                return Err("中止".into());
            }
            if found.ranges.is_empty() {
                return Ok(0);
            }
            let mut ed = Editor::new(Arc::new(doc));
            ed.replace_all(&found.ranges, &found.replacements, (0, 0)).map_err(|e| e.to_string())?;
            let edited = ed.doc().clone();
            let temp = edited.write_temp_with(path, &AtomicU64::new(0), &prog.ctl.cancel, None).map_err(|e| e.to_string())?;
            // マップを閉じてから置き換える（Windows ではマップ中のファイルは置き換えられない）
            drop(edited);
            drop(ed);
            // 置き換える前に、元のファイルを残す（残せなければ、そのファイルは書き換えない）
            if backup {
                let bak = backup_path(path);
                if let Err(e) = std::fs::copy(path, &bak) {
                    let _ = std::fs::remove_file(&temp);
                    return Err(format!("バックアップを作れません（{}）: {e}", bak.display()));
                }
                backups.fetch_add(1, Ordering::Relaxed);
            }
            if let Err(e) = crate::document::commit_save(&temp, path) {
                let _ = std::fs::remove_file(&temp);
                return Err(e.to_string());
            }
            Ok(found.ranges.len())
        })();
        match result {
            Ok(0) => {}
            Ok(n) => {
                report.files_changed += 1;
                report.replaced += n;
            }
            Err(e) if e == "中止" => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push((path.clone(), e)),
        }
        prog.files_done.fetch_add(1, Ordering::Relaxed);
    }
    report.backups = backups.load(Ordering::Relaxed);
    report
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
            search: SearchOptions { case_sensitive: true, regex: false, whole_word: false },
            include: parse_include("*.log"),
            recursive: true,
            max_hits: 1000,
            encoding: None,
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
        let bad = GrepOptions { search: SearchOptions { case_sensitive: true, regex: true, whole_word: false }, ..opts };
        assert!(grep(&dir, "(", &bad, &GrepProgress::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_across_files() {
        let dir = std::env::temp_dir().join(format!("kazakiri-grep-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.log");
        let b = dir.join("b.log");
        let c = dir.join("c.log");
        std::fs::write(&a, "foo=1\nbar foo\n").unwrap();
        std::fs::write(&b, encoding_rs::SHIFT_JIS.encode("名前=foo\r\nfoo 太郎\r\n").0.as_ref()).unwrap();
        std::fs::write(&c, "nothing here\n").unwrap();
        let prog = GrepProgress::default();
        let opts = SearchOptions { case_sensitive: true, regex: false, whole_word: false };
        let r = grep_replace(&[a.clone(), b.clone(), c.clone()], "foo", opts, "花子", true, &prog);
        assert_eq!((r.files_changed, r.replaced), (2, 4), "{r:?}");
        assert!(r.failed.is_empty() && !r.cancelled);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "花子=1\nbar 花子\n");
        // Shift_JIS・CRLF のまま
        let bytes = std::fs::read(&b).unwrap();
        let (s, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
        assert_eq!(s, "名前=花子\r\n花子 太郎\r\n");
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "nothing here\n");
        // バックアップ：置換した 2 つだけ、元の内容のまま残る。一致のなかった c にはない
        assert_eq!(r.backups, 2);
        assert_eq!(std::fs::read_to_string(dir.join("a.log.bak")).unwrap(), "foo=1\nbar foo\n");
        assert_eq!(encoding_rs::SHIFT_JIS.decode(&std::fs::read(dir.join("b.log.bak")).unwrap()).0, "名前=foo\r\nfoo 太郎\r\n");
        assert!(!dir.join("c.log.bak").exists());
        // 正規表現のキャプチャ
        let opts = SearchOptions { case_sensitive: true, regex: true, whole_word: false };
        let r = grep_replace(std::slice::from_ref(&a), r"(\S+)=(\d)", opts, "$2:$1", false, &prog);
        assert_eq!((r.replaced, r.backups), (1, 0));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "1:花子\nbar 花子\n");
        // バックアップは上書きせず、番号を増やす（.bak は最初の内容のまま）
        let r = grep_replace(std::slice::from_ref(&a), "花子", opts, "太郎", true, &prog);
        assert_eq!(r.backups, 1);
        assert_eq!(std::fs::read_to_string(dir.join("a.log.bak")).unwrap(), "foo=1\nbar foo\n");
        assert_eq!(std::fs::read_to_string(dir.join("a.log.bak2")).unwrap(), "1:花子\nbar 花子\n");
        // 壊れた正規表現は書き換えずに失敗として報告
        let r = grep_replace(std::slice::from_ref(&a), "(", opts, "x", true, &prog);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "1:太郎\nbar 太郎\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
