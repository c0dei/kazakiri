//! Markdown のプレビュー。2 つの出し方がある。
//!
//! - 分割：編集する窓の右半分にプレビューを出す。入力するとすぐ更新され、エディタをスクロールするとプレビューも付いてくる
//! - デザインビュー：本文の領域をプレビューに切り替える（EmEditor のデザインビューのように、整形した姿だけを見る）。Esc でもとに戻る
//!
//! どちらも同じ描画を使う。Markdown の解析は kazakiri_core::markdown。ここでは、ブロックを DirectWrite の
//! 文字レイアウトに直して縦に並べ（`Laid`）、見えている部分だけを描く。画像は WIC で読む（ネットワーク上の画像は読まない）。

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kazakiri_core::markdown::{self, Align, Block, ListItem, Node, Span};
use windows::Win32::Graphics::Imaging::*;

use super::*;

/// プレビューできる本文の大きさの上限
pub(super) const MAX_BYTES: u64 = 2 << 20;
/// デザインビューの本文の幅の上限（これより広い窓では中央に寄せる）
const DESIGN_MAX_W: f32 = 980.0;
/// 画像の最大の高さ
const IMAGE_MAX_H: f32 = 640.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MdMode {
    Split,
    Design,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Normal,
    /// 引用など、少し薄い文字
    Muted,
}

#[derive(Clone, Copy)]
enum Fill {
    Code,
    Quote,
    Rule,
    Head,
    Line,
}

enum Item {
    Text { x: f32, y: f32, w: f32, h: f32, layout: IDWriteTextLayout, tone: Tone, links: Vec<(u32, u32, String)> },
    Fill { x: f32, y: f32, w: f32, h: f32, fill: Fill },
    Image { x: f32, y: f32, w: f32, h: f32, path: PathBuf, link: Option<String> },
    Check { x: f32, y: f32, size: f32, checked: bool },
}

/// 並べ終えたプレビュー
pub(super) struct Laid {
    items: Vec<Item>,
    pub height: f32,
    /// (もとのテキストの行, y)。y の小さい順
    marks: Vec<(u32, f32)>,
    /// 見出しの飛び先（`#見出し` のリンク用）
    anchors: Vec<(String, f32)>,
}

impl Laid {
    /// もとのテキストの行に当たる y（行のあいだは補う）
    pub fn y_for_line(&self, line: u32) -> f32 {
        let i = self.marks.partition_point(|&(l, _)| l <= line);
        let Some(&(l0, y0)) = i.checked_sub(1).and_then(|k| self.marks.get(k)) else { return 0.0 };
        match self.marks.get(i) {
            Some(&(l1, y1)) if l1 > l0 => y0 + (y1 - y0) * (line - l0) as f32 / (l1 - l0) as f32,
            _ => y0,
        }
    }

    /// y に当たる、もとのテキストの行
    pub fn line_for_y(&self, y: f32) -> u32 {
        let i = self.marks.partition_point(|&(_, my)| my <= y);
        let Some(&(l0, y0)) = i.checked_sub(1).and_then(|k| self.marks.get(k)) else { return 0 };
        match self.marks.get(i) {
            Some(&(l1, y1)) if y1 > y0 && l1 > l0 => l0 + ((y - y0) / (y1 - y0) * (l1 - l0) as f32) as u32,
            _ => l0,
        }
    }
}

type Key = (usize, u64, u32, u32, bool, u64);

pub(super) struct MdView {
    pub mode: MdMode,
    /// デザインビューのスクロール位置（分割のときは、エディタの先頭行に合わせる）
    pub scroll: Cell<f32>,
    key: Cell<Option<Key>>,
    laid: RefCell<Option<Rc<Laid>>>,
    /// 同じ段落を作り直さないための覚え書き（編集していない段落の再レイアウトを省く）
    layouts: RefCell<HashMap<u64, IDWriteTextLayout>>,
    formats: RefCell<HashMap<(u32, bool), IDWriteTextFormat>>,
    sizes: RefCell<HashMap<PathBuf, Option<(u32, u32)>>>,
    bitmaps: RefCell<HashMap<PathBuf, Option<ID2D1Bitmap>>>,
    /// プレビューの文字が長すぎて出せないとき、その理由
    pub notice: RefCell<Option<String>>,
}

impl MdView {
    pub fn new(mode: MdMode) -> MdView {
        MdView {
            mode,
            scroll: Cell::new(0.0),
            key: Cell::new(None),
            laid: RefCell::new(None),
            layouts: RefCell::new(HashMap::new()),
            formats: RefCell::new(HashMap::new()),
            sizes: RefCell::new(HashMap::new()),
            bitmaps: RefCell::new(HashMap::new()),
            notice: RefCell::new(None),
        }
    }

    /// 描画先が変わった（ブラシ・ビットマップを作り直す）
    pub fn reset(&self) {
        self.key.set(None);
        *self.laid.borrow_mut() = None;
        self.layouts.borrow_mut().clear();
        self.bitmaps.borrow_mut().clear();
    }

    pub fn laid(&self) -> Option<Rc<Laid>> {
        self.laid.borrow().clone()
    }
}

