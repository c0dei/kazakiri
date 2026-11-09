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

pub(super) struct CsvView {
    pub delim: char,
    /// 1 行目を見出しとして扱う（並べ替えで先頭に残す、列名を表示する）
    pub header: bool,
    /// 各列の幅（半角の文字数）。見えた行の中で最大のものに広がっていく
    widths: RefCell<Vec<usize>>,
    /// 幅が変わったら増える（描画済みのレイアウトを作り直す目印）
    pub generation: Cell<u64>,
}

impl CsvView {
    pub fn new(delim: char) -> CsvView {
        CsvView { delim, header: true, widths: RefCell::new(Vec::new()), generation: Cell::new(0) }
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
}

impl Disp {
    pub fn plain(line: &Line) -> Disp {
        let (units, shown) = render_units(line);
        Disp { units, shown, segs: None, cols: Vec::new() }
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
        let mut src_pos = 0u32;
        let n = fields.len();
        for (k, r) in fields.into_iter().enumerate() {
            let f = &text[r];
            let flen = f.encode_utf16().count() as u32;
            let disp_start = units.len() as u32;
            segs.push((src_pos, disp_start, flen));
            units.extend(f.encode_utf16());
            src_pos += flen;
            if k + 1 < n {
                let pad = widths.get(k).copied().unwrap_or(0).saturating_sub(csv::display_width(f)) + 1;
                units.extend(std::iter::repeat_n(b' ' as u16, pad));
                cols.push((disp_start, units.len() as u32));
                // 区切り文字（元は 1 文字）を │ で表示する
                segs.push((src_pos, units.len() as u32, 1));
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
        Disp { units, shown, segs: Some(segs), cols }
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
        match &self.csv {
            Some(v) => Disp::csv(line, v),
            None => Disp::plain(line),
        }
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
        self.invalidate();
    }

    fn after_view_change(&mut self) {
        self.layout_cache.borrow_mut().clear();
        self.x_off = 0.0;
        self.update_scrollbars();
        self.invalidate();
    }

    /// 描く前に、見える行で列の幅を決める
    pub(super) fn csv_measure(&self, lines: &[Line]) {
        if let Some(v) = &self.csv {
            let mut changed = false;
            for l in lines {
                changed |= v.measure(&l.text);
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

    /// カーソルのある列で並べ替える（別スレッドで並べ、1 回の編集として置き換える）
    pub(super) fn sort_by_column(&mut self, desc: bool) {
        let (Some((delim, header)), Some(doc)) = (self.csv.as_ref().map(|v| (v.delim, v.header)), self.doc.clone()) else {
            self.message = "並べ替えは CSV/TSV モードで使えます（表示メニュー）".into();
            return self.invalidate();
        };
        if !self.check_editable() {
            return;
        }
        let Some((col, _, name)) = self.caret_column() else { return };
        let opts = SortOptions { column: col, delim, descending: desc, header };
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
        _ => "その他",
    }
}

/// 並べ替えの進み具合（ステータスバー用）
pub(super) fn sort_status(ctl: &SearchControl, total: u64, name: &str, secs: f64) -> String {
    let pct = ctl.scanned.load(Ordering::Relaxed) as f64 * 100.0 / total.max(1) as f64;
    format!("並べ替え中「{name}」 {:.0}%  {secs:.1} 秒  (Esc で中止)", pct.min(100.0))
}
