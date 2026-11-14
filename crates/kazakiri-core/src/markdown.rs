//! Markdown を、表示しやすい形（ブロックと文字の飾り）に直す。描画は呼び出し側（kazakiri-win）が行う。
//!
//! 各ブロックには、もとのテキストでの行（0 から数える）を持たせる。プレビューとエディタの位置合わせや、
//! プレビューのクリックでエディタの該当行へ移るために使う。

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// 同じ飾りが続く文字列
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// リンク先
    pub link: Option<String>,
    /// 画像（`text` は代替文字）。パース後は `Block::Image` に取り出されるので、ふつうは None
    pub image: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ListItem {
    /// タスクリスト（`- [ ]` / `- [x]`）の状態
    pub checked: Option<bool>,
    pub blocks: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Para(Vec<Span>),
    Code { lang: String, text: String },
    Quote(Vec<Node>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Rule,
    Table { align: Vec<Align>, head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    /// 単独の画像（`link` は画像をリンクで包んだときの飛び先）
    Image { url: String, alt: String, link: Option<String> },
    /// HTML のブロック（そのまま文字で見せる）
    Html(String),
    /// 脚注の定義
    Footnote { label: String, blocks: Vec<Node> },
    /// 先頭の YAML メタデータ（--- で囲んだ部分）
    Meta(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    /// もとのテキストでの行（0 から）
    pub line: u32,
    pub block: Block,
}

enum Kind {
    Root,
    Quote,
    Item(Option<bool>),
    List(bool, u64),
    Footnote(String),
}

struct Frame {
    kind: Kind,
    line: u32,
    nodes: Vec<Node>,
    items: Vec<ListItem>,
}

struct Tbl {
    align: Vec<Align>,
    head: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

struct Inl {
    spans: Vec<Span>,
    line: u32,
    /// 見出しなら、その水準
    heading: Option<u8>,
}

struct Builder {
    line_starts: Vec<usize>,
    frames: Vec<Frame>,
    inl: Option<Inl>,
    code: Option<(String, String, u32)>,
    html: Option<(String, u32)>,
    meta: Option<(String, u32)>,
    tbl: Option<Tbl>,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
    /// 画像の中の文字（代替文字）を集める
    image: Option<(String, String)>,
    /// いま読んでいる表の、もとのテキストでの行
    table_line: u32,
}

impl Builder {
    fn line(&self, off: usize) -> u32 {
        self.line_starts.partition_point(|&s| s <= off).saturating_sub(1) as u32
    }

    fn push_node(&mut self, line: u32, block: Block) {
        if let Some(f) = self.frames.last_mut() {
            f.nodes.push(Node { line, block });
        }
    }

    fn begin_inline(&mut self, line: u32, heading: Option<u8>) {
        self.flush_inline();
        self.inl = Some(Inl { spans: Vec::new(), line, heading });
    }

    fn add_text(&mut self, text: &str, line: u32, code: bool) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
            return;
        }
        if self.inl.is_none() {
            // 段落の印がない文字（ゆるくないリストの項目など）は、暗黙の段落に入れる
            self.inl = Some(Inl { spans: Vec::new(), line, heading: None });
        }
        let span = Span {
            text: text.to_string(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
            link: self.links.last().cloned(),
            image: None,
        };
        if let Some(inl) = &mut self.inl {
            // 同じ飾りが続くなら 1 つにまとめる
            match inl.spans.last_mut() {
                Some(l) if l.image.is_none() && (l.bold, l.italic, l.strike, l.code, &l.link) == (span.bold, span.italic, span.strike, span.code, &span.link) => l.text.push_str(&span.text),
                _ => inl.spans.push(span),
            }
        }
    }

    /// 溜めた文字を、段落（見出し）として確定する。画像は単独のブロックに取り出す
    fn flush_inline(&mut self) {
        let Some(inl) = self.inl.take() else { return };
        let mut run: Vec<Span> = Vec::new();
        let line = inl.line;
        let emit = |b: &mut Builder, run: &mut Vec<Span>| {
            if run.is_empty() {
                return;
            }
            let spans = std::mem::take(run);
            let block = match inl.heading {
                Some(level) => Block::Heading { level, spans },
                None => Block::Para(spans),
            };
            b.push_node(line, block);
        };
        for s in inl.spans {
            if let Some(url) = s.image.clone() {
                emit(self, &mut run);
                self.push_node(line, Block::Image { url, alt: s.text, link: s.link });
            } else {
                run.push(s);
            }
        }
        emit(self, &mut run);
    }

    fn handle(&mut self, ev: Event, off: usize) {
        let line = self.line(off);
        match ev {
            Event::Start(tag) => self.start(tag, line),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((s, _)) = &mut self.code.as_mut().map(|(_, s, l)| (s, l)) {
                    s.push_str(&t);
                } else if let Some((s, _)) = &mut self.meta {
                    s.push_str(&t);
                } else if let Some((s, _)) = &mut self.html {
                    s.push_str(&t);
                } else {
                    self.add_text(&t, line, false);
                }
            }
            Event::Code(t) => self.add_text(&t, line, true),
            Event::Html(t) => {
                if let Some((s, _)) = &mut self.html {
                    s.push_str(&t);
                }
            }
            Event::InlineHtml(t) => {
                let tag = t.trim().to_ascii_lowercase();
                if tag.starts_with("<br") {
                    self.add_text("\n", line, false);
                }
            }
            Event::SoftBreak => self.add_text(" ", line, false),
            Event::HardBreak => self.add_text("\n", line, false),
            Event::Rule => {
                self.flush_inline();
                self.push_node(line, Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame { kind: Kind::Item(c), .. }) = self.frames.last_mut() {
                    *c = Some(done);
                }
            }
            Event::FootnoteReference(l) => {
                let t = format!("[{l}]");
                self.add_text(&t, line, false);
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag, line: u32) {
        match tag {
            Tag::Paragraph => self.begin_inline(line, None),
            Tag::Heading { level, .. } => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.begin_inline(line, Some(n));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.frames.push(Frame { kind: Kind::Quote, line, nodes: Vec::new(), items: Vec::new() });
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new(), line));
            }
            Tag::HtmlBlock => {
                self.flush_inline();
                self.html = Some((String::new(), line));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.frames.push(Frame { kind: Kind::List(start.is_some(), start.unwrap_or(1)), line, nodes: Vec::new(), items: Vec::new() });
            }
            Tag::Item => {
                self.flush_inline();
                self.frames.push(Frame { kind: Kind::Item(None), line, nodes: Vec::new(), items: Vec::new() });
            }
            Tag::FootnoteDefinition(l) => {
                self.flush_inline();
                self.frames.push(Frame { kind: Kind::Footnote(l.to_string()), line, nodes: Vec::new(), items: Vec::new() });
            }
            Tag::Table(al) => {
                self.flush_inline();
                let align = al
                    .iter()
                    .map(|a| match a {
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                        _ => Align::Left,
                    })
                    .collect();
                self.tbl = Some(Tbl { align, head: Vec::new(), rows: Vec::new(), row: Vec::new() });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.tbl {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.begin_inline(line, None),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            Tag::MetadataBlock(_) => self.meta = Some((String::new(), line)),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush_inline(),
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(f) = self.frames.pop() {
                    self.push_node(f.line, Block::Quote(f.nodes));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, text, line)) = self.code.take() {
                    self.push_node(line, Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
                }
            }
            TagEnd::HtmlBlock => {
                if let Some((t, line)) = self.html.take() {
                    if !t.trim().is_empty() {
                        self.push_node(line, Block::Html(t.trim_end().to_string()));
                    }
                }
            }
            TagEnd::List(_) => {
                if let Some(f) = self.frames.pop() {
                    if let Kind::List(ordered, start) = f.kind {
                        self.push_node(f.line, Block::List { ordered, start, items: f.items });
                    }
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(f) = self.frames.pop() {
                    if let (Kind::Item(checked), Some(parent)) = (f.kind, self.frames.last_mut()) {
                        parent.items.push(ListItem { checked, blocks: f.nodes });
                    }
                }
            }
            TagEnd::FootnoteDefinition => {
                self.flush_inline();
                if let Some(f) = self.frames.pop() {
                    if let Kind::Footnote(label) = f.kind {
                        self.push_node(f.line, Block::Footnote { label, blocks: f.nodes });
                    }
                }
            }
            TagEnd::TableCell => {
                let spans = self.inl.take().map(|i| i.spans).unwrap_or_default();
                if let Some(t) = &mut self.tbl {
                    t.row.push(spans);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.tbl {
                    t.head = std::mem::take(&mut t.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.tbl {
                    let r = std::mem::take(&mut t.row);
                    t.rows.push(r);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.tbl.take() {
                    let line = self.table_line;
                    self.push_node(line, Block::Table { align: t.align, head: t.head, rows: t.rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    let line = self.inl.as_ref().map_or(0, |i| i.line);
                    if self.inl.is_none() {
                        self.inl = Some(Inl { spans: Vec::new(), line, heading: None });
                    }
                    if let Some(inl) = &mut self.inl {
                        inl.spans.push(Span { text: alt, link: self.links.last().cloned(), image: Some(url), ..Span::default() });
                    }
                }
            }
            TagEnd::MetadataBlock(_) => {
                if let Some((t, line)) = self.meta.take() {
                    self.push_node(line, Block::Meta(t.trim_end().to_string()));
                }
            }
            _ => {}
        }
    }
}

/// Markdown のテキストを解析する
pub fn parse(src: &str) -> Vec<Node> {
    let mut line_starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let mut b = Builder {
        line_starts,
        frames: vec![Frame { kind: Kind::Root, line: 0, nodes: Vec::new(), items: Vec::new() }],
        inl: None,
        code: None,
        html: None,
        meta: None,
        tbl: None,
        bold: 0,
        italic: 0,
        strike: 0,
        links: Vec::new(),
        image: None,
        table_line: 0,
    };
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        if matches!(ev, Event::Start(Tag::Table(_))) {
            b.table_line = b.line(range.start);
        }
        b.handle(ev, range.start);
    }
    b.flush_inline();
    b.frames.pop().map(|f| f.nodes).unwrap_or_default()
}

/// 単独で開ける HTML の文書にする（書き出し用）
pub fn to_html(src: &str, title: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, Parser::new_ext(src, opts));
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>\n{CSS}</style>\n</head>\n<body>\n<article>\n{body}</article>\n</body>\n</html>\n",
        esc(title)
    )
}

const CSS: &str = "body{margin:0;background:#fff;color:#1e1e1e;font-family:'Yu Gothic UI','Segoe UI',sans-serif;line-height:1.7}
article{max-width:900px;margin:0 auto;padding:24px 20px}
h1,h2{border-bottom:1px solid #d0d0d0;padding-bottom:.25em}
code{background:#f3f3f3;padding:.1em .3em;border-radius:3px;font-family:Consolas,'BIZ UDGothic',monospace}
pre{background:#f7f7f7;border:1px solid #d0d0d0;padding:10px 12px;overflow:auto}pre code{background:none;padding:0}
blockquote{margin:1em 0;padding:0 1em;border-left:4px solid #0067c0;color:#555}
table{border-collapse:collapse}th,td{border:1px solid #b0b0b0;padding:4px 10px}th{background:#e4e8ee}
img{max-width:100%}a{color:#0067c0}hr{border:0;border-top:1px solid #b0b0b0}
@media (prefers-color-scheme:dark){body{background:#1e1f22;color:#d8dade}code{background:#2c2e33}pre{background:#23252a;border-color:#4a4f56}th{background:#30343a}th,td{border-color:#4a4f56}blockquote{color:#a0a4aa}a{color:#6cb0f0}}
";

/// 画像のもとの URL がネットワーク上のものか
pub fn is_remote(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://") || u.starts_with("//") || u.starts_with("data:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn headings_and_paragraph_lines() {
        let n = parse("# 見出し\n\n本文 **太字** と `code` と *斜体*。\n\n## 二つ目\n");
        assert_eq!(n.len(), 3);
        assert!(matches!(&n[0].block, Block::Heading { level: 1, spans } if text(spans) == "見出し"));
        assert_eq!(n[0].line, 0);
        let Block::Para(p) = &n[1].block else { panic!() };
        assert_eq!(n[1].line, 2);
        assert!(p.iter().any(|s| s.bold && s.text == "太字"));
        assert!(p.iter().any(|s| s.code && s.text == "code"));
        assert!(p.iter().any(|s| s.italic && s.text == "斜体"));
        assert_eq!(n[2].line, 4);
    }

    #[test]
    fn lists_nested_and_tasks() {
        let n = parse("- a\n- [x] b\n  - c\n\n1. one\n2. two\n");
        let Block::List { ordered: false, items, .. } = &n[0].block else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].checked, Some(true));
        assert!(items[1].blocks.iter().any(|b| matches!(b.block, Block::List { .. })));
        let Block::List { ordered: true, start: 1, items, .. } = &n[1].block else { panic!() };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn code_table_quote_rule() {
        let n = parse("```rust\nfn main() {}\n```\n\n| a | b |\n|:-:|--:|\n| 1 | 2 |\n\n> 引用\n\n---\n");
        assert!(matches!(&n[0].block, Block::Code { lang, text } if lang == "rust" && text == "fn main() {}"));
        let Block::Table { align, head, rows } = &n[1].block else { panic!() };
        assert_eq!(n[1].line, 4);
        assert_eq!(align, &[Align::Center, Align::Right]);
        assert_eq!((head.len(), rows.len()), (2, 1));
        assert!(matches!(n[2].block, Block::Quote(_)));
        assert!(matches!(n[3].block, Block::Rule));
    }

    #[test]
    fn images_and_links() {
        let n = parse("[![CI](https://x/badge.svg)](https://x/ci)\n\n本文 [リンク](https://example.com) と ![図](img/a.png)\n");
        assert!(matches!(&n[0].block, Block::Image { url, alt, link: Some(l) } if url == "https://x/badge.svg" && alt == "CI" && l == "https://x/ci"));
        assert!(matches!(&n[1].block, Block::Para(p) if p.iter().any(|s| s.link.as_deref() == Some("https://example.com"))));
        assert!(matches!(&n[2].block, Block::Image { url, .. } if url == "img/a.png"));
        assert!(is_remote("https://x") && !is_remote("img/a.png"));
    }

    #[test]
    fn export_html() {
        let h = to_html("# 見出し\n\n| a |\n|---|\n| 1 |\n", "t<1>");
        assert!(h.contains("<h1>見出し</h1>") && h.contains("<table>") && h.contains("<title>t&lt;1&gt;</title>"));
    }

    #[test]
    fn front_matter_and_html() {
        let n = parse("---\ntitle: t\n---\n\n<div>x</div>\n\ntext\n");
        assert!(matches!(&n[0].block, Block::Meta(m) if m.contains("title")));
        assert!(matches!(&n[1].block, Block::Html(_)));
    }
}