fn hash_of<T: Hash + ?Sized>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// 見出しの飛び先の名前（GitHub と同じ：小文字にして、記号を除き、空白を - にする）
fn slug(spans: &[Span]) -> String {
    let t: String = spans.iter().map(|s| s.text.as_str()).collect();
    t.trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// リンク・画像の参照先を、ファイルの場所に直す（ネットワーク上のものは None）
pub(super) fn resolve_path(url: &str, dir: Option<&Path>) -> Option<PathBuf> {
    let u = url.trim();
    if markdown::is_remote(u) || u.starts_with("mailto:") || u.starts_with('#') {
        return None;
    }
    let u = u.strip_prefix("file:///").or_else(|| u.strip_prefix("file://")).unwrap_or(u);
    let u = u.split(['#', '?']).next().unwrap_or(u);
    let p = PathBuf::from(percent_decode(u));
    if p.is_absolute() {
        return Some(p);
    }
    Some(dir?.join(p))
}

unsafe fn wic_factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// 画像の大きさ（ピクセル）。読めない・大きすぎるときは None
fn image_size(path: &Path) -> Option<(u32, u32)> {
    unsafe {
        let f = wic_factory().ok()?;
        let wp = wide(&path.to_string_lossy());
        let dec = f.CreateDecoderFromFilename(PCWSTR(wp.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        (w > 0 && h > 0 && (w as u64) * (h as u64) <= 40_000_000).then_some((w, h))
    }
}

unsafe fn load_bitmap(rt: &ID2D1RenderTarget, path: &Path) -> Option<ID2D1Bitmap> {
    unsafe {
        let f = wic_factory().ok()?;
        let wp = wide(&path.to_string_lossy());
        let dec = f.CreateDecoderFromFilename(PCWSTR(wp.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        let src: IWICBitmapSource = frame.cast().ok()?;
        conv.Initialize(&src, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
        rt.CreateBitmapFromWicBitmap(&conv, None).ok()
    }
}

/// レイアウトを作る道具
struct Lb<'a> {
    v: &'a MdView,
    dw: &'a IDWriteFactory,
    ui_font: &'a str,
    code_font: &'a str,
    base: f32,
    /// 文字の色を変えるブラシ：[リンク, コード]
    brushes: &'a [IUnknown],
    dir: Option<PathBuf>,
    items: Vec<Item>,
    marks: Vec<(u32, f32)>,
    anchors: Vec<(String, f32)>,
    tone: Tone,
    gap: f32,
}

#[derive(Clone, Copy)]
struct Style {
    size: f32,
    bold: bool,
    mono: bool,
    wrap: bool,
    align: DWRITE_TEXT_ALIGNMENT,
}

impl Lb<'_> {
    fn line_h(&self) -> f32 {
        (self.base * 1.6).ceil()
    }

    fn format(&self, size: f32, mono: bool) -> Option<IDWriteTextFormat> {
        let key = (size.to_bits(), mono);
        if let Some(f) = self.v.formats.borrow().get(&key) {
            return Some(f.clone());
        }
        let f = unsafe {
            if mono {
                create_format(self.dw, self.code_font, size)
            } else {
                create_ui_format(self.dw, self.ui_font, size)
            }
        }
        .ok()?;
        self.v.formats.borrow_mut().insert(key, f.clone());
        Some(f)
    }

    /// 文字のレイアウトを作る（同じ内容・幅なら使い回す）
    fn layout(&self, spans: &[Span], st: Style, width: f32) -> Option<IDWriteTextLayout> {
        let key = hash_of(&(spans, st.size.to_bits(), st.bold, st.mono, st.wrap, st.align.0, width.to_bits(), self.code_font, self.ui_font));
        if let Some(l) = self.v.layouts.borrow().get(&key) {
            return Some(l.clone());
        }
        let format = self.format(st.size, st.mono)?;
        let mut units: Vec<u16> = Vec::new();
        let mut ranges = Vec::with_capacity(spans.len());
        for s in spans {
            let a = units.len() as u32;
            units.extend(s.text.encode_utf16());
            ranges.push(DWRITE_TEXT_RANGE { startPosition: a, length: units.len() as u32 - a });
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(&units, &format, width.max(1.0), 1.0e6).ok()?;
            let _ = layout.SetWordWrapping(if st.wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP });
            let _ = layout.SetTextAlignment(st.align);
            if st.bold && !units.is_empty() {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_RANGE { startPosition: 0, length: units.len() as u32 });
            }
            for (s, r) in spans.iter().zip(&ranges) {
                if r.length == 0 {
                    continue;
                }
                if s.bold {
                    let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, *r);
                }
                if s.italic {
                    let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, *r);
                }
                if s.strike {
                    let _ = layout.SetStrikethrough(true, *r);
                }
                if s.code && !st.mono {
                    let name = wide(self.code_font);
                    let _ = layout.SetFontFamilyName(PCWSTR(name.as_ptr()), *r);
                    if let Some(b) = self.brushes.get(1) {
                        let _ = layout.SetDrawingEffect(b, *r);
                    }
                }
                if s.link.is_some() {
                    let _ = layout.SetUnderline(true, *r);
                    if let Some(b) = self.brushes.first() {
                        let _ = layout.SetDrawingEffect(b, *r);
                    }
                }
            }
            let mut cache = self.v.layouts.borrow_mut();
            // 増えすぎないよう、一定数を超えたら捨てる
            if cache.len() > 6000 {
                cache.clear();
            }
            cache.insert(key, layout.clone());
            Some(layout)
        }
    }

    fn links_of(spans: &[Span]) -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        let mut at = 0u32;
        for s in spans {
            let n = s.text.encode_utf16().count() as u32;
            if let Some(l) = &s.link {
                out.push((at, n, l.clone()));
            }
            at += n;
        }
        out
    }

    /// 文字を置いて、高さを返す
    fn text(&mut self, spans: &[Span], st: Style, x: f32, y: f32, w: f32) -> f32 {
        let Some(layout) = self.layout(spans, st, w) else { return self.line_h() };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        let h = m.height.max(self.line_h());
        self.items.push(Item::Text { x, y, w, h, layout, tone: self.tone, links: Self::links_of(spans) });
        h
    }

    fn plain(text: &str) -> Vec<Span> {
        vec![Span { text: text.to_string(), ..Span::default() }]
    }

    fn body(&self) -> Style {
        Style { size: self.base, bold: false, mono: false, wrap: true, align: DWRITE_TEXT_ALIGNMENT_LEADING }
    }

    /// 自然な幅（折り返さないときの幅）
    fn natural_width(&self, spans: &[Span], st: Style) -> f32 {
        let st = Style { wrap: false, align: DWRITE_TEXT_ALIGNMENT_LEADING, ..st };
        let Some(layout) = self.layout(spans, st, 1.0e5) else { return 40.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        m.widthIncludingTrailingWhitespace
    }

    /// 直前に置いたコードのレイアウトに、言語に応じた色を付ける
    fn colorize(&mut self, lang: &str, text: &str) {
        let l = lang.to_ascii_lowercase();
        let ext = match l.as_str() {
            "rust" => "rs",
            "python" | "python3" => "py",
            "javascript" | "node" => "js",
            "typescript" => "ts",
            "shell" | "console" | "zsh" => "sh",
            "csharp" | "c#" => "cs",
            "c++" => "cpp",
            "powershell" => "ps1",
            "golang" => "go",
            "kotlin" => "kt",
            other => other,
        };
        self.colorize_ext(ext, text);
    }

    fn colorize_ext(&mut self, ext: &str, text: &str) {
        use kazakiri_core::syntax::{self, Kind, Lang};
        let Some(lang) = Lang::from_extension(ext) else { return };
        let Some(Item::Text { layout, .. }) = self.items.last() else { return };
        let mut base = 0u32;
        for line in text.split('\n') {
            for (r, kind) in syntax::highlight(lang, line) {
                let a = base + line[..r.start].encode_utf16().count() as u32;
                let n = line[r.start..r.end].encode_utf16().count() as u32;
                let idx = match kind {
                    Kind::Str => 2,
                    Kind::Num => 3,
                    Kind::Keyword => 4,
                    Kind::Comment => 5,
                    Kind::Tag => 6,
                    Kind::Key => 7,
                };
                if let (Some(b), true) = (self.brushes.get(idx), n > 0) {
                    unsafe {
                        let _ = layout.SetDrawingEffect(b, DWRITE_TEXT_RANGE { startPosition: a, length: n });
                    }
                }
            }
            base += line.encode_utf16().count() as u32 + 1;
        }
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Fill) {
        self.items.push(Item::Fill { x, y, w, h, fill });
    }

    /// ブロックを縦に並べて、終わりの y を返す
    fn blocks(&mut self, nodes: &[Node], x: f32, w: f32, mut y: f32) -> f32 {
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                y += self.gap;
            }
            self.marks.push((n.line, y));
            y = self.block(n, x, w, y);
        }
        y
    }

    fn block(&mut self, n: &Node, x: f32, w: f32, y: f32) -> f32 {
        let base = self.base;
        match &n.block {
            Block::Heading { level, spans } => {
                let size = base * [1.9, 1.55, 1.3, 1.12, 1.0, 0.95][(*level as usize).clamp(1, 6) - 1];
                self.anchors.push((slug(spans), y));
                let h = self.text(spans, Style { size, bold: true, ..self.body() }, x, y, w);
                if *level <= 2 {
                    self.fill(x, y + h + 2.0, w, 1.0, Fill::Line);
                    return y + h + 4.0;
                }
                y + h
            }
            Block::Para(spans) => y + self.text(spans, self.body(), x, y, w),
            Block::Code { text, .. } | Block::Html(text) | Block::Meta(text) => {
                let pad = 8.0;
                let spans = Self::plain(if text.is_empty() { " " } else { text });
                let st = Style { size: base * 0.92, mono: true, ..self.body() };
                let top = self.items.len();
                let h = self.text(&spans, st, x + pad, y + pad, w - pad * 2.0);
                if let Block::Code { lang, .. } = &n.block {
                    self.colorize(lang, text);
                }
                self.items.insert(top, Item::Fill { x, y, w, h: h + pad * 2.0, fill: Fill::Code });
                y + h + pad * 2.0
            }
            Block::Quote(children) => {
                let (save_tone, save_gap) = (self.tone, self.gap);
                self.tone = Tone::Muted;
                let end = self.blocks(children, x + 16.0, w - 16.0, y);
                self.tone = save_tone;
                self.gap = save_gap;
                self.fill(x + 2.0, y, 3.0, (end - y).max(self.line_h()), Fill::Quote);
                end.max(y + self.line_h())
            }
            Block::List { ordered, start, items } => self.list(*ordered, *start, items, x, w, y),
            Block::Rule => {
                self.fill(x, y + base * 0.5, w, 1.0, Fill::Rule);
                y + base
            }
            Block::Table { align, head, rows } => self.table(align, head, rows, x, w, y),
            Block::Image { url, alt, link } => self.image(url, alt, link.as_deref(), x, w, y),
            Block::Footnote { label, blocks } => {
                let lab = format!("[{label}]");
                let lw = self.natural_width(&Self::plain(&lab), self.body()) + 8.0;
                self.text(&Self::plain(&lab), self.body(), x, y, lw);
                let save = self.tone;
                self.tone = Tone::Muted;
                let end = self.blocks(blocks, x + lw, w - lw, y);
                self.tone = save;
                end
            }
        }
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[ListItem], x: f32, w: f32, mut y: f32) -> f32 {
        let base = self.base;
        let digits = (start + items.len() as u64).to_string().len() as f32;
        let marker_w = if ordered { base * (1.0 + 0.62 * digits) + 8.0 } else { base * 1.5 };
        let depth_mark = ["•", "◦", "▪"][(((x / (base * 1.5)) as usize) / 2) % 3];
        let lh = self.line_h();
        let save_gap = self.gap;
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                y += (base * 0.25).round();
            }
            let top = y;
            match it.checked {
                Some(c) => {
                    let size = (base * 1.0).round();
                    self.items.push(Item::Check { x: x + 2.0, y: y + (lh - size) / 2.0, size, checked: c });
                }
                None => {
                    let label = if ordered { format!("{}.", start + i as u64) } else { depth_mark.to_string() };
                    let st = Style { align: DWRITE_TEXT_ALIGNMENT_TRAILING, wrap: false, ..self.body() };
                    self.text(&Self::plain(&label), st, x, y, marker_w - 6.0);
                }
            }
            self.gap = (base * 0.25).round();
            let end = self.blocks(&it.blocks, x + marker_w, w - marker_w, y);
            self.gap = save_gap;
            y = end.max(top + lh);
        }
        y
    }

    fn table(&mut self, align: &[Align], head: &[Vec<Span>], rows: &[Vec<Vec<Span>>], x: f32, w: f32, y: f32) -> f32 {
        let cols = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return y;
        }
        let pad = 8.0;
        let hst = Style { bold: true, wrap: false, ..self.body() };
        let bst = Style { wrap: false, ..self.body() };
        // 列の幅：中身の幅（長い列は上限を決めて折り返す）
        let mut widths = vec![40.0f32; cols];
        let cell = |r: &[Vec<Span>], c: usize| r.get(c).cloned().unwrap_or_default();
        for (c, wd) in widths.iter_mut().enumerate() {
            let mut m = self.natural_width(&cell(head, c), hst);
            for r in rows {
                m = m.max(self.natural_width(&cell(r, c), bst));
            }
            *wd = m + pad * 2.0;
        }
        let total: f32 = widths.iter().sum();
        if total > w {
            // 入りきらないときは、幅の大きい列から順に縮める（各列は最低でも 60 か、中身の短いほう）
            let mut over = total - w;
            let min: Vec<f32> = widths.iter().map(|&v| v.min(60.0)).collect();
            while over > 0.5 {
                let (i, _) = widths.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
                let cut = (widths[i] - min[i]).min(over).min(over.max(8.0));
                if cut <= 0.0 {
                    break;
                }
                widths[i] -= cut;
                over -= cut;
            }
        }
        let total: f32 = widths.iter().sum();
        let mut cy = y;
        let all: Vec<(&[Vec<Span>], bool)> = std::iter::once((head, true)).chain(rows.iter().map(|r| (r.as_slice(), false))).collect();
        let mut row_bounds = vec![y];
        for (r, is_head) in all {
            if r.is_empty() && is_head {
                continue;
            }
            // 行の高さ：いちばん高いセル
            let st = if is_head { hst } else { bst };
            let mut cells = Vec::new();
            let mut rh = self.line_h();
            let mut cx = x;
            for (c, &colw) in widths.iter().enumerate() {
                let al = match align.get(c) {
                    Some(Align::Center) => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Some(Align::Right) => DWRITE_TEXT_ALIGNMENT_TRAILING,
                    _ => DWRITE_TEXT_ALIGNMENT_LEADING,
                };
                let spans = cell(r, c);
                let cst = Style { wrap: true, align: al, ..st };
                let lw = colw - pad * 2.0;
                if let Some(layout) = self.layout(&spans, cst, lw) {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    unsafe {
                        let _ = layout.GetMetrics(&mut m);
                    }
                    rh = rh.max(m.height);
                    cells.push((cx + pad, layout, lw, Self::links_of(&spans)));
                }
                cx += colw;
            }
            let row_h = rh + 8.0;
            if is_head {
                self.fill(x, cy, total, row_h, Fill::Head);
            }
            for (tx, layout, lw, links) in cells {
                self.items.push(Item::Text { x: tx, y: cy + 4.0, w: lw, h: rh, layout, tone: self.tone, links });
            }
            cy += row_h;
            row_bounds.push(cy);
        }
        // 罫線
        for &by in &row_bounds {
            self.fill(x, by, total, 1.0, Fill::Line);
        }
        let mut cx = x;
        for wd in &widths {
            self.fill(cx, y, 1.0, cy - y, Fill::Line);
            cx += wd;
        }
        self.fill(cx, y, 1.0, cy - y, Fill::Line);
        cy + 1.0
    }

    fn image(&mut self, url: &str, alt: &str, link: Option<&str>, x: f32, w: f32, y: f32) -> f32 {
        let path = resolve_path(url, self.dir.as_deref()).filter(|p| p.is_file());
        let size = path.as_ref().and_then(|p| {
            let mut sizes = self.v.sizes.borrow_mut();
            *sizes.entry(p.clone()).or_insert_with(|| image_size(p))
        });
        match (path, size) {
            (Some(path), Some((pw, ph))) => {
                let (pw, ph) = (pw as f32, ph as f32);
                let k = (w / pw).min(IMAGE_MAX_H / ph).min(1.0);
                let (dw, dh) = ((pw * k).max(1.0), (ph * k).max(1.0));
                self.items.push(Item::Image { x, y, w: dw, h: dh, path, link: link.map(String::from) });
                y + dh
            }
            _ => {
                // 読めない画像（ネットワーク上のもの・見つからないもの・対応しない形式）は、代替文字をリンクとして出す
                let label = if alt.is_empty() { url.to_string() } else { alt.to_string() };
                let target = link.unwrap_or(url).to_string();
                let spans = vec![Span { text: format!("🖼 {label}"), link: Some(target), ..Span::default() }];
                y + self.text(&spans, self.body(), x, y, w)
            }
        }
    }
}

