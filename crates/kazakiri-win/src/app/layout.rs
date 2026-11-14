//! 表示の寸法（行数・余白・スクロール位置・スクロールバー）と、位置の変換（オフセット ⇔ 画面）。

use super::*;

impl App {
    /// 行を取り出す。横にスクロールしているとき、64KB を超える長い行は、その位置の窓（`Line::skipped` が 0 でない）にする
    pub(super) fn lines_view(&self, doc: &Document, first: u64, count: usize) -> Vec<Line> {
        let mut v = doc.lines(first, count);
        if !self.wrap_on() && self.csv.is_none() {
            // 1 桁 ≒ 1 バイトとして、横の位置を行の中のバイト位置にする。窓は、画面の幅の 3 倍ほどのバイト数
            let cols = (self.editor_w() / self.char_w.max(1.0)) as usize;
            let window = (cols * 3 + 64).clamp(256, LONG_LINE_WINDOW);
            let from = (self.x_off / self.char_w.max(1.0)) as u64;
            for l in &mut v {
                if l.truncated {
                    if let Some(w) = doc.line_window(l.number, from, window) {
                        *l = w;
                    }
                }
            }
        }
        v
    }

    /// 行頭の x 座標。長い行の窓は、横スクロール分をすでに窓の位置で消費している
    pub(super) fn line_x0(&self, line: &Line) -> f32 {
        if line.skipped > 0 { self.gutter_width() + 4.0 } else { self.text_left() }
    }

    /// 画面上の点に対応するバイト位置
    pub(super) fn offset_at(&self, doc: &Document, x: f32, y: f32) -> Option<u64> {
        if self.wrap_on() {
            return self.offset_at_wrapped(doc, x, y);
        }
        let line_no = self.top + (y.max(0.0) / self.line_h) as u64;
        let Some(line) = self.lines_view(doc, line_no, 1).into_iter().next() else {
            return doc.progress().done.then(|| doc.len());
        };
        let d = self.display(&line);
        let tx = x - self.line_x0(&line);
        let idx = unsafe { self.unit_at_x(&d.units[..d.shown], tx) }.unwrap_or(0).min(d.shown);
        Some(self.src_units_to_offset(doc, &line, d.to_src(idx)))
    }

    /// x 座標（行頭基準）に最も近い文字境界の UTF-16 位置
    pub(super) unsafe fn unit_at_x(&self, units: &[u16], x: f32) -> Result<usize> {
        if x <= 0.0 || units.is_empty() {
            return Ok(0);
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.format, 1.0e6, self.line_h)?;
            let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            if layout.HitTestPoint(x, self.line_h / 2.0, &mut trailing, &mut inside, &mut m).is_ok() {
                let i = m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 };
                return Ok(i.min(units.len()));
            }
            // HitTestPoint が使えない環境（Wine など）向け：幅を二分探索
            // lo = 幅が x 以下になる最大の位置
            let (mut lo, mut hi) = (0usize, units.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if self.text_width(&units[..mid])? <= x {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            // 次の文字境界の方が近ければそちら
            if lo < units.len() {
                let (wl, wr) = (self.text_width(&units[..lo])?, self.text_width(&units[..lo + 1])?);
                if x - wl > wr - x {
                    lo += 1;
                }
            }
            // サロゲートペアの途中は避ける
            if lo > 0 && lo < units.len() && (0xDC00..0xE000).contains(&units[lo]) {
                lo -= 1;
            }
            Ok(lo)
        }
    }

    pub(super) fn selection(&self) -> Option<(u64, u64)> {
        self.sel.map(|(a, b)| (a.min(b), a.max(b))).filter(|(a, b)| a < b)
    }

