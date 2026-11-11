//! 構文の色分け（軽量）。
//!
//! 1 行ずつ、前の行の状態を持たずに色の付く範囲を求める。複数行にわたる文字列やコメントは追わない
//! （数GB のファイルの、見えている行だけを色分けするため）。言語はファイルの拡張子から決める。

use std::ops::Range;

/// 色の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Str,
    Num,
    Keyword,
    Comment,
    /// タグ名・セクション名
    Tag,
    /// キー（JSON のキー、設定ファイルのキー）
    Key,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Json,
    Sql,
    Xml,
    Ini,
    Yaml,
    /// `//` と `/* */` のコメントを持つ言語（JavaScript, Java, C, C++, C#, Go, Rust, PHP …）
    CLike,
    /// `#` のコメントを持つ言語（Python, シェル, Ruby, PowerShell …）
    Hash,
}

impl Lang {
    pub fn from_extension(ext: &str) -> Option<Lang> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "json" | "jsonl" | "ndjson" | "geojson" => Lang::Json,
            "sql" => Lang::Sql,
            "xml" | "html" | "htm" | "xhtml" | "svg" | "xaml" | "csproj" | "config" | "plist" => Lang::Xml,
            "ini" | "conf" | "cfg" | "properties" | "toml" | "desktop" | "env" => Lang::Ini,
            "yml" | "yaml" => Lang::Yaml,
            "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "java" | "c" | "h" | "cpp" | "cc" | "hpp" | "cs" | "go" | "rs" | "php" | "kt" | "swift" | "dart" | "scala" | "css" | "scss" => Lang::CLike,
            "py" | "sh" | "bash" | "zsh" | "rb" | "ps1" | "pl" | "r" | "mk" | "makefile" | "dockerfile" => Lang::Hash,
            _ => return None,
        })
    }
}

/// 色を付ける範囲の最大文字数（長い行は先頭だけ）
const MAX_CHARS: usize = 2000;

const SQL_KEYWORDS: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "alter", "drop", "join", "left", "right", "inner", "outer", "full", "cross", "on", "group", "by", "order", "having", "limit", "offset", "and",
    "or", "not", "null", "as", "in", "is", "like", "between", "distinct", "union", "all", "case", "when", "then", "else", "end", "index", "primary", "key", "foreign", "references", "default", "view", "begin", "commit", "rollback", "exists",
    "asc", "desc", "count", "sum", "avg", "min", "max", "top", "with", "returning", "truncate", "grant", "revoke", "unique", "check", "constraint", "database", "schema", "if", "using",
];

const C_KEYWORDS: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "case", "break", "continue", "return", "class", "struct", "enum", "interface", "function", "func", "fn", "def", "let", "const", "var", "val", "public", "private", "protected", "static",
    "final", "void", "int", "long", "float", "double", "bool", "boolean", "char", "string", "new", "this", "super", "null", "nil", "true", "false", "import", "export", "package", "use", "mod", "pub", "impl", "trait", "match", "try", "catch",
    "throw", "throws", "finally", "async", "await", "yield", "namespace", "using", "typeof", "instanceof", "in", "of", "from", "as", "extends", "implements", "default", "undefined", "type", "where", "mut", "self", "go", "defer", "chan",
];

