//! CSV / TSV の表示：列をそろえ、カーソルのある列を強調する。
//!
//! 各行を「列の幅に合わせて空白を足し、区切り文字を │ にした表示用の文字列」に変換して描く。
//! 元の文字列の位置と表示用の位置の対応表（`Disp::segs`）を持つので、
//! カーソル・選択・検索の強調などは、位置を変換するだけでそのまま使える。

use std::cell::{Cell, RefCell};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kazakiri_core::csv::{self, SortOptions};
use kazakiri_core::{EditKind, Line, SearchControl};

use super::*;

/// 列の幅の上限（半角の文字数）。これより長いフィールドはその行だけ列がずれる
const MAX_COL_WIDTH: usize = 48;
/// 並べ替えの完了通知
pub(super) const WM_SORT_DONE: u32 = WM_APP + 6;
/// 引用符の中の改行（前の行から続く行）の一覧ができた
pub(super) const WM_CONT_DONE: u32 = WM_APP + 20;

/// 前の行から続くレコードの行番号の一覧（別スレッドで求める）
pub(super) struct CsvCont {
    /// 求めた対象（文書の識別と区切り文字）
    pub key: (usize, char),
    pub cell: Arc<std::sync::OnceLock<Vec<u64>>>,
}

pub(super) struct CsvView {
    pub delim: char,
    /// 1 行目を見出しとして扱う（並べ替えで先頭に残す、列名を表示する）
    pub header: bool,
    /// 各列の幅（半角の文字数）。見えた行の中で最大のものに広がっていく
    widths: RefCell<Vec<usize>>,
    /// 幅が変わったら増える（描画済みのレイアウトを作り直す目印）
    pub generation: Cell<u64>,
    /// 並べ替え・列フィルタの対象の列（0 始まり）。None ならカーソルのある列
    pub target: Option<usize>,
}

impl CsvView {
    pub fn new(delim: char) -> CsvView {
        CsvView { delim, header: true, widths: RefCell::new(Vec::new()), generation: Cell::new(0), target: None }
    }

    /// 行を見て列の幅を広げる。広がったら true
    pub fn measure(&self, text: &str) -> bool {
        let mut widths = self.widths.borrow_mut();
        let mut changed = false;
        for (k, r) in csv::split_fields(text, self.delim).into_iter().enumerate() {
            let w = csv::display_width(&text[r]).min(MAX_COL_WIDTH);
            if k >= widths.len() {
                widths.push(w);
                changed = true;
            } else if w > widths[k] {
                widths[k] = w;
                changed = true;
            }
        }
        if changed {
            self.generation.set(self.generation.get() + 1);
        }
        changed
    }
}

/// 描画用の 1 行
pub(super) struct Disp {
    /// 表示する UTF-16 列（省略記号を含む）
    pub units: Vec<u16>,
    /// そのうち本文の長さ
    pub shown: usize,
    /// CSV のとき：(元の位置, 表示の位置, 長さ) の区間。None なら表示＝元の文字列
    segs: Option<Vec<(u32, u32, u32)>>,
    /// CSV のとき：各列の表示範囲 [開始, 終了)
    pub cols: Vec<(u32, u32)>,
    /// 色を変える範囲（表示の位置）
    pub effects: Vec<(u32, u32, Effect)>,
}

/// 文字の色の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Effect {
    /// 空白・タブ・改行の印
    Mark,
    LogError,
    LogWarn,
    LogDebug,
    /// 構文の色分け
    Syn(kazakiri_core::syntax::Kind),
    /// 固定した見出し行の、対象の列（アクセントカラー）
    Target,
}

