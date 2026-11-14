//! 描画（Direct2D / DirectWrite）：本文・行番号・ステータスバー・文字の幅の計測。

use super::*;

impl App {
    pub(super) unsafe fn ensure_gfx(&mut self) -> Result<()> {
        if self.gfx.is_some() {
            return Ok(());
        }
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc)?;
            let dpi = GetDpiForWindow(self.hwnd) as f32;
            let props = D2D1_RENDER_TARGET_PROPERTIES { dpiX: dpi, dpiY: dpi, ..Default::default() };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            let brush = target.CreateSolidColorBrush(&self.theme.text, None)?;
            let t = self.theme;
            let mk = |c: &D2D1_COLOR_F| -> Result<IUnknown> { target.CreateSolidColorBrush(c, None)?.cast() };
            let effects = vec![
                mk(&t.marks)?,
                mk(&t.log_error)?,
                mk(&t.log_warn)?,
                mk(&t.log_debug)?,
                mk(&t.syn_str)?,
                mk(&t.syn_num)?,
                mk(&t.syn_kw)?,
                mk(&t.syn_comment)?,
                mk(&t.syn_tag)?,
                mk(&t.syn_key)?,
                mk(&theme::accent(self.settings.dark))?,
            ];
            let md = vec![mk(&theme::accent(self.settings.dark))?, mk(&t.syn_tag)?, mk(&t.syn_str)?, mk(&t.syn_num)?, mk(&t.syn_kw)?, mk(&t.syn_comment)?, mk(&t.syn_tag)?, mk(&t.syn_key)?];
            self.gfx = Some(Gfx { target, brush, effects, md });
            if let Some(v) = &self.md {
                v.reset();
            }
            // レイアウトは古いブラシを指しているので作り直す
            self.layout_cache.borrow_mut().clear();
        }
        Ok(())
    }

    pub(super) unsafe fn paint(&mut self) -> Result<()> {
        unsafe {
            // CSV 表示のあいだ、等幅でないフォントは、列がそろうよう等幅フォントに替える（CSV でなくなれば戻す）
            if (self.font_prop && self.csv.is_some()) != self.font_overridden {
                self.apply_font();
                if self.font_overridden {
                    self.message = "CSV 表示のあいだは、列をそろえるため等幅フォントにしています".into();
                }
            }
            if !self.message.is_empty() && self.message != self.last_message {
                self.last_message = self.message.clone();
                self.message_at = Instant::now();
                SetTimer(Some(self.hwnd), TIMER_MSG, MESSAGE_SECS * 1000 + 200, None);
            }
            self.ensure_gfx()?;
            // スクロールバーの出し入れなどでクライアント領域の大きさが変わっても WM_SIZE が来ないことがあるので、毎回合わせる
            if let Some(g) = &self.gfx {
                let mut rc = RECT::default();
                let _ = GetClientRect(self.hwnd, &mut rc);
                let sz = g.target.GetPixelSize();
                if (rc.right as u32, rc.bottom as u32) != (sz.width, sz.height) {
                    let _ = g.target.Resize(&D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 });
                }
            }
            self.ensure_continuation();
            self.clamp_top();
            let (w, h) = self.client_size();
            let g = self.gfx.as_ref().unwrap();
            let rt: ID2D1RenderTarget = g.target.cast()?;
            let brush = &g.brush;
            rt.BeginDraw();
            rt.Clear(Some(&self.theme.bg));

            // メニューバー（ダークテーマのとき）とタブバーを描き、それより下は丸ごと下へずらして描く
            if self.tb_h() > 0.0 {
                rt.SetTransform(&Matrix3x2::identity());
                self.draw_toolbar(&rt, brush, w)?;
            }
            rt.SetTransform(&Matrix3x2::translation(0.0, self.tb_h()));
            self.draw_tabs(&rt, brush, w)?;
            rt.SetTransform(&Matrix3x2::translation(0.0, self.chrome_h()));
            let pin = self.pin_h();
            if pin > 0.0 {
                self.draw_pinned_header(&rt, brush, w)?;
            }
            // デザインビュー：本文の代わりに整形した Markdown を描く
            if self.md_mode() == Some(mdview::MdMode::Design) {
                rt.SetTransform(&Matrix3x2::translation(0.0, self.chrome_h()));
                self.draw_markdown(&rt, brush)?;
                self.draw_status(&rt, brush, w, h)?;
                rt.SetTransform(&Matrix3x2::identity());
                self.draw_tip(&rt, brush, w)?;
                return rt.EndDraw(None, None);
            }
            // 差分表示：本文の代わりに左右の比較を描く
            if self.diffview.is_some() {
                rt.SetTransform(&Matrix3x2::translation(0.0, self.chrome_h()));
                self.draw_diffview(&rt, brush, w, h - self.status_h())?;
                self.draw_status(&rt, brush, w, h)?;
                rt.SetTransform(&Matrix3x2::identity());
                return rt.EndDraw(None, None);
            }
            // 本文は固定した見出し行の下から
            rt.SetTransform(&Matrix3x2::translation(0.0, self.chrome_h() + pin));

            let text_h = self.text_height();
            let digits = group(self.total_lines()).len().max(4);
            let gutter = self.gutter_width();

            let lines = match &self.doc {
                Some(doc) => self.lines_view(doc, self.top, self.visible_lines() + 1),
                None => Vec::new(),
            };

            // 行番号の欄（本文と同じ背景。番号の色だけで区別する）
            brush.SetColor(&self.theme.bg);
            rt.FillRectangle(&rect(0.0, 0.0, gutter, text_h), brush);

            // 本文
            rt.PushAxisAlignedClip(&rect(gutter, 0.0, w, text_h), D2D1_ANTIALIAS_MODE_ALIASED);
            // 折り返し表示：行を折り返し行に分けて描く（行番号は最初の折り返し行だけ）
            let mut wrapped_numbers: Vec<(f32, u64)> = Vec::new();
            if self.wrap_on() {
                self.csv_measure(&lines);
                let mut y = 0.0;
                'rows: for line in &lines {
                    let rows = self.rows_of(line.clone());
                    for r in 0..rows.count() {
                        if y >= text_h {
                            break 'rows;
                        }
                        let (a, b) = rows.range(r);
                        let layout = self.row_layout(&rows.disp, a, b)?;
                        self.draw_row(&rt, brush, line, &rows.disp, &layout, (a, b), self.text_left(), y)?;
                        if r == 0 {
                            wrapped_numbers.push((y, line.number));
                        }
                        y += self.line_h;
                    }
                }
            }
            // 直前に描いた行のレイアウトは使い回す（1 行スクロールなら作り直すのは 1 行だけ）
            let lines_nowrap: &[Line] = if self.wrap_on() { &[] } else { &lines };
            self.csv_measure(lines_nowrap);
            let generation = self.csv.as_ref().map_or(0, |v| v.generation.get());
            let caret_col = self.target_column().map(|(c, _, _)| c);
            let mut old = std::mem::take(&mut *self.layout_cache.borrow_mut());
            let mut cache = HashMap::with_capacity(lines.len());
            for (i, line) in lines_nowrap.iter().enumerate() {
                let y = i as f32 * self.line_h;
                let entry = match old.remove(&line.start) {
                    Some(c) if c.end == line.end && c.generation == generation => c,
                    _ => {
                        let disp = self.display(line);
                        let layout = self.dw.CreateTextLayout(&disp.units, &self.format, 1.0e6, self.line_h)?;
                        for &(a, b, e) in &disp.effects {
                            let brush = g.brush_for(e);
                            let _ = layout.SetDrawingEffect(brush, DWRITE_TEXT_RANGE { startPosition: a, length: b - a });
                        }
                        CachedLine { end: line.end, generation, layout, disp }
                    }
                };
                if let Some(k) = caret_col {
                    if let Some(&(a, b)) = entry.disp.cols.get(k) {
                        self.fill_units(&rt, brush, &entry.layout, &entry.disp.units, a, b, self.text_left(), y, self.theme.column)?;
                    }
                }
                self.draw_line(&rt, brush, line, &entry, self.line_x0(line), y)?;
                cache.insert(line.start, entry);
            }
            *self.layout_cache.borrow_mut() = cache;
            rt.PopAxisAlignedClip();

            if self.wrap_on() {
                for &(y, number) in &wrapped_numbers {
                    let num = format!("{:>width$}", group(number + 1), width = digits);
                    brush.SetColor(&self.theme.gutter_text);
                    self.draw_str(&rt, brush, &num, self.char_w * 0.5, y, gutter)?;
                }
            }
            for (i, line) in lines_nowrap.iter().enumerate() {
                let y = i as f32 * self.line_h;
                let num = format!("{:>width$}", group(line.number + 1), width = digits);
                brush.SetColor(&self.theme.gutter_text);
                self.draw_str(&rt, brush, &num, self.char_w * 0.5, y, gutter)?;
            }
            // ブックマークの印（行番号の欄の左端に帯）
            if !self.bookmarks.is_empty() {
                brush.SetColor(&theme::accent(self.settings.dark));
                if self.wrap_on() {
                    for &(y, number) in &wrapped_numbers {
                        if self.bookmarks.contains(&number) {
                            rt.FillRectangle(&rect(0.0, y + 1.0, 4.0, y + self.line_h - 1.0), brush);
                        }
                    }
                }
                for (i, line) in lines_nowrap.iter().enumerate() {
                    if self.bookmarks.contains(&line.number) {
                        let y = i as f32 * self.line_h;
                        rt.FillRectangle(&rect(0.0, y + 1.0, 4.0, y + self.line_h - 1.0), brush);
                    }
                }
            }
            if lines.is_empty() && self.doc.is_some() {
                brush.SetColor(&self.theme.dim);
                self.draw_str(&rt, brush, "索引作成中…", gutter + 8.0, 0.0, w)?;
            }

            // 結果一覧とステータスバーは、見出し行の分を含めた位置に描く
            rt.SetTransform(&Matrix3x2::translation(0.0, self.chrome_h()));
            // 分割のプレビュー：エディタの右半分に重ねる
            if self.md_mode() == Some(mdview::MdMode::Split) {
                self.draw_markdown(&rt, brush)?;
            }
            self.draw_panel(&rt, brush, w, text_h + pin)?;
            self.draw_status(&rt, brush, w, h)?;
            self.draw_prompt_bar(&rt, brush, w, h)?;
            rt.SetTransform(&Matrix3x2::identity());
            self.draw_tip(&rt, brush, w)?;
            self.draw_col_drop(&rt, brush)?;
            rt.EndDraw(None, None)
        }
    }

    pub(super) unsafe fn draw_line(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, line: &Line, c: &CachedLine, x: f32, y: f32) -> Result<()> {
        unsafe { self.draw_row(rt, brush, line, &c.disp, &c.layout, (0, c.disp.units.len()), x, y) }
    }

    /// 行の一部（折り返し表示のときの 1 行分。折り返さなければ全体）を描く。
    /// `layout` は `d.units[row.0..row.1]` のレイアウト
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_row(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, line: &Line, d: &Disp, layout: &IDWriteTextLayout, row: (usize, usize), x: f32, y: f32) -> Result<()> {
        let (ra, rb) = (row.0 as u32, row.1 as u32);
        let units = &d.units[row.0..row.1];
        let last = row.1 == d.units.len();
        let local = |(ua, ub): (u32, u32)| (ua.clamp(ra, rb) - ra, ub.clamp(ra, rb) - ra);
        unsafe {
            if !self.markers.is_empty() {
                self.draw_markers(rt, brush, layout, units, x, y)?;
            }
            if let Some(doc) = &self.doc {
                // 選択範囲
                if let Some((a, b)) = self.selection() {
                    let next = line.end + newline_len(doc, line);
                    if a < next.max(line.start + 1) && b > line.start {
                        let (ua, ub) = local(self.span_units(doc, line, a, b, d));
                        self.fill_units(rt, brush, layout, units, ua, ub, x, y, self.theme.select)?;
                        // 改行まで選択していたら行末に印
                        if last && b > line.end && a <= line.end {
                            let ex = x + self.text_width(&units[..d.shown.saturating_sub(row.0).min(units.len())])?;
                            rt.FillRectangle(&rect(ex, y, ex + self.char_w, y + self.line_h), brush);
                        }
                    }
                }
                // 検索一致（選択と同じなら一致の色を優先）
                if let Some((hs, he)) = self.hit {
                    if hs < line.end.max(line.start + 1) && he > line.start {
                        let (ua, ub) = local(self.span_units(doc, line, hs, he, d));
                        self.fill_units(rt, brush, layout, units, ua, ub, x, y, self.theme.matched)?;
                    }
                }
            }
            // マルチカーソル：ほかのカーソルの選択
            if let Some(doc) = &self.doc {
                for &(anchor, head) in self.extra_cursors() {
                    let (a, b) = (anchor.min(head), anchor.max(head));
                    if a != b && a < line.end.max(line.start + 1) && b > line.start {
                        let (ua, ub) = local(self.span_units(doc, line, a, b, d));
                        self.fill_units(rt, brush, layout, units, ua, ub, x, y, self.theme.select)?;
                    }
                }
            }
            // 矩形選択
            if let Some((x1, x2, wide)) = self.rect_span(line.number) {
                if wide {
                    brush.SetColor(&self.theme.select);
                    rt.FillRectangle(&rect(x + x1, y, x + x2, y + self.line_h), brush);
                } else if self.prompt.is_none() {
                    brush.SetColor(&self.theme.caret);
                    rt.FillRectangle(&rect(x + x1 - 0.5, y + 1.0, x + x1 + 1.5, y + self.line_h - 1.0), brush);
                }
            }
            brush.SetColor(&self.theme.text);
            rt.DrawTextLayout(Vector2 { X: x, Y: y }, layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            // マルチカーソル：ほかのカーソル
            if let (Some(doc), false) = (&self.doc, self.prompt.is_some()) {
                for &(_, head) in self.extra_cursors() {
                    if head >= line.start && head <= line.end {
                        let (ua, _) = self.span_units(doc, line, head, head, d);
                        if ua >= ra && (ua < rb || (last && ua == rb)) {
                            let cx = x + self.text_width(&units[..(ua - ra) as usize])?;
                            brush.SetColor(&self.theme.caret);
                            rt.FillRectangle(&rect(cx - 0.5, y + 1.0, cx + 1.5, y + self.line_h - 1.0), brush);
                        }
                    }
                }
            }
            // カーソル
            if let (Some(doc), Some(_)) = (&self.doc, self.sel) {
                let c = self.caret();
                if c >= line.start && c <= line.end && self.prompt.is_none() {
                    let (ua, _) = self.span_units(doc, line, c, c, d);
                    // 折り返しの境目は、次の折り返し行の先頭に出す
                    if ua >= ra && (ua < rb || (last && ua == rb)) {
                        let cx = x + self.text_width(&units[..(ua - ra) as usize])?;
                        // IME の変換中の文字（あれば、カーソルの位置に重ねて描く）
                        if let Some(comp) = self.comp.as_ref().filter(|_| self.ime_inline_on()) {
                            return self.draw_composition(rt, brush, comp, cx, y);
                        }
                        let mut color = self.theme.caret;
                        if self.overwrite {
                            // 上書きモード：次の 1 文字を覆うブロック（文字が読めるよう半透明）
                            let next = units.get((ua - ra) as usize).copied().unwrap_or(0);
                            let w = if next == 0 || (ua - ra) as usize >= d.shown.saturating_sub(row.0) {
                                self.char_w
                            } else {
                                self.text_width(&units[(ua - ra) as usize..(ua - ra) as usize + 1 + usize::from((0xD800..0xDC00).contains(&next))]).unwrap_or(self.char_w)
                            };
                            color.a = 0.45;
                            brush.SetColor(&color);
                            rt.FillRectangle(&rect(cx, y + 1.0, cx + w.max(2.0), y + self.line_h - 1.0), brush);
                        } else {
                            brush.SetColor(&color);
                            rt.FillRectangle(&rect(cx - 0.5, y + 1.0, cx + 1.5, y + self.line_h - 1.0), brush);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// バイト範囲 [s, e) を、この行の描画文字列中の UTF-16 範囲に直す
    pub(super) fn span_units(&self, doc: &Document, line: &Line, s: u64, e: u64, d: &Disp) -> (u32, u32) {
        let s = s.clamp(line.start, line.end);
        let e = e.clamp(s, line.end);
        let a = decode_units(doc, &doc.read(line.start, s));
        let b = a + decode_units(doc, &doc.read(s, e));
        (d.to_disp(a) as u32, d.to_disp(b) as u32)
    }

    /// 行の中の UTF-16 位置 → ファイルのバイト位置
    pub(super) fn src_units_to_offset(&self, doc: &Document, line: &Line, units: usize) -> u64 {
        let prefix: Vec<u16> = line.text.encode_utf16().take(units).collect();
        let text = String::from_utf16_lossy(&prefix);
        (line.start + doc.encode(&text).len() as u64).min(line.end)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn fill_units(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        layout: &IDWriteTextLayout,
        units: &[u16],
        a: u32,
        b: u32,
        x: f32,
        y: f32,
        color: D2D1_COLOR_F,
    ) -> Result<()> {
        if b <= a {
            return Ok(());
        }
        unsafe {
            brush.SetColor(&color);
            let mut m = [DWRITE_HIT_TEST_METRICS::default(); 8];
            let mut n = 0u32;
            if layout.HitTestTextRange(a, b - a, x, y, Some(&mut m), &mut n).is_ok() {
                for r in &m[..n as usize] {
                    rt.FillRectangle(&rect(r.left, y, r.left + r.width, y + self.line_h), brush);
                }
            } else {
                // HitTestTextRange が使えない環境（Wine など）向け: 前後の文字列幅から求める
                let l = self.text_width(&units[..a as usize])?;
                let r = self.text_width(&units[..b as usize])?;
                rt.FillRectangle(&rect(x + l, y, x + r, y + self.line_h), brush);
            }
        }
        Ok(())
    }

    pub(super) unsafe fn text_width(&self, units: &[u16]) -> Result<f32> {
        if units.is_empty() {
            return Ok(0.0);
        }
        unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.format, 1.0e6, self.line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(m.widthIncludingTrailingWhitespace)
        }
    }

    /// 画面の部品用のフォントでの文字列の幅（同じ文字列は覚えておく。メニュー・タブ・ステータスバーを描くたびに測り直さない）
    pub(super) unsafe fn ui_text_width(&self, units: &[u16]) -> Result<f32> {
        if units.is_empty() {
            return Ok(0.0);
        }
        if let Some(&w) = self.ui_width_cache.borrow().get(units) {
            return Ok(w);
        }
        let w = unsafe {
            let layout = self.dw.CreateTextLayout(units, &self.ui_format, 1.0e6, self.ui_line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            m.widthIncludingTrailingWhitespace
        };
        let mut cache = self.ui_width_cache.borrow_mut();
        // 増えすぎないよう、一定数を超えたら捨てる（ステータスバーの数字などで少しずつ増えるため）
        if cache.len() > 2048 {
            cache.clear();
        }
        cache.insert(units.to_vec(), w);
        Ok(w)
    }

    /// 画面の部品用のフォントで文字列を描く
    pub(super) unsafe fn draw_ui_str(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, s: &str, x: f32, y: f32, right: f32) -> Result<()> {
        let units: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            rt.DrawText(&units, &self.ui_format, &rect(x, y, right.max(x + 1.0), y + self.ui_line_h), brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        Ok(())
    }

    pub(super) unsafe fn draw_str(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, s: &str, x: f32, y: f32, right: f32) -> Result<()> {
        let units: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            rt.DrawText(
                &units,
                &self.format,
                &rect(x, y, right.max(x + 1.0), y + self.line_h),
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    pub(super) fn status_left(&self) -> String {
        let flags = format!(
            "{}{}",
            if self.opts.case_sensitive { "[Aa]" } else { "" },
            if self.opts.regex { "[.*]" } else { "" }
        );
        match self.prompt {
            // 入力欄は、ステータスバーの上の枠（draw_prompt_bar）に出す
            Some(_) => "入力中… Enter で実行、Esc で取消".to_string(),
            None if self.rjob.is_some() => self.job_status(self.rjob.as_ref().unwrap()),
            None if self.saving.is_some() => {
                let job = self.saving.as_ref().unwrap();
                let pct = job.written.load(Ordering::Relaxed) as f64 * 100.0 / job.total.max(1) as f64;
                let name = job.target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                format!("保存中… {:.0}%  {name}  (Esc で中止)", pct.min(100.0))
            }
            None => match &self.search {
                Some(job) => {
                    let pct = job.ctl.scanned.load(Ordering::Relaxed) as f64 * 100.0 / job.span.max(1) as f64;
                    let dir = if job.dir == Direction::Forward { "↓" } else { "↑" };
                    format!("検索中{dir}{flags} 「{}」 {:.0}%  {:.1} 秒  (Esc で中止)", self.last_find, pct.min(100.0), job.started.elapsed().as_secs_f64())
                }
                None if !self.message.is_empty() && self.message_at.elapsed().as_secs() < MESSAGE_SECS as u64 => self.message.clone(),
                None => self.doc_summary(),
            },
        }
    }

    /// ステータスバーの左に出す、ファイルの概要（行数・CSV なら列数）
    fn doc_summary(&self) -> String {
        let Some(doc) = self.doc.as_ref() else { return String::new() };
        let (lines, exact) = doc.line_count();
        let mut s = format!("{}{} 行", if exact { "" } else { "約" }, group(lines));
        if let Some((_, cols, _)) = self.csv.as_ref().and_then(|_| self.target_column()) {
            s.push_str(&format!(" · {cols} 列"));
        }
        s
    }

    /// 入力欄のカード（本文の右上に浮かせる）の位置。(x0, y0, x1, y1)。ツールバーとタブバーを除いた座標
    pub(super) fn prompt_card(&self) -> (f32, f32, f32, f32) {
        if self.is_find_bar() {
            return self.find_layout().card;
        }
        let w = self.client_size().0;
        let cw = (w - 40.0).clamp(280.0, 560.0).min(w);
        let (x0, y0) = ((w - cw - 20.0).max(0.0), self.pin_h() + 10.0);
        (x0, y0, x0 + cw, y0 + 14.0 + self.ui_line_h * 3.0 + 14.0 + 10.0 + 6.0 + self.filter_chip_row_h())
    }

    /// 入力欄（カードの中の入力の枠）の位置。(x0, y0, x1, y1)
    pub(super) fn prompt_input_rect(&self) -> (f32, f32, f32, f32) {
        if self.is_find_bar() {
            let l = self.find_layout();
            return if self.prompt_focus == 1 { l.repl.unwrap_or(l.find) } else { l.find };
        }
        let (x0, y0, x1, _) = self.prompt_card();
        let top = y0 + 10.0 + self.ui_line_h + 6.0;
        (x0 + 12.0, top, x1 - 12.0, top + self.ui_line_h + 8.0)
    }

    /// 入力欄の文字の上端（ツールバーとタブバーを除いた座標）
    pub(super) fn prompt_input_top(&self) -> f32 {
        self.prompt_input_rect().1 + 4.0
    }

    /// 入力欄（行へ移動・列フィルタ・分割・連番）を、本文の右上に浮かせたカードで出す。1 行目は題と操作、2 行目は入力、3 行目は説明
    pub(super) unsafe fn draw_prompt_bar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, _w: f32, _h: f32) -> Result<()> {
        let Some(prompt) = self.prompt else { return Ok(()) };
        if self.is_find_bar() {
            return unsafe { self.draw_find_bar(rt, brush) };
        }
        let t = self.theme;
        let (title, hint) = match prompt {
            Prompt::Find | Prompt::Replace => (String::new(), String::new()),
            Prompt::GotoLine => ("行へ移動".to_string(), "行番号を入力して Enter".to_string()),
            Prompt::LineFilter => ("行フィルター".to_string(), format!("検索語を入力して Enter。一致した行だけを新しいタブに出します（{}{}）", if self.opts.case_sensitive { "大文字小文字を区別" } else { "大文字小文字を区別しない" }, if self.opts.regex { "・正規表現" } else { "" })),
            Prompt::Sequence => ("連番を挿入".to_string(), "開始値,増分,桁数（例: 1,1,3 → 001, 002…）を入力して Enter".to_string()),
            Prompt::Split => ("ファイルを分割".to_string(), "1 ファイルの行数、またはサイズ（例: 1000000 / 512MB / 2GB）を入力して Enter".to_string()),
            Prompt::ColumnFilter => {
                let names: Vec<String> = self.csv_column_names(12).iter().enumerate().map(|(i, n)| format!("{}:{n}", i + 1)).collect();
                let cols = if names.is_empty() { String::new() } else { format!("　列 {}", names.join("  ")) };
                ("列フィルタ".to_string(), format!("「列番号 演算子 値」の形で、演算子は上のチップでも選べます（例: 3 = ERROR / 4 >= 100）。Enter で絞り込み{cols}"))
            }
        };
        let (cx0, cy0, cx1, cy1) = self.prompt_card();
        let (ix0, iy0, ix1, iy1) = self.prompt_input_rect();
        unsafe {
            // 影・枠・面
            brush.SetColor(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.22 });
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 + 2.0, cx1 + 1.0, cy1 + 4.0, 10.0), brush);
            brush.SetColor(&t.dim);
            rt.FillRoundedRectangle(&rrect(cx0 - 1.0, cy0 - 1.0, cx1 + 1.0, cy1 + 1.0, 9.0), brush);
            brush.SetColor(&t.panel_head);
            rt.FillRoundedRectangle(&rrect(cx0, cy0, cx1, cy1, 8.0), brush);
            // 題と、閉じ方
            brush.SetColor(&t.text);
            self.draw_ui_str(rt, brush, &title, cx0 + 14.0, cy0 + 8.0, cx1 - 120.0)?;
            brush.SetColor(&t.gutter_text);
            let esc: Vec<u16> = "Esc で閉じる".encode_utf16().collect();
            let ew = self.ui_text_width(&esc)?;
            self.draw_ui_str(rt, brush, "Esc で閉じる", cx1 - 14.0 - ew, cy0 + 8.0, cx1 - 8.0)?;
            // 説明（2 行まで折り返す）
            let hint_units: Vec<u16> = hint.encode_utf16().collect();
            brush.SetColor(&t.gutter_text);
            rt.DrawText(&hint_units, &self.ui_format, &rect(cx0 + 14.0, iy1 + 8.0 + self.filter_chip_row_h(), cx1 - 14.0, cy1 - 4.0), brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            // 入力欄
            self.draw_field(rt, brush, (ix0, iy0, ix1, iy1), &self.input, true)?;
            self.draw_filter_chips(rt, brush)?;
        }
        Ok(())
    }

    /// ステータスバーの右側（ファイルの情報）を、部分に分けたもの。Some(n) の部分はクリックできる
    /// （0 = 文字コード、1 = 改行コード、2 = 行・桁、3 = タブ幅、4 = 挿入／上書き）
    pub(super) fn status_segments(&self) -> Vec<(String, Option<u8>)> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let p = doc.progress();
        let (lines, exact) = doc.line_count();
        let lines = if exact { group(lines) } else { format!("約{}", group(lines)) };
        let idx = if p.done {
            String::new()
        } else {
            format!("索引 {:.0}%  ", p.scanned_bytes as f64 * 100.0 / p.total_bytes.max(1) as f64)
        };
        let sel = match self.selection() {
            Some((a, b)) => format!("選択 {}   ", human_size(b - a)),
            None => String::new(),
        };
        let follow = if self.follow { "追従中   " } else { "" };
        let modified = if self.is_modified() { "変更あり   " } else { "" };
        let enc = if doc.forced_encoding().is_some() { format!("{}（指定）", doc.encoding().name()) } else { doc.encoding().name() };
        // カーソルの行と列（列は文字数）
        let c = self.caret();
        let pos = match doc.line_of_offset(c).and_then(|l| doc.line_range(l).map(|r| (l, r))) {
            Some((l, (s, _))) if c - s <= 1 << 20 => match self.caret_column() {
                Some((k, n, name)) => {
                    let name = if name.is_empty() { String::new() } else { format!("「{}」", name.chars().take(24).collect::<String>()) };
                    if self.csv_letters {
                        format!("{} 行, 列 {}（{}/{}）{name}", group(l + 1), col_letters(k), k + 1, n)
                    } else {
                        format!("{} 行, {}/{} 列{name}", group(l + 1), k + 1, n)
                    }
                }
                None => {
                    let col = doc.decode(&doc.read(s, c)).chars().count() + 1;
                    format!("{} 行, {} 列", group(l + 1), group(col as u64))
                }
            },
            Some((l, _)) => format!("{} 行", group(l + 1)),
            None => format!("{} 行目付近", group(self.top + 1)),
        };
        vec![
            (format!("{modified}{follow}{idx}{sel}"), None),
            (pos, Some(2)),
            (format!(" / {lines} 行   "), None),
            (enc, Some(0)),
            ("   ".into(), None),
            (doc.newline_name().to_string(), Some(1)),
            ("   ".into(), None),
            (format!("タブ {}", self.settings.tab_width), Some(3)),
            ("   ".into(), None),
            ((if self.overwrite { "上書き" } else { "挿入" }).into(), Some(4)),
            ("   ".into(), None),
            (human_size(doc.len()), None),
        ]
    }

    pub(super) unsafe fn draw_status(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, h: f32) -> Result<()> {
        let top = h - self.status_base_h();
        unsafe {
            if self.fkey_h() > 0.0 {
                self.draw_fkeys(rt, brush, w, top - self.fkey_h())?;
            }
            brush.SetColor(&self.theme.status_bg);
            rt.FillRectangle(&rect(0.0, top, w, h), brush);
            brush.SetColor(&self.theme.panel_head);
            rt.FillRectangle(&rect(0.0, top, w, top + 1.0), brush);
            brush.SetColor(&self.theme.status_text);
        }
        let left = self.status_left();
        let segs = self.status_segments();
        let right: String = segs.iter().map(|(s, _)| s.as_str()).collect();
        unsafe {
            // 右側（ファイルの情報）を優先し、左側（メッセージ）は残りの幅で切る
            let units: Vec<u16> = right.encode_utf16().collect();
            let layout = self.dw.CreateTextLayout(&units, &self.ui_format, w, self.ui_line_h)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let rw = m.widthIncludingTrailingWhitespace.min(w * 0.75);
            // クリックできる部分は、ホバーしたときに角丸の背景（チップ）を出す。位置は status_hit と同じ並べ方
            let mut x0 = w - rw - 8.0;
            for (text, kind) in &segs {
                let u: Vec<u16> = text.encode_utf16().collect();
                let x1 = x0 + self.ui_text_width(&u)?;
                if let Some(k) = kind {
                    if self.hover == Some(super::toolbar::Hover::Status(*k)) {
                        brush.SetColor(&self.theme.panel_head);
                        rt.FillRoundedRectangle(&rrect(x0 - 5.0, top + 2.0, x1 + 5.0, h - 2.0, 5.0), brush);
                    }
                }
                brush.SetColor(&self.theme.status_text);
                self.draw_ui_str(rt, brush, text, x0, top + 3.0, x1 + 1.0)?;
                x0 = x1;
            }
            brush.SetColor(&self.theme.status_text);
            // 右側と重ならないように切り取る（文字の切り取り指定が効かない環境もあるので領域で切る）
            rt.PushAxisAlignedClip(&rect(0.0, top, (w - rw - 24.0).max(0.0), h), D2D1_ANTIALIAS_MODE_ALIASED);
            self.draw_ui_str(rt, brush, &left, 8.0, top + 3.0, w - rw - 24.0)?;
            rt.PopAxisAlignedClip();
        }
        Ok(())
    }
}

/// 列の番号（0 始まり）を A, B, … Z, AA, AB, … にする
fn col_letters(mut k: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (k % 26) as u8);
        if k < 26 {
            break;
        }
        k = k / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}
