//! 編集のランダムテスト。
//! 単純な String を「正解」として、Document に同じ編集を加え、
//! 内容・行・位置→行・検索の結果が一致するかを毎回確かめる。

use encoding_rs::Encoding;
use kazakiri_core::{Direction, Document, Editor, EditKind, Query, SearchControl, SearchOptions, SearchOutcome};
use std::sync::Arc;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

struct Case {
    enc: &'static Encoding,
    bom: &'static [u8],
    /// 行の区切り（モデル側で行を分ける文字）
    sep: char,
    pieces: &'static [&'static str],
}

fn encode(enc: &'static Encoding, s: &str) -> Vec<u8> {
    if enc == encoding_rs::UTF_16LE {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    } else if enc == encoding_rs::UTF_16BE {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    } else {
        enc.encode(s).0.into_owned()
    }
}

/// モデルの文字列での「行の開始位置（バイト）」
fn line_starts(c: &Case, model: &str) -> Vec<u64> {
    let bom = c.bom.len() as u64;
    let mut v = vec![bom];
    let mut acc = String::new();
    for ch in model.chars() {
        acc.push(ch);
        if ch == c.sep {
            v.push(bom + encode(c.enc, &acc).len() as u64);
        }
    }
    v
}

fn byte_pos(c: &Case, model: &str, char_idx: usize) -> u64 {
    let prefix: String = model.chars().take(char_idx).collect();
    c.bom.len() as u64 + encode(c.enc, &prefix).len() as u64
}

fn wait(d: &Document) {
    while !d.progress().done {
        std::thread::yield_now();
    }
}

fn check(c: &Case, doc: &Document, model: &str, rng: &mut Rng) {
    let mut want = c.bom.to_vec();
    want.extend(encode(c.enc, model));
    assert_eq!(doc.len(), want.len() as u64);
    assert_eq!(&*doc.read(0, doc.len()), &want[..], "content");
    // 一部だけ読む
    for _ in 0..5 {
        let a = rng.below(want.len() + 1);
        let b = a + rng.below(want.len() + 1 - a);
        assert_eq!(&*doc.read(a as u64, b as u64), &want[a..b]);
    }
    let starts = line_starts(c, model);
    assert_eq!(doc.line_count(), (starts.len() as u64, true), "line count");
    // CRLF の CR だけが改行の一部（最後の行の末尾の CR は本文）
    let parts: Vec<&str> = model.split(c.sep).collect();
    let n_parts = parts.len();
    let lines: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, l)| if i + 1 < n_parts && c.sep == '\n' { l.strip_suffix('\r').unwrap_or(l) } else { l }.to_string())
        .collect();
    let got = doc.lines(0, starts.len() + 3);
    assert_eq!(got.len(), starts.len());
    for (i, l) in got.iter().enumerate() {
        assert_eq!(l.start, starts[i], "line {i} start");
        assert_eq!(l.text, lines[i], "line {i} text");
        assert_eq!(doc.line_range(i as u64), Some((l.start, l.end)));
    }
    for off in (0..=want.len() as u64).step_by(3) {
        let expect = starts.partition_point(|&s| s <= off).max(1) as u64 - 1;
        assert_eq!(doc.line_of_offset(off), Some(expect), "offset {off}");
    }
}

fn check_search(c: &Case, doc: &Document, model: &str) {
    // モデル中の単語をいくつか探す
    for needle in ["ab", "あい", "Z9"] {
        let mut expected = Vec::new();
        let mut from = 0;
        while let Some(i) = model[from..].find(needle) {
            let ci = model[..from + i].chars().count();
            let s = byte_pos(c, model, ci);
            expected.push(s);
            from += i + needle.len();
        }
        for opts in [
            SearchOptions { case_sensitive: true, regex: false },
            SearchOptions { case_sensitive: true, regex: true },
        ] {
            let q = Query::new(doc, needle, opts).unwrap();
            let ctl = SearchControl::default();
            let mut got = Vec::new();
            let mut pos = 0;
            while let SearchOutcome::Found(s, e) = q.find(doc, pos, Direction::Forward, &ctl) {
                got.push(s);
                pos = e;
            }
            assert_eq!(got, expected, "forward {needle:?} {opts:?}");
            let mut back = Vec::new();
            let mut pos = doc.len();
            while let SearchOutcome::Found(s, _) = q.find(doc, pos, Direction::Backward, &ctl) {
                back.push(s);
                pos = s;
            }
            back.reverse();
            assert_eq!(back, expected, "backward {needle:?} {opts:?}");
        }
    }
}