    pub(super) fn selected_text(&self, limit: u64) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let (a, b) = self.selection()?;
        (b - a <= limit).then(|| doc.decode(&doc.read(a, b)).into_owned())
    }

    pub(super) fn copy(&mut self) {
        if self.rect_copy() || self.multi_copy() {
            return;
        }
        let Some((a, b)) = self.selection() else { return };
        if b - a > COPY_LIMIT {
            self.message = format!("選択範囲が大きすぎます（{}）。コピーできるのは {} までです", human_size(b - a), human_size(COPY_LIMIT));
        } else if let Some(text) = self.selected_text(COPY_LIMIT) {
            self.message = match clipboard::set_text(self.hwnd, &text) {
                Ok(()) => format!("コピーしました（{}）", human_size(b - a)),
                Err(e) => format!("コピーに失敗しました: {e}"),
            };
        }
        self.invalidate();
    }

    // ---- スクロール ----

    pub(super) fn total_lines(&self) -> u64 {
        self.doc.as_ref().map_or(1, |d| d.line_count().0)
    }

    pub(super) fn client_size(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            // タブバー・ツールバーと、スクロールバーを除いた大きさ（本文・結果一覧・ステータスバーの領域）
            let (vw, hh) = self.scrollbar_px();
            let hh = if self.hbar_on.get() { hh } else { 0 };
            ((rc.right - rc.left - vw) as f32 / scale, ((rc.bottom - rc.top - hh) as f32 / scale - self.chrome_h()).max(0.0))
        }
    }

    /// ステータスバー（と、あれば機能ボタンの列）の高さ
    /// 本文より上の部分（タブバー＋ツールバー＋モードバー）の高さ。いちばん上がタブバー（タイトルバーを兼ねる）
    pub(super) fn chrome_h(&self) -> f32 {
        self.tb_h() + self.tab_h()
    }

    pub(super) fn status_h(&self) -> f32 {
        self.status_base_h() + self.fkey_h()
    }

    pub(super) fn text_height(&self) -> f32 {
        self.client_size().1 - self.status_h() - self.panel_h() - self.pin_h()
    }

    /// 見出し行を上に固定して表示するか（CSV モードで「1 行目は見出し」のとき）
    pub(super) fn pinned(&self) -> bool {
        self.csv.as_ref().is_some_and(|v| v.header) && self.total_lines() > 1
    }

    /// 固定した見出し行の高さ（固定しないときは 0）
    pub(super) fn pin_h(&self) -> f32 {
        if self.pinned() { self.line_h } else { 0.0 }
    }

    /// 本文の先頭にできる行（見出し行を固定していれば 1 行目は固定領域に出るので 2 行目から）
    pub(super) fn min_top(&self) -> u64 {
        u64::from(self.pinned())
    }

    pub(super) fn clamp_top(&mut self) {
        let min = self.min_top();
        self.top = self.top.clamp(min, self.max_top().max(min));
    }

    pub(super) fn gutter_width(&self) -> f32 {
        let digits = group(self.total_lines()).len().max(4);
        self.char_w * (digits as f32 + 2.0)
    }

    /// 本文の行頭の x 座標（横スクロール込み）
    pub(super) fn text_left(&self) -> f32 {
        self.gutter_width() + 4.0 - self.x_off
    }

    pub(super) fn visible_lines(&self) -> usize {
        ((self.text_height() / self.line_h).floor() as usize).max(1)
    }

    pub(super) fn max_top(&self) -> u64 {
        if self.wrap_on() {
            return self.max_top_wrapped();
        }
        self.total_lines().saturating_sub(self.visible_lines() as u64)
    }

    pub(super) fn scroll_by(&mut self, delta: i64) {
        if delta < 0 {
            self.stick_bottom = false;
        }
        let t = (self.top as i64 + delta).max(0) as u64;
        self.scroll_to(t);
    }

    pub(super) fn scroll_to(&mut self, line: u64) {
        self.top = line;
        self.clamp_top();
        self.update_scrollbars();
        self.invalidate();
    }

    pub(super) fn hscroll_by(&mut self, dx: f32) {
        if self.wrap_on() {
            return;
        }
        self.x_off = (self.x_off + dx).max(0.0);
        self.update_scrollbars();
        self.invalidate();
    }

    /// スクロールバーの値は i32 なので、行数が大きいときは縮尺をかける
    pub(super) fn scroll_scale(&self) -> u64 {
        (self.total_lines() / (i32::MAX as u64 / 2)).max(1)
    }

    pub(super) fn update_scrollbars(&self) {
        // タブの切り替えなどで、モードバーが出入りして本文の上端が動くことがあるので、スクロールバーを置き直す
        self.layout_scrollbars();
        // Markdown のデザインビュー：プレビューの高さに合わせる（横スクロールバーは出さない）
        if let Some((total, page, pos)) = self.md_scroll_info() {
            let v = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_RANGE | SIF_PAGE | SIF_POS, nMin: 0, nMax: total, nPage: page, nPos: pos, nTrackPos: 0 };
            unsafe {
                SetScrollInfo(self.vbar, SB_CTL, &v, true);
            }
            self.set_hbar(false);
            return;
        }
        let scale = self.scroll_scale();
        let page = self.visible_lines() as u64;
        let total = self.total_lines();
        let v = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: (total.saturating_sub(1) / scale) as i32,
            nPage: (page / scale).max(1) as u32,
            nPos: (self.top / scale) as i32,
            nTrackPos: 0,
        };
        let w = self.editor_w();
        // 長い行があれば、その長さまでスクロールできる（1 桁 ≒ 1 バイト）
        let long_max = self.doc.as_ref().map_or(0.0, |d| {
            (self.top..self.top + self.visible_lines() as u64 + 1)
                .filter_map(|n| d.line_range(n))
                .map(|(s, e)| (e - s) as f32 * self.char_w)
                .fold(0.0f32, f32::max)
        });
        let hmax = (self.char_w * 2000.0).max(self.x_off + w).max(long_max).min(1.0e9);
        let h = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: hmax as i32,
            nPage: w.max(1.0) as u32,
            nPos: self.x_off as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.vbar, SB_CTL, &v, true);
            SetScrollInfo(self.hbar, SB_CTL, &h, true);
        }
        // 折り返し表示のときは横スクロールバーを出さない
        self.set_hbar(!self.wrap_on());
    }

    /// 縦・横のスクロールバーの幅（物理ピクセル）
    fn scrollbar_px(&self) -> (i32, i32) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        unsafe { (GetSystemMetricsForDpi(SM_CXVSCROLL, dpi), GetSystemMetricsForDpi(SM_CYHSCROLL, dpi)) }
    }

    /// 横のスクロールバーを出す／隠す（出し入れで本文の高さが変わるので、置き直す）
    fn set_hbar(&self, on: bool) {
        if self.hbar_on.get() != on {
            self.hbar_on.set(on);
            self.layout_scrollbars();
            self.invalidate();
        }
    }

    /// スクロールバー（子ウィンドウ）を、本文の右端・下端に置く。縦は、ツールバーの下から
    pub(super) fn layout_scrollbars(&self) {
        let mut rc = RECT::default();
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (vw, hh) = self.scrollbar_px();
        let hh = if self.hbar_on.get() { hh } else { 0 };
        let top = (self.chrome_h() * scale).round() as i32;
        // 位置が前回と同じなら何もしない（スクロールのたびに呼ばれるので）
        let placed = [rc.right, rc.bottom, top, vw, hh, 0];
        if self.sb_placed.get() == placed {
            return;
        }
        self.sb_placed.set(placed);
        unsafe {
            let _ = SetWindowPos(self.vbar, None, rc.right - vw, top, vw, (rc.bottom - top - hh).max(0), SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = SetWindowPos(self.hbar, None, 0, rc.bottom - hh.max(1), (rc.right - vw).max(0), hh.max(1), SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(self.hbar, if hh > 0 { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    pub(super) fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // ---- 描画 ----
}