/// ログレベルの語（単語として現れたもの）を探す。戻り値は UTF-16 の位置
fn log_levels(text: &str) -> Vec<(u32, u32, Effect)> {
    const WORDS: [(&str, Effect); 9] = [
        ("FATAL", Effect::LogError),
        ("CRITICAL", Effect::LogError),
        ("ERROR", Effect::LogError),
        ("SEVERE", Effect::LogError),
        ("WARNING", Effect::LogWarn),
        ("WARN", Effect::LogWarn),
        ("DEBUG", Effect::LogDebug),
        ("TRACE", Effect::LogDebug),
        ("VERBOSE", Effect::LogDebug),
    ];
    let head: &str = match text.char_indices().nth(512) {
        Some((i, _)) => &text[..i],
        None => text,
    };
    let mut out = Vec::new();
    for (w, eff) in WORDS {
        let mut from = 0;
        while let Some(i) = head[from..].find(w) {
            let a = from + i;
            let b = a + w.len();
            let before = head[..a].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = head[b..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            if before && after && !out.iter().any(|&(s, e, _): &(usize, usize, Effect)| a < e && s < b) {
                out.push((a, b, eff));
            }
            from = b;
        }
    }
    out.into_iter()
        .map(|(a, b, e)| (head[..a].encode_utf16().count() as u32, head[..b].encode_utf16().count() as u32, e))
        .collect()
}

impl Disp {
    pub fn plain(line: &Line) -> Disp {
        let (units, shown) = render_units(line);
        Disp { units, shown, segs: None, cols: Vec::new(), effects: Vec::new() }
    }

    /// 空白・タブ・改行を見える印にした行。タブは「>」と空白で次のタブ位置まで埋める
    pub fn whitespace(line: &Line, tab_width: usize, eol: Option<&str>) -> Disp {
        let src: Vec<u16> = line.text.encode_utf16().take(MAX_RENDER_UNITS).collect();
        let cut = line.truncated || src.len() == MAX_RENDER_UNITS;
        let text = String::from_utf16_lossy(&src);
        let mut units = Vec::with_capacity(src.len() + 16);
        let mut segs: Vec<(u32, u32, u32)> = Vec::new();
        let mut effects = Vec::new();
        let (mut col, mut src_pos) = (0usize, 0u32);
        // 1 対 1 で写る区間をまとめる
        let mut run: Option<(u32, u32, u32)> = None;
        let flush = |run: &mut Option<(u32, u32, u32)>, segs: &mut Vec<(u32, u32, u32)>| {
            if let Some(r) = run.take() {
                segs.push(r);
            }
        };
        for c in text.chars() {
            let n = c.len_utf16() as u32;
            let d = units.len() as u32;
            match c {
                '\t' => {
                    flush(&mut run, &mut segs);
                    let w = tab_width - col % tab_width;
                    segs.push((src_pos, d, 1));
                    units.push(b'>' as u16);
                    units.extend(std::iter::repeat_n(b' ' as u16, w - 1));
                    effects.push((d, d + 1, Effect::Mark));
                    col += w;
                }
                _ => {
                    let (shown, w, mark) = match c {
                        ' ' => ('\u{FF65}', 1, true), // 半角の中黒
                        '\u{3000}' => ('\u{25A1}', 2, true), // 全角空白は □
                        _ => (c, kazakiri_core::csv::char_width(c), false),
                    };
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(shown.encode_utf16(&mut buf));
                    if mark {
                        effects.push((d, d + n, Effect::Mark));
                    }
                    match &mut run {
                        Some(r) if r.0 + r.2 == src_pos && r.1 + r.2 == d => r.2 += n,
                        _ => {
                            flush(&mut run, &mut segs);
                            run = Some((src_pos, d, n));
                        }
                    }
                    col += w;
                }
            }
            src_pos += n;
        }
        flush(&mut run, &mut segs);
        if segs.is_empty() || segs.last().map(|s| s.0 + s.2) != Some(src_pos) {
            segs.push((src_pos, units.len() as u32, 0));
        }
        let shown = units.len();
        if let Some(mark) = eol {
            let d = units.len() as u32;
            units.extend(mark.encode_utf16());
            effects.push((d, units.len() as u32, Effect::Mark));
        }
        if cut {
            units.extend("  …(省略)".encode_utf16());
        }
        Disp { units, shown, segs: Some(segs), cols: Vec::new(), effects }
    }

    /// ログレベルの語に色を付ける
    /// 構文の色分け（行ごと。複数行にわたる文字列やコメントは追わない）
    pub fn with_syntax(mut self, line: &Line, lang: kazakiri_core::syntax::Lang) -> Disp {
        for (r, kind) in kazakiri_core::syntax::highlight(lang, &line.text) {
            let a = line.text[..r.start].encode_utf16().count();
            let b = a + line.text[r.clone()].encode_utf16().count();
            let (da, db) = (self.to_disp(a) as u32, self.to_disp(b) as u32);
            if db > da {
                self.effects.push((da, db, Effect::Syn(kind)));
            }
        }
        self
    }

    pub fn with_log_colors(mut self, line: &Line) -> Disp {
        for (a, b, e) in log_levels(&line.text) {
            let (da, db) = (self.to_disp(a as usize) as u32, self.to_disp(b as usize) as u32);
            if db > da {
                self.effects.push((da, db, e));
            }
        }
        self
    }

    /// CSV の行：列をそろえた文字列を作る
    pub fn csv(line: &Line, view: &CsvView) -> Disp {
        let src: Vec<u16> = line.text.encode_utf16().take(MAX_RENDER_UNITS).collect();
        let cut = src.len() < line.text.encode_utf16().count() || line.truncated;
        let text = String::from_utf16_lossy(&src);
        let widths = view.widths.borrow();
        let fields = csv::split_fields(&text, view.delim);
        let mut units = Vec::with_capacity(src.len() + fields.len() * 4);
        let mut segs = Vec::with_capacity(fields.len() * 2);
        let mut cols = Vec::with_capacity(fields.len());
        let mut effects = Vec::new();
        let mut src_pos = 0u32;
        let n = fields.len();
        for (k, r) in fields.into_iter().enumerate() {
            let f = &text[r];
            let flen = f.encode_utf16().count() as u32;
            let disp_start = units.len() as u32;
            segs.push((src_pos, disp_start, flen));
            // フィールドの中のタブは、列の幅の計算（半角 1 文字分）に合わせて空白 1 つで見せる（タブ位置まで広がって列がずれるのを防ぐ）
            units.extend(f.encode_utf16().map(|u| if u == 0x09 { 0x20 } else { u }));
            src_pos += flen;
            if k + 1 < n {
                let pad = widths.get(k).copied().unwrap_or(0).saturating_sub(csv::display_width(f)) + 1;
                units.extend(std::iter::repeat_n(b' ' as u16, pad));
                cols.push((disp_start, units.len() as u32));
                // 区切り文字（元は 1 文字）を │ で表示する
                segs.push((src_pos, units.len() as u32, 1));
                // 区切りの線は、控えめな色（空白の印と同じ）にする
                effects.push((units.len() as u32, units.len() as u32 + 1, Effect::Mark));
                units.push(0x2502);
                units.push(b' ' as u16);
                src_pos += 1;
            } else {
                cols.push((disp_start, units.len() as u32));
            }
        }
        let shown = units.len();
        if cut {
            units.extend("  …(省略)".encode_utf16());
        }
        Disp { units, shown, segs: Some(segs), cols, effects }
    }

    /// 元の文字列の位置（UTF-16）→ 表示の位置
    pub fn to_disp(&self, src: usize) -> usize {
        let Some(segs) = &self.segs else { return src.min(self.shown) };
        let i = segs.partition_point(|s| s.0 as usize <= src).saturating_sub(1);
        match segs.get(i) {
            Some(&(s, d, len)) => d as usize + (src - s as usize).min(len as usize),
            None => 0,
        }
    }

    /// 表示の位置 → 元の文字列の位置（空白の部分はそのフィールドの末尾）
    pub fn to_src(&self, disp: usize) -> usize {
        let Some(segs) = &self.segs else { return disp };
        let i = segs.partition_point(|s| s.1 as usize <= disp).saturating_sub(1);
        match segs.get(i) {
            Some(&(s, d, len)) => s as usize + (disp - d as usize).min(len as usize),
            None => 0,
        }
    }

    /// 表示の位置がどの列か
    pub fn col_at(&self, disp: usize) -> Option<usize> {
        if self.cols.is_empty() {
            return None;
        }
        Some(self.cols.partition_point(|c| (c.0 as usize) <= disp).saturating_sub(1))
    }
}

pub(super) struct SortDone {
    id: u64,
    result: std::result::Result<Vec<u8>, String>,
    elapsed: Duration,
    desc: bool,
    col: usize,
}

impl App {
    /// 表示用の 1 行（CSV モードなら列をそろえる）
    pub(super) fn display(&self, line: &Line) -> Disp {
        let d = match &self.csv {
            // 前の行から続くレコードの行は、列に分けず、薄い文字でそのまま見せる
            Some(_) if self.is_continuation(line.number) => {
                let mut d = Disp::plain(line);
                if d.shown > 0 {
                    d.effects.push((0, d.shown as u32, Effect::Mark));
                }
                d
            }
            Some(v) => Disp::csv(line, v),
            None if self.settings.whitespace => Disp::whitespace(line, self.settings.tab_width, self.eol_mark(line)),
            None => Disp::plain(line),
        };
        // CSV 表示でなく、ファイルの種類が分かるとき（拡張子）は、構文の色分け。そうでなければログレベルの色分け
        if self.csv.is_none() && self.settings.syntax {
            if let Some(lang) = self.syntax_lang() {
                return d.with_syntax(line, lang);
            }
        }
        if self.settings.log_colors { d.with_log_colors(line) } else { d }
    }

    /// 開いているファイルの拡張子から決めた言語
    pub(super) fn syntax_lang(&self) -> Option<kazakiri_core::syntax::Lang> {
        let ext = self.doc.as_ref()?.path()?.extension()?.to_str()?.to_string();
        kazakiri_core::syntax::Lang::from_extension(&ext)
    }

    /// 行末の印：CRLF は ¶、LF は ↓、CR は ←、ファイルの終わりは [EOF]
    fn eol_mark(&self, line: &Line) -> Option<&'static str> {
        let doc = self.doc.as_ref()?;
        let n = newline_len(doc, line);
        let unit = if doc.encoding().is_utf16() { 2 } else { 1 };
        Some(match n {
            0 => "[EOF]",
            _ if doc.newline() == kazakiri_core::Newline::Cr => "←",
            _ if n >= 2 * unit => "¶",
            _ => "↓",
        })
    }

    /// ファイル名から CSV / TSV かを決め、区切り文字を推定する
    pub(super) fn auto_csv(&mut self) {
        let Some(doc) = self.doc.clone() else { return };
        let ext = doc.path().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase());
        self.csv = match ext.as_deref() {
            Some("tsv") | Some("tab") => Some(CsvView::new('\t')),
            Some("csv") => Some(CsvView::new(self.guess_delim())),
            _ => None,
        };
    }

    fn guess_delim(&self) -> char {
        let Some(doc) = &self.doc else { return ',' };
        let lines: Vec<String> = doc.lines(0, 20).into_iter().map(|l| l.text).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        csv::detect_delimiter(&refs)
    }

    pub(super) fn toggle_csv(&mut self) {
        self.csv = match self.csv {
            Some(_) => None,
            None => Some(CsvView::new(self.guess_delim())),
        };
        self.message = match &self.csv {
            Some(v) => format!("CSV/TSV モード（区切り: {}）", delim_name(v.delim)),
            None => "CSV/TSV モードを終了しました".into(),
        };
        self.after_view_change();
    }

    pub(super) fn set_delim(&mut self, d: char) {
        if d != '\t' {
            self.last_csv_delim = d;
        }
        let header = self.csv.as_ref().is_none_or(|v| v.header);
        let mut v = CsvView::new(d);
        v.header = header;
        self.csv = Some(v);
        self.message = format!("区切り: {}", delim_name(d));
        self.after_view_change();
    }

    pub(super) fn toggle_header(&mut self) {
        if let Some(v) = &mut self.csv {
            v.header = !v.header;
            self.message = format!("1 行目を見出しとして扱う: {}", on_off(v.header));
        }
        self.clamp_top();
        self.update_scrollbars();
        self.invalidate();
    }

    /// 外部ソート：`out` に、並べ替えた結果を書き出す（終わったら開く）
    pub(super) fn start_external_sort(&mut self, out: PathBuf, desc: bool) {
        let (Some((delim, header)), Some(doc)) = (self.csv.as_ref().map(|v| (v.delim, v.header)), self.doc.clone()) else { return };
        let Some((col, _, name)) = self.target_column() else { return };
        if doc.path().is_some_and(|p| p == out.as_path()) {
            self.message = "元のファイルとは別の名前で保存してください".into();
            return self.invalidate();
        }
        let opts = SortOptions { column: col, delim, descending: desc, header };
        self.cancel_jobs();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        let done = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let label = format!("「{name}」で並べ替えて保存中");
        self.rjob = Some(results::ResultsJob { id, started: Instant::now(), what: results::JobKind::FileOp { ctl: ctl.clone(), done: done.clone(), total: doc.len() * 2, label } });
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = csv::sort_to_file(&doc, opts, &out, csv::RUN_ROWS, &done, &ctl);
            drop(doc);
            let result = r.map(|()| format!("並べ替えて保存しました（{:.1} 秒）→ {}", t.elapsed().as_secs_f64(), out.display()));
            let open = result.is_ok().then_some(out);
            let msg = Box::into_raw(Box::new(results::JobDone::FileOp { id, result, open }));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), results::WM_RESULTS_DONE, WPARAM(0), LPARAM(msg as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(msg) });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    fn after_view_change(&mut self) {
        self.layout_cache.borrow_mut().clear();
        self.x_off = 0.0;
        self.clamp_top();
        self.update_scrollbars();
        self.invalidate();
    }

    /// 固定した見出し行（1 行目）を、本文の上に描く
    pub(super) unsafe fn draw_pinned_header(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let Some(doc) = &self.doc else { return Ok(()) };
        let Some(line) = doc.lines(0, 1).into_iter().next() else { return Ok(()) };
        let gutter = self.gutter_width();
        unsafe {
            brush.SetColor(&self.theme.header_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, self.line_h), brush);
            rt.FillRectangle(&rect(0.0, 0.0, gutter, self.line_h), brush);
            brush.SetColor(&self.theme.gutter_text);
            // ほかの行番号と同じ桁数に右揃えする
            let digits = group(self.total_lines()).len().max(4);
            self.draw_str(rt, brush, &format!("{:>width$}", 1, width = digits), self.char_w * 0.5, 0.0, gutter)?;
            // 本文の下との境目
            brush.SetColor(&theme::accent(self.settings.dark));
            rt.FillRectangle(&rect(gutter, self.line_h - 1.0, w, self.line_h), brush);
            rt.PushAxisAlignedClip(&rect(gutter, 0.0, w, self.line_h), D2D1_ANTIALIAS_MODE_ALIASED);
            let disp = self.display(&line);
            let layout = self.dw.CreateTextLayout(&disp.units, &self.format, 1.0e6, self.line_h)?;
            // 並べ替え・列フィルタの対象の列は、見出しをアクセントカラーで
            if let (Some((col, _, _)), Some(g)) = (self.target_column(), self.gfx.as_ref()) {
                if let Some(&(a, b)) = disp.cols.get(col) {
                    let _ = layout.SetDrawingEffect(g.brush_for(Effect::Target), DWRITE_TEXT_RANGE { startPosition: a, length: b.saturating_sub(a) });
                }
            }
            let entry = CachedLine { end: line.end, generation: 0, layout, disp };
            self.draw_line(rt, brush, &line, &entry, self.text_left(), 0.0)?;
            rt.PopAxisAlignedClip();
        }
        Ok(())
    }

    /// 求めた「前の行から続く行」の一覧（まだ求めていなければ None）
    pub(super) fn continuation_list(&self) -> Option<&Vec<u64>> {
        let c = self.csv_cont.as_ref()?;
        let v = self.csv.as_ref()?;
        let doc = self.doc.as_ref()?;
        (c.key == (Arc::as_ptr(doc) as *const () as usize, v.delim)).then(|| c.cell.get()).flatten()
    }

    pub(super) fn is_continuation(&self, line: u64) -> bool {
        self.continuation_list().is_some_and(|l| l.binary_search(&line).is_ok())
    }

    /// 前の行から続く行の一覧を、別スレッドで求める（編集していないファイルだけ。編集すると行番号がずれるため）
    pub(super) fn ensure_continuation(&mut self) {
        let (Some(v), Some(doc)) = (&self.csv, &self.doc) else {
            self.csv_cont = None;
            return;
        };
        if self.is_modified() || !doc.progress().done {
            return;
        }
        let key = (Arc::as_ptr(doc) as *const () as usize, v.delim);
        if self.csv_cont.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let cell = Arc::new(std::sync::OnceLock::new());
        self.csv_cont = Some(CsvCont { key, cell: cell.clone() });
        let (doc, delim, hwnd) = (doc.clone(), v.delim, self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let list = csv::continuation_lines(&doc, delim, &SearchControl::default());
            drop(doc);
            let found = !list.is_empty();
            let _ = cell.set(list);
            // 続きの行がなければ、表示は変わらないので描き直さない
            if found {
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_CONT_DONE, WPARAM(0), LPARAM(0));
                }
            }
        });
    }

    /// 描く前に、見える行で列の幅を決める
    pub(super) fn csv_measure(&self, lines: &[Line]) {
        if let Some(v) = &self.csv {
            let mut changed = false;
            // 固定した見出し行は、見える行に入っていなくても、その幅も数える（見出しのほうが長いと、見出しだけ列がずれるため）
            if v.header {
                if let Some(h) = self.doc.as_ref().and_then(|d| d.lines(0, 1).into_iter().next()) {
                    changed |= v.measure(&h.text);
                }
            }
            for l in lines {
                // 続きの行（列に分けない行）は、列の幅に数えない
                if !self.is_continuation(l.number) {
                    changed |= v.measure(&l.text);
                }
            }
            if changed {
                self.layout_cache.borrow_mut().clear();
            }
        }
    }

    /// カーソルのある列（0 始まり）と、その列の見出し
    pub(super) fn caret_column(&self) -> Option<(usize, usize, String)> {
        let v = self.csv.as_ref()?;
        let doc = self.doc.as_ref()?;
        let c = self.caret();
        let line = doc.line_of_offset(c)?;
        let l = doc.lines(line, 1).into_iter().next()?;
        let d = Disp::csv(&l, v);
        let src = doc.decode(&doc.read(l.start, c.min(l.end))).encode_utf16().count();
        let col = d.col_at(d.to_disp(src))?;
        let total = d.cols.len();
        let name = if v.header {
            doc.lines(0, 1).into_iter().next().and_then(|h| {
                csv::split_fields(&h.text, v.delim).get(col).map(|r| csv::unquote(&h.text[r.clone()]))
            })
        } else {
            None
        };
        Some((col, total, name.unwrap_or_default()))
    }

    /// 固定した見出し行を押した：その列を、並べ替え・列フィルタの対象にする（カーソルを見出し行のその列へ移す）
    pub(super) fn header_click(&mut self, x: f32) {
        let Some(doc) = self.doc.clone() else { return };
        if self.csv.is_none() {
            return;
        }
        let Some(off) = self.offset_in_line_at_x(&doc, 0, (x - self.text_left()).max(0.0)) else { return };
        self.sel = Some((off, off));
        self.goal_x = None;
        if let Some((col, _, name)) = self.caret_column() {
            if let Some(v) = &mut self.csv {
                v.target = Some(col);
            }
            self.message = format!("対象の列: {} 列目{}（「列で昇順／降順」「列フィルタ」はこの列に対して行います）", col + 1, if name.is_empty() { String::new() } else { format!("「{name}」") });
        }
        self.invalidate();
    }

    /// 並べ替え・列フィルタの対象の列：(列, 列数, 見出しの名前)。明示的に選んでいればその列、なければカーソルのある列
    pub(super) fn target_column(&self) -> Option<(usize, usize, String)> {
        let v = self.csv.as_ref()?;
        let Some(col) = v.target else { return self.caret_column() };
        let names = self.csv_column_names(usize::MAX);
        let doc = self.doc.as_ref()?;
        let total = doc.lines(0, 1).first().map_or(0, |l| csv::split_fields(&l.text, v.delim).len());
        Some((col, total.max(col + 1), names.get(col).cloned().unwrap_or_default()))
    }

    /// 対象の列を選ぶメニュー（使い終わったら呼び出し側が DestroyMenu する）
    pub(super) unsafe fn build_col_menu(&self) -> Option<HMENU> {
        let v = self.csv.as_ref()?;
        let (cur, total, _) = self.target_column()?;
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let w = wide("カーソルのある列に合わせる(&C)");
            let _ = AppendMenuW(m, MF_STRING | if v.target.is_none() { MF_CHECKED } else { MF_UNCHECKED }, menu::ID_COL_FOLLOW as usize, PCWSTR(w.as_ptr()));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            let names = self.csv_column_names(usize::MAX);
            for i in 0..total.min(200) {
                let label = match names.get(i) {
                    Some(n) if !n.is_empty() => format!("{}: {}", i + 1, n.replace('&', "&&")),
                    _ => format!("{} 列目", i + 1),
                };
                let w = wide(&label);
                let checked = v.target == Some(i) || (v.target.is_none() && i == cur);
                let _ = AppendMenuW(m, MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED }, (menu::ID_COL_BASE + i as u32) as usize, PCWSTR(w.as_ptr()));
            }
            Some(m)
        }
    }

    /// 見出し行の各列の名前（最大 `max` 個）
    pub(super) fn csv_column_names(&self, max: usize) -> Vec<String> {
        let (Some(v), Some(doc)) = (self.csv.as_ref(), self.doc.as_ref()) else { return Vec::new() };
        if !v.header {
            return Vec::new();
        }
        let Some(h) = doc.lines(0, 1).into_iter().next() else { return Vec::new() };
        csv::split_fields(&h.text, v.delim).into_iter().take(max).map(|r| csv::unquote(&h.text[r])).collect()
    }

    /// カーソルのある列で並べ替える（別スレッドで並べ、1 回の編集として置き換える）
    pub(super) fn sort_by_column(&mut self, desc: bool) {
        let (Some((delim, header)), Some(doc)) = (self.csv.as_ref().map(|v| (v.delim, v.header)), self.doc.clone()) else {
            self.message = "並べ替えは CSV/TSV モードで使えます（表示メニュー）".into();
            return self.invalidate();
        };
        if !self.check_editable() {
            return;
        }
        // 並べ替え中に押されても、最初からやり直さない（クリックのたびに止めて始め直すと、いつまでも終わらない）
        if matches!(self.rjob.as_ref().map(|j| &j.what), Some(results::JobKind::Sort { .. })) {
            self.message = "並べ替え中です。終わるまでお待ちください（Esc で中止）".into();
            return self.invalidate();
        }
        let Some((col, _, name)) = self.target_column() else { return };
        let opts = SortOptions { column: col, delim, descending: desc, header };
        // 行が多すぎてメモリに載らないときは、並べ替えた結果を別のファイルへ書き出す（外部ソート）
        if doc.line_count().0 > csv::SORT_LIMIT {
            self.message = "行が多いので、並べ替えた結果を新しいファイルに保存します".into();
            self.want_sort = Some(desc);
            return self.invalidate();
        }
        self.cancel_jobs();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        self.rjob = Some(results::ResultsJob {
            id,
            started: Instant::now(),
            what: results::JobKind::Sort { ctl: ctl.clone(), total: doc.len(), name },
        });
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let t = Instant::now();
            let result = csv::sort_document(&doc, opts, &ctl);
            drop(doc);
            let done = Box::into_raw(Box::new(SortDone { id, result, elapsed: t.elapsed(), desc, col }));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_SORT_DONE, WPARAM(0), LPARAM(done as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(done) });
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    pub(super) fn on_sort_done(&mut self, done: SortDone) {
        if self.rjob.as_ref().map(|j| j.id) != Some(done.id) {
            return;
        }
        self.rjob = None;
        match done.result {
            Err(e) => self.message = e,
            Ok(bytes) => {
                let Some(doc) = self.doc.clone() else { return };
                let (a, b) = (doc.text_start() as u64, doc.len());
                drop(doc);
                let sel = self.sel.unwrap_or((a, a));
                let t = Instant::now();
                match self.with_editor(|ed| ed.replace_bytes(a, b, bytes, EditKind::Other, sel)) {
                    Some(Ok(_)) => {
                        self.sel = Some((a, a));
                        self.after_edit();
                        self.scroll_to(0);
                        self.message = format!(
                            "{} 列目で{}に並べ替えました（{:.2} 秒）。Ctrl+Z で元に戻せます",
                            done.col + 1,
                            if done.desc { "降順" } else { "昇順" },
                            done.elapsed.as_secs_f64() + t.elapsed().as_secs_f64()
                        );
                    }
                    Some(Err(e)) => self.message = e.to_string(),
                    None => {}
                }
            }
        }
        self.invalidate();
    }
}