fn run(c: Case, seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let mut model: String = (0..40).map(|_| c.pieces[rng.below(c.pieces.len())]).collect();
    let mut bytes = c.bom.to_vec();
    bytes.extend(encode(c.enc, &model));
    let doc = Document::from_bytes_with(bytes, Some(c.enc));
    wait(&doc);
    let mut ed = Editor::new(Arc::new(doc));
    let mut history = vec![model.clone()];
    let mut future: Vec<String> = Vec::new();
    for step in 0..steps {
        let n_chars = model.chars().count();
        match rng.below(10) {
            // 元に戻す・やり直し
            0 if history.len() > 1 => {
                ed.undo().unwrap();
                future.push(history.pop().unwrap());
                model = history.last().unwrap().clone();
            }
            // すべて置換（モデルは str::replace と同じ結果になるはず）
            8 => {
                let from = ["ab", "あい", "c"][rng.below(3)];
                let to = c.pieces[rng.below(c.pieces.len())];
                let q = Query::new(ed.doc(), from, SearchOptions { case_sensitive: true, regex: false }).unwrap();
                let r = q.find_all(ed.doc(), Some(to), usize::MAX, &SearchControl::default());
                let n = ed.replace_all(&r.ranges, &r.replacements, (0, 0)).unwrap();
                assert_eq!(n, model.matches(from).count(), "replace count");
                model = model.replace(from, to);
                // 一致がなければ履歴は増えない
                if n > 0 {
                    history.push(model.clone());
                    future.clear();
                }
            }
            9 if !future.is_empty() => {
                ed.redo().unwrap();
                model = future.pop().unwrap();
                history.push(model.clone());
            }
            // 削除
            1..=3 if n_chars > 0 => {
                let a = rng.below(n_chars);
                let b = (a + 1 + rng.below(8)).min(n_chars);
                let (pa, pb) = (byte_pos(&c, &model, a), byte_pos(&c, &model, b));
                ed.break_group();
                ed.replace(pa, pb, "", EditKind::Other, (pa, pb)).unwrap();
                future.clear();
                let mut chars: Vec<char> = model.chars().collect();
                chars.drain(a..b);
                model = chars.into_iter().collect();
                history.push(model.clone());
            }
            // 挿入・置換
            _ => {
                let a = rng.below(n_chars + 1);
                let b = if rng.below(4) == 0 { (a + rng.below(4)).min(n_chars) } else { a };
                let ins: String = (0..1 + rng.below(4)).map(|_| c.pieces[rng.below(c.pieces.len())]).collect();
                let (pa, pb) = (byte_pos(&c, &model, a), byte_pos(&c, &model, b));
                ed.break_group();
                ed.replace(pa, pb, &ins, EditKind::Other, (pa, pb)).unwrap();
                future.clear();
                let chars: Vec<char> = model.chars().collect();
                model = chars[..a].iter().collect::<String>() + &ins + &chars[b..].iter().collect::<String>();
                history.push(model.clone());
            }
        }
        check(&c, ed.doc(), &model, &mut rng);
        if step % 10 == 0 {
            check_search(&c, ed.doc(), &model);
        }
    }
    // 保存して開き直しても同じ
    let written = std::sync::atomic::AtomicU64::new(0);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut out = Vec::new();
    ed.doc().write_to(&mut out, &written, &cancel).unwrap();
    let mut want = c.bom.to_vec();
    want.extend(encode(c.enc, &model));
    assert_eq!(out, want);
}

const WORDS: &[&str] = &["ab", "c", "あい", "う", "Z9", " ", "\n"];
const WORDS_CRLF: &[&str] = &["ab", "c", "あい", "Z9", " ", "\r\n"];
const WORDS_CR: &[&str] = &["ab", "c", "Z9", " ", "\r"];

#[test]
fn utf8_lf() {
    for seed in 1..6 {
        run(Case { enc: encoding_rs::UTF_8, bom: b"", sep: '\n', pieces: WORDS }, seed * 7919, 150);
    }
}

#[test]
fn utf8_bom_crlf() {
    for seed in 1..4 {
        run(Case { enc: encoding_rs::UTF_8, bom: b"\xEF\xBB\xBF", sep: '\n', pieces: WORDS_CRLF }, seed * 104729, 150);
    }
}

#[test]
fn sjis_crlf() {
    for seed in 1..4 {
        run(Case { enc: encoding_rs::SHIFT_JIS, bom: b"", sep: '\n', pieces: WORDS_CRLF }, seed * 31337, 150);
    }
}

#[test]
fn utf16le_and_be() {
    for seed in 1..3 {
        run(Case { enc: encoding_rs::UTF_16LE, bom: b"\xFF\xFE", sep: '\n', pieces: WORDS_CRLF }, seed * 4243, 120);
        run(Case { enc: encoding_rs::UTF_16BE, bom: b"\xFE\xFF", sep: '\n', pieces: WORDS }, seed * 977, 120);
    }
}

#[test]
fn cr_only() {
    for seed in 1..3 {
        run(Case { enc: encoding_rs::UTF_8, bom: b"", sep: '\r', pieces: WORDS_CR }, seed * 6151, 150);
    }
}

#[test]
fn edit_requires_index_and_valid_positions() {
    let doc = Document::from_bytes_with(b"\xEF\xBB\xBFabc".to_vec(), None);
    wait(&doc);
    // BOM の中は編集できない
    assert!(doc.insert(1, b"x").is_err());
    assert!(doc.insert(10, b"x").is_err());
    let d2 = doc.insert(3, b"x").unwrap();
    assert_eq!(&*d2.read(0, d2.len()), b"\xEF\xBB\xBFxabc");
    // 元の Document は変わらない
    assert_eq!(&*doc.read(0, doc.len()), b"\xEF\xBB\xBFabc");
}