const HASH_KEYWORDS: &[&str] = &[
    "if", "elif", "else", "fi", "then", "for", "while", "do", "done", "in", "case", "esac", "def", "class", "return", "import", "from", "as", "with", "try", "except", "finally", "raise", "lambda", "pass", "break", "continue", "and", "or", "not",
    "is", "None", "True", "False", "self", "end", "function", "local", "export", "echo", "begin", "module", "yield", "nil", "true", "false", "param", "foreach",
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `s` の先頭から数えて最大 `MAX_CHARS` 文字までの部分
fn head(s: &str) -> &str {
    match s.char_indices().nth(MAX_CHARS) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 1 行の色分け。範囲は `line` の中のバイト位置で、重ならず、小さい順
pub fn highlight(lang: Lang, line: &str) -> Vec<(Range<usize>, Kind)> {
    let s = head(line);
    let mut out = Vec::new();
    match lang {
        Lang::Json => json(s, &mut out),
        Lang::Sql => code(s, &mut out, Style { line_comment: Some("--"), block: true, quotes: &['\''], kw: Words::Sql, ci: true }),
        Lang::Xml => xml(s, &mut out),
        Lang::Ini => ini(s, &mut out),
        Lang::Yaml => yaml(s, &mut out),
        Lang::CLike => code(s, &mut out, Style { line_comment: Some("//"), block: true, quotes: &['"', '\'', '`'], kw: Words::C, ci: false }),
        Lang::Hash => code(s, &mut out, Style { line_comment: Some("#"), block: false, quotes: &['"', '\''], kw: Words::Hash, ci: false }),
    }
    out
}

/// 文字列リテラルの終わり（閉じる引用符の次の位置）。閉じていなければ行末
fn string_end(s: &str, start: usize, quote: char, backslash: bool) -> usize {
    let mut it = s[start + quote.len_utf8()..].char_indices();
    let base = start + quote.len_utf8();
    while let Some((i, c)) = it.next() {
        if backslash && c == '\\' {
            it.next();
        } else if c == quote {
            // SQL の '' は ' そのもの
            if !backslash && s[base + i + 1..].starts_with(quote) {
                it.next();
                continue;
            }
            return base + i + c.len_utf8();
        }
    }
    s.len()
}

/// 数値の終わり（始まりが数字のとき）
fn number_end(s: &str, start: usize) -> usize {
    let b = s.as_bytes();
    let mut i = start;
    if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        return i;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
}

fn json(s: &str, out: &mut Vec<(Range<usize>, Kind)>) {
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        match c {
            '"' => {
                let e = string_end(s, i, '"', true);
                // 後ろに : が続くなら、キー
                let key = s[e..].trim_start().starts_with(':');
                out.push((i..e, if key { Kind::Key } else { Kind::Str }));
                i = e;
            }
            '-' | '0'..='9' if c != '-' || s[i + 1..].starts_with(|d: char| d.is_ascii_digit()) => {
                let st = if c == '-' { i + 1 } else { i };
                let e = number_end(s, st);
                out.push((i..e, Kind::Num));
                i = e;
            }
            c if c.is_alphabetic() => {
                let e = i + s[i..].find(|d: char| !is_word(d)).unwrap_or(s.len() - i);
                if matches!(&s[i..e], "true" | "false" | "null") {
                    out.push((i..e, Kind::Keyword));
                }
                i = e;
            }
            c => i += c.len_utf8(),
        }
    }
}

#[derive(Clone, Copy)]
enum Words {
    Sql,
    C,
    Hash,
}

struct Style {
    line_comment: Option<&'static str>,
    /// `/* ... */`（同じ行で閉じるもの。閉じなければ行末まで）
    block: bool,
    quotes: &'static [char],
    kw: Words,
    ci: bool,
}

fn code(s: &str, out: &mut Vec<(Range<usize>, Kind)>, st: Style) {
    let words: &[&str] = match st.kw {
        Words::Sql => SQL_KEYWORDS,
        Words::C => C_KEYWORDS,
        Words::Hash => HASH_KEYWORDS,
    };
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap();
        if let Some(lc) = st.line_comment {
            if rest.starts_with(lc) {
                out.push((i..s.len(), Kind::Comment));
                return;
            }
        }
        if st.block && rest.starts_with("/*") {
            let e = rest[2..].find("*/").map_or(s.len(), |k| i + 2 + k + 2);
            out.push((i..e, Kind::Comment));
            i = e;
            continue;
        }
        if st.quotes.contains(&c) {
            let e = string_end(s, i, c, !matches!(st.kw, Words::Sql));
            out.push((i..e, Kind::Str));
            i = e;
            continue;
        }
        if c.is_ascii_digit() {
            let prev_word = s[..i].chars().next_back().is_some_and(is_word);
            let e = number_end(s, i);
            if !prev_word {
                out.push((i..e, Kind::Num));
            }
            i = e;
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let e = i + rest.find(|d: char| !is_word(d) && d != '$').unwrap_or(rest.len());
            let w = &s[i..e];
            let hit = if st.ci { words.iter().any(|k| k.eq_ignore_ascii_case(w)) } else { words.contains(&w) };
            if hit {
                out.push((i..e, Kind::Keyword));
            }
            i = e.max(i + c.len_utf8());
            continue;
        }
        i += c.len_utf8();
    }
}