impl App {
    /// プレビューの描画用の設定が変わったことを調べる鍵
    fn md_cache_key(&self, width: f32) -> Option<Key> {
        let doc = self.doc.as_ref()?;
        let version = self.editor.as_ref().map_or(0, |e| e.version());
        let fonts = hash_of(&(&self.settings.ui_font, self.settings.ui_font_size.to_bits(), &self.settings.font));
        Some((Arc::as_ptr(doc) as *const () as usize, version, width.to_bits(), self.settings.font_size.to_bits(), self.settings.dark, fonts))
    }

    /// プレビューを（必要なら）作り直して返す
    fn md_laid(&self, width: f32) -> Option<Rc<Laid>> {
        let v = self.md.as_ref()?;
        let key = self.md_cache_key(width)?;
        if v.key.get() == Some(key) {
            if let Some(l) = v.laid() {
                return Some(l);
            }
        }
        let doc = self.doc.as_ref()?;
        let start = doc.text_start() as u64;
        let len = doc.len().saturating_sub(start);
        if len > MAX_BYTES {
            *v.notice.borrow_mut() = Some(format!("プレビューできるのは {} MB までです", MAX_BYTES >> 20));
            v.key.set(Some(key));
            *v.laid.borrow_mut() = None;
            return None;
        }
        *v.notice.borrow_mut() = None;
        let src = doc.decode(&doc.read(start, start + len)).into_owned();
        let nodes = markdown::parse(&src);
        let g = self.gfx.as_ref()?;
        let base = self.settings.font_size.max(8.0);
        let mut lb = Lb {
            v,
            dw: &self.dw,
            ui_font: &self.settings.ui_font,
            code_font: &self.settings.font,
            base,
            brushes: &g.md,
            dir: doc.path().and_then(|p| p.parent()).map(Path::to_path_buf),
            items: Vec::new(),
            marks: Vec::new(),
            anchors: Vec::new(),
            tone: Tone::Normal,
            gap: (base * 0.7).round(),
        };
        let pad = 16.0;
        let end = lb.blocks(&nodes, pad, (width - pad * 2.0).max(40.0), pad);
        let laid = Rc::new(Laid { items: lb.items, height: end + pad, marks: lb.marks, anchors: lb.anchors });
        v.key.set(Some(key));
        *v.laid.borrow_mut() = Some(laid.clone());
        Some(laid)
    }