pub(super) fn delim_name(d: char) -> &'static str {
    match d {
        '\t' => "タブ",
        ',' => "カンマ",
        ';' => "セミコロン",
        '|' => "縦棒",
        _ => "指定した記号",
    }
}

/// 並べ替えの進み具合（ステータスバー用）
pub(super) fn sort_status(ctl: &SearchControl, total: u64, name: &str, secs: f64) -> String {
    let pct = ctl.scanned.load(Ordering::Relaxed) as f64 * 100.0 / total.max(1) as f64;
    format!("並べ替え中「{name}」 {:.0}%  {secs:.1} 秒  (Esc で中止)", pct.min(100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line { number: 0, start: 0, end: text.len() as u64, text: text.into(), truncated: false, skipped: 0 }
    }

    #[test]
    fn log_words() {
        let r = log_levels("2026 [ERROR] x WARN y ERRORS DEBUG");
        let kinds: Vec<_> = r.iter().map(|x| x.2).collect();
        assert_eq!(kinds, [Effect::LogError, Effect::LogWarn, Effect::LogDebug]);
        assert_eq!((r[0].0, r[0].1), (6, 11));
    }

    #[test]
    fn whitespace_mapping() {
        // "a\tb c　d" → "a>  b･c□d"（タブ幅 4）
        let d = Disp::whitespace(&line("a\tb c\u{3000}d"), 4, Some("↓"));
        let shown = String::from_utf16_lossy(&d.units[..d.shown]);
        assert_eq!(shown, "a>  b\u{FF65}c\u{25A1}d");
        // 元の位置 → 表示の位置：b は 2 → 4
        assert_eq!(d.to_disp(2), 4);
        assert_eq!(d.to_src(4), 2);
        // タブの途中の空白は、タブの後ろ（b の前）に写る
        assert_eq!(d.to_src(3), 2);
        assert_eq!(d.to_disp(7), d.shown);
        assert_eq!(String::from_utf16_lossy(&d.units[d.shown..]), "↓");
    }
}