fn xml(s: &str, out: &mut Vec<(Range<usize>, Kind)>) {
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let e = after.find("-->").map_or(s.len(), |k| i + 4 + k + 3);
            out.push((i..e, Kind::Comment));
            i = e;
        } else if let Some(after) = rest.strip_prefix('<') {
            // <tag, </tag, <?xml, <!DOCTYPE
            let name_start = i + 1 + after.find(|c: char| !matches!(c, '/' | '?' | '!')).unwrap_or(after.len());
            let name_end = name_start + s[name_start..].find(|c: char| !(is_word(c) || matches!(c, ':' | '-' | '.'))).unwrap_or(s.len() - name_start);
            out.push((i..name_end.max(i + 1), Kind::Tag));
            i = name_end.max(i + 1);
            // 属性の値、タグの終わり
            while i < s.len() {
                let c = s[i..].chars().next().unwrap();
                if c == '"' || c == '\'' {
                    let e = string_end(s, i, c, false);
                    out.push((i..e, Kind::Str));
                    i = e;
                } else if c == '>' {
                    let st = if s[..i].ends_with('/') || s[..i].ends_with('?') { i - 1 } else { i };
                    out.push((st..i + 1, Kind::Tag));
                    i += 1;
                    break;
                } else {
                    i += c.len_utf8();
                }
            }
        } else {
            i += rest.chars().next().unwrap().len_utf8();
        }
    }
}

fn ini(s: &str, out: &mut Vec<(Range<usize>, Kind)>) {
    let t = s.trim_start();
    let off = s.len() - t.len();
    if t.starts_with(';') || t.starts_with('#') {
        out.push((off..s.len(), Kind::Comment));
    } else if t.starts_with('[') {
        let e = t.find(']').map_or(s.len(), |k| off + k + 1);
        out.push((off..e, Kind::Tag));
    } else if let Some(k) = t.find(['=', ':']) {
        if k > 0 && !t[..k].contains(char::is_whitespace) || k > 0 {
            out.push((off..off + t[..k].trim_end().len(), Kind::Key));
        }
        value(s, off + k + 1, out);
    }
}

/// 値の部分（文字列・数値・true/false）
fn value(s: &str, from: usize, out: &mut Vec<(Range<usize>, Kind)>) {
    let mut i = from.min(s.len());
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if c == '"' || c == '\'' {
            let e = string_end(s, i, c, true);
            out.push((i..e, Kind::Str));
            i = e;
        } else if (c == '#' || c == ';') && s[..i].ends_with(char::is_whitespace) {
            out.push((i..s.len(), Kind::Comment));
            return;
        } else if c.is_ascii_digit() && !s[..i].chars().next_back().is_some_and(is_word) {
            let e = number_end(s, i);
            out.push((i..e, Kind::Num));
            i = e;
        } else if c.is_alphabetic() {
            let e = i + s[i..].find(|d: char| !is_word(d)).unwrap_or(s.len() - i);
            if matches!(&s[i..e], "true" | "false" | "yes" | "no" | "on" | "off" | "null") {
                out.push((i..e, Kind::Keyword));
            }
            i = e;
        } else {
            i += c.len_utf8();
        }
    }
}