    /// プレビューが出ている領域の幅（分割のときの右半分。デザインビューは全幅）
    pub(super) fn md_pane_w(&self) -> f32 {
        match &self.md {
            Some(v) if v.mode == MdMode::Split => (self.client_size().0 / 2.0).floor(),
            _ => 0.0,
        }
    }

    /// エディタの本文が使える幅（分割のプレビューの分を除く）
    pub(super) fn editor_w(&self) -> f32 {
        self.client_size().0 - self.md_pane_w()
    }

    /// デザインビューのスクロールバー用：(内容の高さ, 1 ページの高さ, 位置)
    pub(super) fn md_scroll_info(&self) -> Option<(i32, u32, i32)> {
        if self.md_mode() != Some(MdMode::Design) {
            return None;
        }
        let v = self.md.as_ref()?;
        let (_, _, h) = self.md_area();
        let total = v.laid().map_or(h, |l| l.height.max(h));
        Some((total.ceil() as i32, h.max(1.0) as u32, v.scroll.get() as i32))
    }

    /// デザインビューのスクロールバーの操作
    pub(super) fn md_vscroll(&mut self, code: SCROLLBAR_COMMAND, track: i32) {
        let (_, _, h) = self.md_area();
        let step = self.line_h * 2.0;
        match code {
            SB_LINEUP => self.md_scroll_by(-step),
            SB_LINEDOWN => self.md_scroll_by(step),
            SB_PAGEUP => self.md_scroll_by(-(h - step)),
            SB_PAGEDOWN => self.md_scroll_by(h - step),
            SB_TOP => self.md_scroll_by(-1.0e9),
            SB_BOTTOM => self.md_scroll_by(1.0e9),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                if let Some(v) = &self.md {
                    v.scroll.set(track as f32);
                }
                self.invalidate();
            }
            _ => {}
        }
    }

    /// （本文の座標で）点の下にあるリンクの飛び先。あいだ、ステータスバーに出す
    pub(super) fn md_link_hover(&mut self, x: f32, y: f32) -> bool {
        let url = if self.md_hit(x, y) {
            let (left, w, _) = self.md_area();
            let (_, off) = self.md_content(w);
            self.md.as_ref().and_then(|v| v.laid()).and_then(|laid| laid.link_at(x - left - off, y + self.md_scroll(&laid)))
        } else {
            None
        };
        match url {
            Some(u) => {
                if self.message != u {
                    self.message = u;
                    self.md_hover = true;
                    self.invalidate();
                }
                true
            }
            None => {
                if self.md_hover {
                    self.md_hover = false;
                    self.message.clear();
                    self.invalidate();
                }
                false
            }
        }
    }

    pub(super) fn md_mode(&self) -> Option<MdMode> {
        self.md.as_ref().map(|v| v.mode)
    }

    /// （本文の座標で）プレビューの上か
    pub(super) fn md_hit(&self, x: f32, y: f32) -> bool {
        match self.md_mode() {
            Some(MdMode::Design) => y >= 0.0 && y < self.client_size().1 - self.status_h(),
            Some(MdMode::Split) => y >= 0.0 && y < self.text_height() + self.pin_h() && x >= self.editor_w(),
            None => false,
        }
    }

    /// 領域の外枠：(左, 幅, 高さ)。デザインビューは本文の幅を絞って中央に寄せる
    fn md_area(&self) -> (f32, f32, f32) {
        let (cw, ch) = self.client_size();
        match self.md_mode() {
            Some(MdMode::Design) => (0.0, cw, ch - self.status_h()),
            _ => {
                let pw = self.md_pane_w();
                (cw - pw, pw, self.text_height() + self.pin_h())
            }
        }
    }

    /// 内容を置く幅（デザインビューは広すぎる窓で絞る）と、左の余白
    fn md_content(&self, w: f32) -> (f32, f32) {
        if self.md_mode() == Some(MdMode::Design) && w > DESIGN_MAX_W {
            (DESIGN_MAX_W, ((w - DESIGN_MAX_W) / 2.0).floor())
        } else {
            (w, 0.0)
        }
    }

    /// いまのスクロール位置（DIP）。分割のときはエディタの先頭行に合わせる
    fn md_scroll(&self, laid: &Laid) -> f32 {
        match self.md_mode() {
            Some(MdMode::Split) => {
                // 先頭の行にだけ合わせると、下へ行くほどずれる（プレビューのほうが縦に長いため）。
                // 文書の頭では先頭、末尾では最下部がそろうよう、基準の行を画面の上から下へ動かす
                let (_, _, h) = self.md_area();
                let vis = self.visible_lines() as f32;
                let max_top = (self.total_lines() as f32 - vis).max(1.0);
                let f = (self.top as f32 / max_top).clamp(0.0, 1.0);
                let line = self.top as f32 + f * vis;
                let y0 = laid.y_for_line(line as u32);
                let y1 = laid.y_for_line(line as u32 + 1);
                (y0 + (y1 - y0) * line.fract() - f * h).max(0.0)
            }
            _ => self.md.as_ref().map_or(0.0, |v| v.scroll.get()),
        }
    }

    pub(super) fn md_toggle(&mut self, mode: MdMode) {
        if self.md_mode() == Some(mode) {
            return self.md_close();
        }
        let from_design = self.md_mode() == Some(MdMode::Design);
        if from_design {
            self.md_leave_design();
        }
        self.diffview = None;
        if self.doc.is_none() {
            return;
        }
        self.md = Some(MdView::new(mode));
        if mode == MdMode::Design {
            // 本文の位置から始める
            let width = self.md_area().1;
            let (cw, _) = self.md_content(width);
            if let Some(l) = self.md_laid(cw) {
                if let Some(v) = &self.md {
                    v.scroll.set(l.y_for_line(self.top as u32));
                }
            }
            self.message = "デザインビュー: ↑↓ PageUp/PageDown Home/End・ホイールで動かす。クリックでその位置へ、Esc で編集に戻る".into();
        } else {
            self.message = "Markdown プレビュー（分割）: 右の窓をクリックすると、その位置へ移ります".into();
        }
        self.update_scrollbars();
        self.invalidate();
    }

    pub(super) fn md_close(&mut self) {
        if self.md.is_none() {
            return;
        }
        self.md_leave_design();
        self.md = None;
        self.update_scrollbars();
        self.invalidate();
    }

    /// デザインビューを出るとき、見ていた位置へエディタの先頭を合わせる
    fn md_leave_design(&mut self) {
        if self.md_mode() != Some(MdMode::Design) {
            return;
        }
        let (_, w, _) = self.md_area();
        let (cw, _) = self.md_content(w);
        let line = self.md_laid(cw).zip(self.md.as_ref()).map(|(l, v)| l.line_for_y(v.scroll.get()));
        if let Some(line) = line {
            self.top = (line as u64).min(self.total_lines().saturating_sub(1));
            self.clamp_top();
        }
    }

    /// いまの本文を HTML にして保存する
    pub(super) fn md_export_html(&mut self, out: PathBuf) {
        let Some(doc) = self.doc.clone() else { return };
        let start = doc.text_start() as u64;
        let src = doc.decode(&doc.read(start, doc.len())).into_owned();
        let title = doc.path().and_then(|p| p.file_stem()).map_or_else(|| "document".to_string(), |s| s.to_string_lossy().into_owned());
        let html = markdown::to_html(&src, &title);
        self.message = match std::fs::write(&out, html) {
            Ok(()) => format!("HTML に書き出しました: {}", out.display()),
            Err(e) => format!("書き出せませんでした: {e}"),
        };
        self.invalidate();
    }

    // ---- スクロール・クリック・キー ----

    pub(super) fn md_scroll_by(&mut self, delta_px: f32) {
        let (_, w, h) = self.md_area();
        let (cw, _) = self.md_content(w);
        let Some(laid) = self.md_laid(cw) else { return };
        match self.md_mode() {
            Some(MdMode::Split) => {
                // 分割：エディタを動かす（プレビューは付いてくる）
                let lines = (delta_px / self.line_h).round() as i64;
                self.scroll_by(if lines == 0 { delta_px.signum() as i64 } else { lines });
            }
            Some(MdMode::Design) => {
                if let Some(v) = &self.md {
                    let max = (laid.height - h).max(0.0);
                    v.scroll.set((v.scroll.get() + delta_px).clamp(0.0, max));
                }
                self.update_scrollbars();
                self.invalidate();
            }
            None => {}
        }
    }

    pub(super) fn md_wheel(&mut self, delta: i64) {
        self.md_scroll_by(-(delta as f32) / 120.0 * self.line_h * 3.0);
    }

    /// デザインビューのキー。処理したら true
    pub(super) fn md_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.md_mode() != Some(MdMode::Design) {
            return false;
        }
        let (_, _, h) = self.md_area();
        let step = self.line_h * 2.0;
        match vk {
            VK_ESCAPE => self.md_close(),
            VK_UP => self.md_scroll_by(-step),
            VK_DOWN => self.md_scroll_by(step),
            VK_PRIOR => self.md_scroll_by(-(h - step)),
            VK_NEXT => self.md_scroll_by(h - step),
            VK_HOME => self.md_scroll_by(-1.0e9),
            VK_END => self.md_scroll_by(1.0e9),
            _ if ctrl => return false,
            _ => {}
        }
        true
    }

    /// プレビューをクリックした：リンクなら開き、そうでなければその行へ移る
    pub(super) fn md_click(&mut self, x: f32, y: f32) -> Option<Deferred> {
        let (left, w, _) = self.md_area();
        let (cw, off) = self.md_content(w);
        let laid = self.md_laid(cw)?;
        let (px, py) = (x - left - off, y + self.md_scroll(&laid));
        // リンク
        if let Some(url) = laid.link_at(px, py) {
            return self.md_open_link(&url, &laid);
        }
        let line = laid.line_for_y(py) as u64;
        let split = self.md_mode() == Some(MdMode::Split);
        if let Some(doc) = self.doc.clone() {
            if let Some((s, _)) = doc.line_range(line.min(doc.line_count().0.saturating_sub(1))) {
                self.sel = Some((s, s));
                if split {
                    self.ensure_visible(s);
                }
            }
        }
        self.invalidate();
        None
    }

    fn md_open_link(&mut self, url: &str, laid: &Laid) -> Option<Deferred> {
        if let Some(name) = url.strip_prefix('#') {
            let name = percent_decode(name).to_lowercase();
            if let Some((_, y)) = laid.anchors.iter().find(|(s, _)| *s == name) {
                match self.md_mode() {
                    Some(MdMode::Design) => {
                        if let Some(v) = &self.md {
                            v.scroll.set(*y);
                        }
                    }
                    _ => {
                        let line = laid.line_for_y(*y) as u64;
                        self.top = line;
                        self.clamp_top();
                    }
                }
                self.invalidate();
            } else {
                self.message = format!("見出しが見つかりません: #{name}");
                self.invalidate();
            }
            return None;
        }
        let dir = self.doc.as_ref().and_then(|d| d.path()).and_then(|p| p.parent()).map(Path::to_path_buf);
        // 別の Markdown ファイルはタブで開く
        if let Some(p) = resolve_path(url, dir.as_deref()) {
            let is_md = p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown"));
            if is_md && p.is_file() {
                self.open(p, None);
                return None;
            }
        }
        // それ以外は、関連づけられたアプリで開く（http など）。書式が怪しい文字は渡さない
        let target = match resolve_path(url, dir.as_deref()) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => url.to_string(),
        };
        let allowed = markdown::is_remote(&target) && !target.starts_with("data:") || target.starts_with("mailto:") || Path::new(&target).exists();
        if allowed {
            let w = wide(&target);
            unsafe {
                ShellExecuteW(Some(self.hwnd), w!("open"), PCWSTR(w.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
        } else {
            self.message = format!("開けません: {target}");
            self.invalidate();
        }
        None
    }

    // ---- 描画 ----

    /// プレビューを描く。`x0, y0` は領域の左上（いまの変換のもとで）、`w, h` はその大きさ
    pub(super) unsafe fn draw_markdown(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let (x0, w, h) = self.md_area();
        let t = self.theme;
        let dark = self.settings.dark;
        let (cw, off) = self.md_content(w);
        unsafe {
            brush.SetColor(&t.bg);
            rt.FillRectangle(&rect(x0, 0.0, x0 + w, h), brush);
            let Some(v) = &self.md else { return Ok(()) };
            let split = v.mode == MdMode::Split;
            if split {
                // エディタとの境目
                brush.SetColor(&t.dim);
                rt.FillRectangle(&rect(x0, 0.0, x0 + 1.0, h), brush);
            }
            let Some(laid) = self.md_laid(cw) else {
                let msg = v.notice.borrow().clone().unwrap_or_else(|| "プレビューできません".into());
                brush.SetColor(&t.gutter_text);
                self.draw_ui_str(rt, brush, &msg, x0 + 16.0, 16.0, x0 + w - 8.0)?;
                return Ok(());
            };
            let scroll = self.md_scroll(&laid).min((laid.height - h).max(0.0)).max(0.0);
            if !split {
                v.scroll.set(scroll);
            }
            rt.PushAxisAlignedClip(&rect(x0 + if split { 1.0 } else { 0.0 }, 0.0, x0 + w, h), D2D1_ANTIALIAS_MODE_ALIASED);
            let ox = x0 + off;
            let visible = |y: f32, ih: f32| y + ih >= scroll && y <= scroll + h;
            let muted = {
                let (a, b) = (t.text, t.bg);
                D2D1_COLOR_F { r: a.r * 0.62 + b.r * 0.38, g: a.g * 0.62 + b.g * 0.38, b: a.b * 0.62 + b.b * 0.38, a: 1.0 }
            };
            let accent = theme::accent(dark);
            for it in &laid.items {
                match it {
                    Item::Fill { x, y, w: fw, h: fh, fill } => {
                        if !visible(*y, *fh) {
                            continue;
                        }
                        let c = match fill {
                            Fill::Code => t.panel_bg,
                            Fill::Quote => accent,
                            Fill::Rule | Fill::Line => t.dim,
                            Fill::Head => t.panel_head,
                        };
                        brush.SetColor(&c);
                        rt.FillRectangle(&rect(ox + x, y - scroll, ox + x + fw, y - scroll + fh), brush);
                        if let Fill::Code = fill {
                            brush.SetColor(&t.dim);
                            rt.DrawRectangle(&rect(ox + x + 0.5, y - scroll + 0.5, ox + x + fw - 0.5, y - scroll + fh - 0.5), brush, 1.0, None);
                        }
                    }
                    Item::Text { x, y, h: th, layout, tone, .. } => {
                        if !visible(*y, *th) {
                            continue;
                        }
                        brush.SetColor(if *tone == Tone::Muted { &muted } else { &t.text });
                        rt.DrawTextLayout(Vector2 { X: ox + x, Y: y - scroll }, layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                    Item::Check { x, y, size, checked } => {
                        if !visible(*y, *size) {
                            continue;
                        }
                        let r = rect(ox + x, y - scroll, ox + x + size, y - scroll + size);
                        brush.SetColor(&t.gutter_text);
                        rt.DrawRectangle(&r, brush, 1.5, None);
                        if *checked {
                            brush.SetColor(&accent);
                            rt.FillRectangle(&rect(ox + x + 3.0, y - scroll + 3.0, ox + x + size - 3.0, y - scroll + size - 3.0), brush);
                        }
                    }
                    Item::Image { x, y, w: iw, h: ih, path, .. } => {
                        if !visible(*y, *ih) {
                            continue;
                        }
                        let bmp = {
                            let mut m = v.bitmaps.borrow_mut();
                            m.entry(path.clone()).or_insert_with(|| load_bitmap(rt, path)).clone()
                        };
                        let dest = rect(ox + x, y - scroll, ox + x + iw, y - scroll + ih);
                        match bmp {
                            Some(b) => rt.DrawBitmap(&b, Some(&dest), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                            None => {
                                brush.SetColor(&t.dim);
                                rt.DrawRectangle(&dest, brush, 1.0, None);
                            }
                        }
                    }
                }
            }
            rt.PopAxisAlignedClip();
        }
        Ok(())
    }
}

impl Laid {
    /// 点（内容の座標）にあるリンクの飛び先
    fn link_at(&self, x: f32, y: f32) -> Option<String> {
        for it in &self.items {
            match it {
                Item::Text { x: tx, y: ty, w, h, layout, links, .. } if !links.is_empty() => {
                    if x < *tx || x > tx + w || y < *ty || y > ty + h {
                        continue;
                    }
                    let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
                    let mut m = DWRITE_HIT_TEST_METRICS::default();
                    if unsafe { layout.HitTestPoint(x - tx, y - ty, &mut trailing, &mut inside, &mut m) }.is_err() || !inside.as_bool() {
                        continue;
                    }
                    if let Some((_, _, url)) = links.iter().find(|(a, n, _)| m.textPosition >= *a && m.textPosition < a + n) {
                        return Some(url.clone());
                    }
                }
                Item::Image { x: ix, y: iy, w, h, link: Some(l), .. } if x >= *ix && x <= ix + w && y >= *iy && y <= iy + h => return Some(l.clone()),
                _ => {}
            }
        }
        None
    }
}
