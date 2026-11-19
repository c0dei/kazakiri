//! ログの時刻での操作（検索メニュー）：時刻の範囲で絞り込んで一覧に出す、その範囲を新しいタブに抽出する、時刻へ移動する。
//! 行の先頭付近の `2026-11-18 12:34:56` を時刻として読む（kazakiri_core::logtime）。時刻のない行（スタックトレースなど）は、
//! 直前の時刻のある行の続きとして、一緒に出す。一覧の 1 件は「範囲に入る連続した行のまとまり」。
//! 時刻へ移動は、時刻順に並んだログとして二分探索するので、数GB でも一瞬。

use kazakiri_core::loglevel::{self, LevelSpec};
use kazakiri_core::FindAll;
use kazakiri_core::logtime::{self, TimeRange};

use super::results::{JobKind, ResultKind};
use super::*;

/// 新しいタブに出せる大きさの上限（バイト）
const EXTRACT_LIMIT: u64 = 256 << 20;

impl App {
    /// 入力欄に出す初期値：前回の入力
    pub(super) fn time_prompt_default(&self) -> String {
        self.last_time.clone()
    }

    /// 時刻の範囲で絞り込む。`extract` なら、結果を新しいタブに出す
    pub(super) fn start_time_filter(&mut self, spec: &str, extract: bool) {
        if self.doc.is_none() {
            return;
        }
        let range = match TimeRange::parse(spec) {
            Ok(r) => r,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        self.last_time = spec.trim().to_string();
        let label = format!("時刻 {}", self.last_time);
        self.start_record_filter(label, extract, move |doc, ctl| logtime::filter_time(doc, range, 1_000_000, ctl));
    }

    /// ログレベルで絞り込む。`extract` なら、結果を新しいタブに出す
    pub(super) fn start_level_filter(&mut self, spec: &str, extract: bool) {
        if self.doc.is_none() {
            return;
        }
        let level = match LevelSpec::parse(spec) {
            Ok(l) => l,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        self.last_level = spec.trim().to_string();
        let label = format!("レベル {}", self.last_level);
        self.start_record_filter(label, extract, move |doc, ctl| loglevel::filter_level(doc, level, 1_000_000, ctl));
    }

    /// 時刻・レベルの絞り込みを、別のスレッドで走らせて、結果を一覧に出す
    fn start_record_filter(&mut self, label: String, extract: bool, job: impl FnOnce(&Document, &SearchControl) -> std::result::Result<(FindAll, u64), String> + Send + 'static) {
        let Some(doc) = self.doc.clone() else { return };
        self.cancel_jobs();
        let id = self.next_job;
        self.next_job += 1;
        let ctl = Arc::new(SearchControl::default());
        self.rjob = Some(ResultsJob { id, started: Instant::now(), what: JobKind::FindAll { ctl: ctl.clone(), total: doc.line_count().0, replace: false } });
        self.time_extract = extract;
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let t = Instant::now();
            match job(&doc, &ctl) {
                Ok((found, lines)) => {
                    let query = format!("{label}（{} 行）", group(lines));
                    results::post(hwnd, results::WM_RESULTS_DONE, JobDone::FindAll { id, query, found, elapsed: t.elapsed() })
                }
                Err(e) => results::post(hwnd, results::WM_RESULTS_DONE, JobDone::FilterFailed { id, message: e }),
            }
        });
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SEARCH, 100, None);
        }
        self.invalidate();
    }

    /// 絞り込みの結果（一覧）を、新しいタブに出す
    pub(super) fn time_extract_to_tab(&mut self) {
        let (Some(doc), Some(Results { kind: ResultKind::InDoc { ranges }, summary, .. })) = (self.doc.clone(), self.results.as_ref()) else { return };
        let bytes: u64 = ranges.iter().map(|&(s, e)| e - s).sum();
        if bytes > EXTRACT_LIMIT {
            self.message = format!("範囲が大きすぎて（{}）新しいタブに出せません。時刻の範囲をせばめてください（一覧からは移動できます）", human_size(bytes));
            return self.invalidate();
        }
        let mut out = String::with_capacity(bytes as usize + ranges.len() * 2);
        for &(s, e) in ranges {
            out.push_str(&doc.decode(&doc.read(s, e)));
            out.push('\n');
        }
        let summary = summary.clone();
        self.new_tab();
        let nl = self.newline_str();
        let text = if nl == "\n" { out } else { out.replace('\n', nl) };
        self.type_text(&text, EditKind::Other);
        self.sel = Some((0, 0));
        self.top = 0;
        self.message = format!("{summary} を新しいタブに出しました");
        self.invalidate();
    }

    /// 時刻へ移動：指定の時刻（範囲なら開始）以降で最初の行へ
    pub(super) fn goto_time(&mut self, spec: &str) {
        let Some(doc) = self.doc.clone() else { return };
        let target = match TimeRange::parse(spec) {
            Ok(r) => r.from,
            Err(e) => {
                self.message = e;
                return self.invalidate();
            }
        };
        self.last_time_goto = spec.trim().to_string();
        if !doc.line_count().1 {
            self.message = "索引の作成中は、時刻へ移動できません".into();
            return self.invalidate();
        }
        match logtime::find_time_line(&doc, target) {
            Some(line) => {
                self.scroll_to(line.saturating_sub(self.visible_lines() as u64 / 3));
                if let Some((s, e)) = doc.line_range(line) {
                    self.sel = Some((s, e));
                }
                self.message = format!("{} 行目（時刻順のログとして探しました）", group(line + 1));
            }
            None => self.message = "その時刻以降の行がありません（時刻のある行が見つかりません）".into(),
        }
        self.invalidate();
    }
}