fn yaml(s: &str, out: &mut Vec<(Range<usize>, Kind)>) {
    let t = s.trim_start();
    let off = s.len() - t.len();
    if t.starts_with('#') {
        out.push((off..s.len(), Kind::Comment));
        return;
    }
    // 「- 」（リスト）を飛ばして、「キー:」
    let body = t.strip_prefix("- ").unwrap_or(t);
    let boff = off + (t.len() - body.len());
    if let Some(k) = body.find(": ").or_else(|| body.strip_suffix(':').map(|b| b.len())) {
        if !body[..k].starts_with(['"', '\'']) && !body[..k].is_empty() {
            out.push((boff..boff + k, Kind::Key));
            value(s, boff + k + 1, out);
            return;
        }
    }
    value(s, boff, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Lang, line: &str) -> Vec<(String, Kind)> {
        highlight(lang, line).into_iter().map(|(r, k)| (line[r].to_string(), k)).collect()
    }

    #[test]
    fn json_keys_strings_numbers() {
        let k = kinds(Lang::Json, r#"  {"name": "x\"y", "n": -12.5e3, "ok": true, "z": null}"#);
        assert_eq!(k[0], ("\"name\"".into(), Kind::Key));
        assert_eq!(k[1], ("\"x\\\"y\"".into(), Kind::Str));
        assert!(k.contains(&("-12.5e3".into(), Kind::Num)));
        assert!(k.contains(&("true".into(), Kind::Keyword)) && k.contains(&("null".into(), Kind::Keyword)));
    }

    #[test]
    fn sql() {
        let k = kinds(Lang::Sql, "SELECT id, 'it''s' FROM t WHERE n >= 10 -- note");
        assert!(k.contains(&("SELECT".into(), Kind::Keyword)));
        assert!(k.contains(&("'it''s'".into(), Kind::Str)));
        assert!(k.contains(&("10".into(), Kind::Num)));
        assert_eq!(k.last().unwrap(), &("-- note".to_string(), Kind::Comment));
        // 識別子の一部の数字は数値にしない
        assert!(!kinds(Lang::Sql, "col1 = 2").iter().any(|(t, _)| t == "1"));
    }

    #[test]
    fn xml_tags() {
        let k = kinds(Lang::Xml, r#"<a href="u" id='x'>text</a><!-- c -->"#);
        assert!(k.contains(&("<a".into(), Kind::Tag)));
        assert!(k.contains(&("\"u\"".into(), Kind::Str)));
        assert!(k.contains(&(">".into(), Kind::Tag)));
        assert!(k.contains(&("</a".into(), Kind::Tag)));
        assert!(k.contains(&("<!-- c -->".into(), Kind::Comment)));
    }

    #[test]
    fn ini_and_yaml() {
        assert_eq!(kinds(Lang::Ini, "[main]"), vec![("[main]".into(), Kind::Tag)]);
        assert_eq!(kinds(Lang::Ini, "; comment"), vec![("; comment".into(), Kind::Comment)]);
        let k = kinds(Lang::Ini, "port = 8080 # c");
        assert_eq!(k[0], ("port".into(), Kind::Key));
        assert!(k.contains(&("8080".into(), Kind::Num)) && k.contains(&("# c".into(), Kind::Comment)));
        let k = kinds(Lang::Yaml, "  - name: \"web\"");
        assert_eq!(k[0], ("name".into(), Kind::Key));
        assert!(k.contains(&("\"web\"".into(), Kind::Str)));
    }

    #[test]
    fn c_like_and_hash() {
        let k = kinds(Lang::CLike, "let x = 0x1F; // hi /* no */");
        assert!(k.contains(&("let".into(), Kind::Keyword)) && k.contains(&("0x1F".into(), Kind::Num)));
        assert_eq!(k.last().unwrap().1, Kind::Comment);
        let k = kinds(Lang::CLike, "a /* b */ + \"s\"");
        assert!(k.contains(&("/* b */".into(), Kind::Comment)) && k.contains(&("\"s\"".into(), Kind::Str)));
        let k = kinds(Lang::Hash, "if x: # tail");
        assert!(k.contains(&("if".into(), Kind::Keyword)) && k.last().unwrap().1 == Kind::Comment);
        // コメント記号が文字列の中にあるときは、コメントにしない
        let k = kinds(Lang::CLike, "u = \"http://x\"");
        assert!(!k.iter().any(|(_, kind)| *kind == Kind::Comment));
    }

    #[test]
    fn extensions_and_robustness() {
        assert_eq!(Lang::from_extension("JSON"), Some(Lang::Json));
        assert_eq!(Lang::from_extension("log"), None);
        // 範囲は重ならず、小さい順で、文字の境目にある（日本語・絵文字・閉じていない文字列でも落ちない）
        for lang in [Lang::Json, Lang::Sql, Lang::Xml, Lang::Ini, Lang::Yaml, Lang::CLike, Lang::Hash] {
            for line in ["日本語 \"あい うえ", "<<>> '' \"\" 🙂 /* x", "[a=b:c] # 🙂", "", "0x", "1e", "-", "\\", "- : :", "$$ a_b ä1"] {
                let r = highlight(lang, line);
                let mut prev = 0;
                for (rg, _) in r {
                    assert!(rg.start >= prev && rg.end >= rg.start && rg.end <= line.len(), "{lang:?} {line:?}");
                    assert!(line.is_char_boundary(rg.start) && line.is_char_boundary(rg.end), "{lang:?} {line:?}");
                    prev = rg.end;
                }
            }
        }
    }
}
