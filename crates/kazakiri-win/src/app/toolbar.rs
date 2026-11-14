//! ツールバー（メニューバーとタブバーの間）と、機能ボタンの列（ステータスバーの上）。
//!
//! どちらも自前で描く。ダークテーマでも標準の部品のように白くならず、アイコンは線と図形で描くので
//! 画面の解像度に関係なく崩れない。ボタンを押すと、メニューと同じコマンドを実行する。

use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_numerics::Vector2;

use super::*;

/// アイコンの種類
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Icon {
    New,
    Open,
    Save,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Find,
    Replace,
    Grep,
    Wrap,
    Whitespace,
    Csv,
    Bookmark,
    Next,
}

/// ツールバーのボタン（None は区切り）。(アイコン, コマンドの ID, 説明)
const BUTTONS: &[Option<(Icon, u32, &str)>] = &[
    Some((Icon::New, menu::ID_NEW, "新規作成 (Ctrl+N)")),
    Some((Icon::Open, menu::ID_OPEN, "開く (Ctrl+O)")),
    Some((Icon::Save, menu::ID_SAVE, "上書き保存 (Ctrl+S)")),
    None,
    Some((Icon::Undo, menu::ID_UNDO, "元に戻す (Ctrl+Z)")),
    Some((Icon::Redo, menu::ID_REDO, "やり直し (Ctrl+Y)")),
    None,
    Some((Icon::Cut, menu::ID_CUT, "切り取り (Ctrl+X)")),
    Some((Icon::Copy, menu::ID_COPY, "コピー (Ctrl+C)")),
    Some((Icon::Paste, menu::ID_PASTE, "貼り付け (Ctrl+V)")),
    None,
    Some((Icon::Find, menu::ID_FIND, "検索 (Ctrl+F)")),
    Some((Icon::Replace, menu::ID_REPLACE, "置換 (Ctrl+H)")),
    Some((Icon::Grep, menu::ID_GREP, "フォルダ内を検索 (Ctrl+Shift+F)")),
    None,
    Some((Icon::Bookmark, menu::ID_BM_TOGGLE, "ブックマークの設定／解除 (Ctrl+F2)")),
    Some((Icon::Next, menu::ID_BM_NEXT, "次のブックマーク (F2)")),
    None,
    Some((Icon::Wrap, menu::ID_WRAP, "折り返して表示")),
    Some((Icon::Whitespace, menu::ID_WHITESPACE, "空白・タブ・改行を表示 (Ctrl+Shift+W)")),
    Some((Icon::Csv, menu::ID_CSV, "CSV/TSV を表として表示 (Ctrl+Shift+C)")),
];

/// 機能ボタンの列（ステータスバーの上）。(表示, コマンドの ID)
const FKEYS: &[(&str, u32)] = &[
    ("検索  Ctrl+F", menu::ID_FIND),
    ("次を検索  F3", menu::ID_FIND_NEXT),
    ("前を検索  Shift+F3", menu::ID_FIND_PREV),
    ("置換  Ctrl+H", menu::ID_REPLACE),
    ("GREP", menu::ID_GREP),
    ("行へ移動  Ctrl+G", menu::ID_GOTO),
    ("ブックマーク  Ctrl+F2", menu::ID_BM_TOGGLE),
    ("次のBM  F2", menu::ID_BM_NEXT),
    ("折り返し", menu::ID_WRAP),
    ("空白表示", menu::ID_WHITESPACE),
    ("CSV 表示", menu::ID_CSV),
    ("ダーク", menu::ID_DARK),
];

/// ツールバーの 1 行目の高さと、モードバー（2 行目）の高さ
pub(super) const ROW1_H: f32 = 34.0;
pub(super) const MODE_H: f32 = 28.0;

/// モードバーのボタン。(表示, コマンドの ID, 説明)
const MODES: &[Option<(&str, u32, &str)>] = &[
    Some(("CSV", menu::ID_CSV, "CSV/TSV を表として表示 (Ctrl+Shift+C)")),
    None,
    Some(("カンマ", menu::ID_DELIM_BASE, "カンマ区切りで表示")),
    Some(("タブ", menu::ID_DELIM_BASE + 1, "タブ区切りで表示")),
    Some(("セミコロン", menu::ID_DELIM_BASE + 2, "セミコロン区切りで表示")),
    Some(("パイプ", menu::ID_DELIM_BASE + 3, "パイプ（|）区切りで表示")),
    None,
    Some(("セル選択", menu::ID_CELL_MODE, "ドラッグでセルの四角を選ぶ（Excel のように）。コピー・削除・入力は選んだセルの全行に効く")),
    Some(("見出し", menu::ID_CSV_HEADER, "1 行目を見出しにして、上に固定する")),
    None,
    Some(("列で昇順", menu::ID_SORT_ASC, "対象の列（見出しをクリックして選ぶ）で昇順に並べ替え")),
    Some(("列で降順", menu::ID_SORT_DESC, "対象の列（見出しをクリックして選ぶ）で降順に並べ替え")),
    Some(("列フィルタ", menu::ID_FILTER, "CSV/TSV：対象の列（見出しをクリックして選ぶ）の条件で行を絞り込む")),
    None,
    Some(("行フィルター", menu::ID_FILTER_LINES, "検索語を入力して、一致した行だけを新しいタブに出す (Ctrl+Shift+L)")),
    None,
    Some(("MDプレビュー", menu::ID_MD_SPLIT, "Markdown を右に整形して並べて表示 (Ctrl+Shift+V)")),
    Some(("デザインビュー", menu::ID_MD_DESIGN, "Markdown を整形して表示（編集には Esc で戻る） (Ctrl+Shift+G)")),
];

/// MODES のうち、ここから後ろは Markdown 用（先頭は CSV 用との間の区切り）
const MD_FROM: usize = 15;

const BTN_W: f32 = 30.0;
const SEP_W: f32 = 20.0;
/// MODES のうち、区切り文字の 4 つ（枠で囲んで 1 つのグループに見せる）
const DELIM_GROUP: std::ops::RangeInclusive<usize> = 2..=5;
const BTN_PAD: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Hover {
    Tool(usize),
    Fkey(usize),
    /// モードバーのボタン
    Mode(usize),
    /// モードバーの「対象の列」ドロップダウン
    ColPick,
    /// タブ
    Tab(usize),
    /// ステータスバーのクリックできる部分（`status_segments` の種類）
    Status(u8),
}

impl App {
    /// ツールバー全体の高さ（1 行目＋あればモードバー）
    pub(super) fn tb_h(&self) -> f32 {
        if !self.settings.toolbar {
            return 0.0;
        }
        ROW1_H + self.mode_h()
    }

    fn mode_h(&self) -> f32 {
        if self.settings.toolbar && self.settings.modebar && self.modebar_relevant() { MODE_H } else { 0.0 }
    }

    /// 開いているファイルの拡張子（小文字）
    fn doc_ext(&self) -> Option<String> {
        self.doc.as_ref().and_then(|d| d.path()).and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_ascii_lowercase())
    }

    /// CSV 用のボタンを出す場面（CSV の表示中、または CSV/TSV のファイル）
    fn csv_context(&self) -> bool {
        self.csv.is_some() || matches!(self.doc_ext().as_deref(), Some("csv" | "tsv"))
    }

    /// Markdown 用のボタンを出す場面（プレビュー中、または Markdown のファイル）
    fn md_context(&self) -> bool {
        !self.csv.is_some() && (self.md.is_some() || matches!(self.doc_ext().as_deref(), Some("md" | "markdown")))
    }

    /// モードバーが役に立つ場面か。ログやテキストでは出さずに、本文を広く使う。
    /// CSV の表示は、ツールバーの表のボタンか Ctrl+Shift+C で切り替える
    fn modebar_relevant(&self) -> bool {
        self.csv_context() || self.md_context()
    }

    /// MODES の番号 i のボタン（区切りを含む）を、いまの場面で出すか
    fn mode_shown(&self, i: usize) -> bool {
        if i < MD_FROM { self.csv_context() } else { self.md_context() && i > MD_FROM }
    }

    /// モードバーのボタンの位置 (x0, x1, 番号) と、「対象の列」ドロップダウンの位置 (x0, x1)。
    /// ドロップダウンは、並べ替え・列フィルタのボタンの直前に置く
    fn mode_layout(&self) -> (Vec<(f32, f32, usize)>, Option<(f32, f32)>) {
        let pick_w = self.colpick_label().map(|label| {
            let units: Vec<u16> = label.encode_utf16().collect();
            unsafe { self.ui_text_width(&units) }.unwrap_or(units.len() as f32 * self.ui_char_w) + 34.0
        });
        let mut x = BTN_PAD;
        let mut out = Vec::new();
        let mut pick = None;
        for (i, b) in MODES.iter().enumerate() {
            if !self.mode_shown(i) {
                continue;
            }
            match b {
                Some((label, id, _)) => {
                    if *id == menu::ID_SORT_ASC {
                        if let Some(w) = pick_w {
                            pick = Some((x, x + w));
                            x += w + 8.0;
                        }
                    }
                    let units: Vec<u16> = label.encode_utf16().collect();
                    let w = unsafe { self.ui_text_width(&units) }.unwrap_or(units.len() as f32 * self.ui_char_w) + 18.0;
                    out.push((x, x + w, i));
                    x += w + 2.0;
                }
                None => x += SEP_W,
            }
        }
        (out, pick)
    }

    fn mode_boxes(&self) -> Vec<(f32, f32, usize)> {
        self.mode_layout().0
    }

    /// 「対象の列」の表示（CSV 表示のとき）。ボタンの文字
    fn colpick_label(&self) -> Option<String> {
        let (col, _, name) = self.target_column()?;
        let follow = self.csv.as_ref().is_some_and(|v| v.target.is_none());
        Some(format!("対象の列: {}{}{}", col + 1, if name.is_empty() { String::new() } else { format!(": {name}") }, if follow { "（カーソル）" } else { "" }))
    }

    /// 「対象の列」の欄の左下（画面の座標）。ドロップダウンの一覧をここから開く
    pub(super) fn colpick_anchor(&self) -> Option<POINT> {
        let (x0, _) = self.colpick_box()?;
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let mut pt = POINT { x: (x0 * scale) as i32, y: ((ROW1_H + MODE_H - 2.0) * scale) as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        Some(pt)
    }

    /// 「対象の列」ドロップダウンの位置。(x0, x1)
    pub(super) fn colpick_box(&self) -> Option<(f32, f32)> {
        self.mode_layout().1
    }

    fn mode_checked(&self, id: u32) -> bool {
        match id {
            menu::ID_MD_SPLIT => return self.md_mode() == Some(mdview::MdMode::Split),
            menu::ID_MD_DESIGN => return self.md_mode() == Some(mdview::MdMode::Design),
            _ => {}
        }
        let Some(v) = self.csv.as_ref() else { return false };
        match id {
            menu::ID_CSV => true,
            menu::ID_CSV_HEADER => v.header,
            menu::ID_CELL_MODE => self.cell_mode,
            id if (menu::ID_DELIM_BASE..menu::ID_DELIM_BASE + 4).contains(&id) => menu::DELIMS[(id - menu::ID_DELIM_BASE) as usize].1 == v.delim,
            _ => false,
        }
    }

    pub(super) fn status_base_h(&self) -> f32 {
        self.ui_line_h + 6.0
    }

    pub(super) fn fkey_h(&self) -> f32 {
        if self.settings.fkeys { self.ui_line_h + 8.0 } else { 0.0 }
    }

    /// ボタンの位置。(x0, x1, ボタンの番号)
    fn tool_boxes(&self) -> Vec<(f32, f32, usize)> {
        let mut x = BTN_PAD;
        let mut out = Vec::new();
        for (i, b) in BUTTONS.iter().enumerate() {
            if b.is_some() {
                out.push((x, x + BTN_W, i));
                x += BTN_W;
            } else {
                x += SEP_W;
            }
        }
        out
    }

    /// 押されているトグルの状態
    fn tool_checked(&self, id: u32) -> bool {
        match id {
            menu::ID_WRAP => self.settings.wrap,
            menu::ID_WHITESPACE => self.settings.whitespace,
            menu::ID_CSV => self.csv.is_some(),
            menu::ID_DARK => self.settings.dark,
            menu::ID_MD_SPLIT => self.md_mode() == Some(mdview::MdMode::Split),
            menu::ID_MD_DESIGN => self.md_mode() == Some(mdview::MdMode::Design),
            _ => false,
        }
    }

    /// マウスの位置（本文の座標。タブバーの上が負）にあるボタン
    pub(super) fn hover_at(&self, x: f32, y: f32) -> Option<Hover> {
        let tb = self.tb_h();
        if (-self.tab_h()..0.0).contains(&y) {
            return self.tab_hover(x);
        }
        if tb > 0.0 && (-(self.tab_h() + tb)..-self.tab_h()).contains(&y) {
            // 上の行はツールバー、下の行はモードバー
            if y < -(self.tab_h() + self.mode_h()) {
                return self.tool_boxes().into_iter().find(|&(x0, x1, _)| x >= x0 && x < x1).map(|(_, _, i)| Hover::Tool(i));
            }
            if let Some((x0, x1)) = self.colpick_box() {
                if x >= x0 && x < x1 {
                    return Some(Hover::ColPick);
                }
            }
            return self.mode_boxes().into_iter().find(|&(x0, x1, _)| x >= x0 && x < x1).map(|(_, _, i)| Hover::Mode(i));
        }
        let (w, h) = self.client_size();
        if y >= h - self.status_base_h() && y < h {
            return self.status_hit(x).map(Hover::Status);
        }
        let fk = self.fkey_h();
        if fk > 0.0 && y >= h - self.status_h() && y < h - self.status_base_h() {
            let i = (x / (w / FKEYS.len() as f32)) as usize;
            return (i < FKEYS.len()).then_some(Hover::Fkey(i));
        }
        None
    }

    pub(super) fn hover_command(&self, hv: Hover) -> Option<(u32, &'static str)> {
        match hv {
            Hover::Tool(i) => BUTTONS.get(i).copied().flatten().map(|(_, id, tip)| (id, tip)),
            Hover::Mode(i) => MODES.get(i).copied().flatten().map(|(_, id, tip)| (id, tip)),
            Hover::ColPick => Some((menu::ID_COLPICK, "並べ替え・列フィルタの対象の列を選ぶ（見出しのクリックでも選べる）")),
            Hover::Fkey(i) => FKEYS.get(i).map(|&(label, id)| (id, label)),
            Hover::Tab(_) | Hover::Status(_) => None,
        }
    }

    /// マウスが動いた：ボタンの強調と、説明の表示
    pub(super) fn update_hover(&mut self, x: f32, y: f32) {
        let hv = self.hover_at(x, y);
        if hv != self.hover {
            self.hover = hv;
            self.hide_tip();
            // ツールバーのボタンの上なら、少し待ってから説明を出す
            if matches!(hv, Some(Hover::Tool(_) | Hover::Mode(_) | Hover::ColPick)) {
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_TIP, 450, None);
                }
            }
            self.invalidate();
        }
        if hv.is_some() && !self.tracking_leave {
            self.tracking_leave = true;
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut t);
            }
        }
    }

    pub(super) fn mouse_left(&mut self) {
        self.tracking_leave = false;
        self.hide_tip();
        if self.hover.take().is_some() {
            self.invalidate();
        }
    }

    /// 説明（ツールチップ）を消す
    pub(super) fn hide_tip(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TIP);
        }
        if self.tip.take().is_some() {
            self.invalidate();
        }
    }

    /// 待ち時間が過ぎても、まだ同じボタンの上なら説明を出す
    pub(super) fn show_tip(&mut self) {
        match self.hover {
            Some(Hover::Tool(i)) => {
                let Some((x0, _, _)) = self.tool_boxes().into_iter().find(|b| b.2 == i) else { return };
                if let Some((_, _, tip)) = BUTTONS.get(i).copied().flatten() {
                    self.tip = Some((tip.to_string(), x0, false));
                }
            }
            Some(Hover::Mode(i)) => {
                let Some((x0, _, _)) = self.mode_boxes().into_iter().find(|b| b.2 == i) else { return };
                if let Some((_, _, tip)) = MODES.get(i).copied().flatten() {
                    self.tip = Some((tip.to_string(), x0, true));
                }
            }
            Some(Hover::ColPick) => {
                let Some((x0, _)) = self.colpick_box() else { return };
                self.tip = Some(("並べ替え・列フィルタの対象の列を選ぶ（見出しのクリックでも選べる）".to_string(), x0, true));
            }
            _ => return,
        }
        self.invalidate();
    }

    // ---- 描画 ----

    /// 説明（ツールチップ）：ボタンの下に小さな枠で出す。窓の座標で描く
    pub(super) unsafe fn draw_tip(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let Some((text, x0, second_row)) = &self.tip else { return Ok(()) };
        let t = self.theme;
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let tw = self.ui_text_width(&units)?;
            let (bw, bh) = (tw + 16.0, self.ui_line_h + 8.0);
            let x = x0.min((w - bw - 4.0).max(0.0));
            let y = self.tb_h() + 4.0 - if *second_row { 0.0 } else { self.mode_h() };
            brush.SetColor(&t.dim);
            rt.FillRoundedRectangle(&rrect(x, y, x + bw, y + bh, 4.0), brush);
            brush.SetColor(&t.panel_bg);
            rt.FillRoundedRectangle(&rrect(x + 1.0, y + 1.0, x + bw - 1.0, y + bh - 1.0, 3.5), brush);
            brush.SetColor(&t.text);
            self.draw_ui_str(rt, brush, text, x + 8.0, y + 4.0, x + bw)?;
        }
        Ok(())
    }


    pub(super) unsafe fn draw_toolbar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let h = ROW1_H;
        let t = self.theme;
        unsafe {
            brush.SetColor(&t.toolbar_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, h), brush);
            for &(x0, x1, i) in &self.tool_boxes() {
                let Some((icon, id, _)) = BUTTONS[i] else { continue };
                let hovered = self.hover == Some(Hover::Tool(i));
                let on = self.tool_checked(id);
                if hovered || on {
                    brush.SetColor(if on { &t.panel_sel } else { &t.panel_head });
                    rt.FillRoundedRectangle(&rrect(x0, 3.0, x1, h - 3.0, 5.0), brush);
                }
                brush.SetColor(&t.text);
                self.draw_icon(rt, brush, icon, x0 + (BTN_W - 16.0) / 2.0, (h - 16.0) / 2.0);
            }
            // 区切り線（モードバーがなければ、ツールバーの下の線）
            if self.mode_h() == 0.0 {
                brush.SetColor(&t.dim);
                rt.FillRectangle(&rect(0.0, h - 1.0, w, h), brush);
            }
        }
        if self.mode_h() > 0.0 {
            rt.SetTransform(&Matrix3x2::translation(0.0, ROW1_H));
            self.draw_modebar(rt, brush, w)?;
            rt.SetTransform(&Matrix3x2::identity());
        }
        Ok(())
    }

    /// モードバー（ツールバーの 2 行目）：CSV の区切りなど、文字のボタン
    unsafe fn draw_modebar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let h = MODE_H;
        let t = self.theme;
        unsafe {
            brush.SetColor(&t.gutter_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, h), brush);
            let boxes = self.mode_boxes();
            // グループの境目に細い縦線
            brush.SetColor(&t.dim);
            for pair in boxes.windows(2) {
                if pair[1].2 > pair[0].2 + 1 {
                    let cx = (pair[0].1 + 2.0 + SEP_W / 2.0).floor() + 0.5;
                    rt.FillRectangle(&rect(cx, 7.0, cx + 1.0, h - 7.0), brush);
                }
            }
            // 区切り文字のグループを枠で囲む
            let first = boxes.iter().find(|b| b.2 == *DELIM_GROUP.start());
            let last = boxes.iter().find(|b| b.2 == *DELIM_GROUP.end());
            if let (Some(a), Some(b)) = (first, last) {
                rt.DrawRoundedRectangle(&rrect(a.0 - 2.5, 2.5, b.1 + 2.5, h - 2.5, 5.0), brush, 1.0, None);
            }
            for &(x0, x1, i) in &boxes {
                let Some((label, id, _)) = MODES[i] else { continue };
                let hovered = self.hover == Some(Hover::Mode(i));
                let on = self.mode_checked(id);
                if hovered || on {
                    brush.SetColor(if on { &t.panel_sel } else { &t.panel_head });
                    rt.FillRoundedRectangle(&rrect(x0, 3.0, x1, h - 3.0, 4.0), brush);
                }
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, label, x0 + 9.0, (h - self.ui_line_h) / 2.0, x1)?;
            }
            // CSV 表示のあいだ、並べ替え・列フィルタの対象の列を、ドロップダウンの形で見せる（押すと列の一覧が欄の下に開く）
            if let (Some(label), Some((x0, x1))) = (self.colpick_label(), self.colpick_box()) {
                let open = self.col_drop.is_some();
                let hovered = self.hover == Some(Hover::ColPick);
                // 枠：開いているあいだはアクセントカラー
                brush.SetColor(if open { &theme::accent(self.settings.dark) } else { &t.dim });
                rt.FillRoundedRectangle(&rrect(x0, 2.0, x1, h - 2.0, 6.0), brush);
                brush.SetColor(if hovered || open { &t.panel_head } else { &t.bg });
                rt.FillRoundedRectangle(&rrect(x0 + 1.0, 3.0, x1 - 1.0, h - 3.0, 5.0), brush);
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, &label, x0 + 10.0, (h - self.ui_line_h) / 2.0, x1 - 26.0)?;
                // ⌄（開いているときは ⌃）
                let (cx, cy) = (x1 - 14.0, h / 2.0);
                let dy = if open { 1.5 } else { -1.5 };
                brush.SetColor(&t.gutter_text);
                rt.DrawLine(Vector2 { X: cx - 4.0, Y: cy + dy - if open { 0.0 } else { 0.0 } }, Vector2 { X: cx, Y: cy - dy }, brush, 1.4, None);
                rt.DrawLine(Vector2 { X: cx, Y: cy - dy }, Vector2 { X: cx + 4.0, Y: cy + dy }, brush, 1.4, None);
            }
            brush.SetColor(&t.dim);
            rt.FillRectangle(&rect(0.0, h - 1.0, w, h), brush);
        }
        Ok(())
    }

    pub(super) unsafe fn draw_fkeys(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32, top: f32) -> Result<()> {
        let h = self.fkey_h();
        let t = self.theme;
        let cell = w / FKEYS.len() as f32;
        unsafe {
            brush.SetColor(&t.gutter_bg);
            rt.FillRectangle(&rect(0.0, top, w, top + h), brush);
            brush.SetColor(&t.dim);
            rt.FillRectangle(&rect(0.0, top, w, top + 1.0), brush);
            for (i, (label, id)) in FKEYS.iter().enumerate() {
                let x0 = i as f32 * cell;
                let hovered = self.hover == Some(Hover::Fkey(i));
                let on = self.tool_checked(*id);
                if hovered || on {
                    brush.SetColor(if on { &t.panel_sel } else { &t.panel_head });
                    rt.FillRoundedRectangle(&rrect(x0 + 2.0, top + 3.0, x0 + cell - 2.0, top + h - 2.0, 4.0), brush);
                }
                brush.SetColor(&t.text);
                rt.PushAxisAlignedClip(&rect(x0 + 2.0, top, x0 + cell - 2.0, top + h), D2D1_ANTIALIAS_MODE_ALIASED);
                self.draw_ui_str(rt, brush, label, x0 + 8.0, top + 4.0, x0 + cell - 4.0)?;
                rt.PopAxisAlignedClip();
            }
        }
        Ok(())
    }

    /// 16 × 16 の枠の中にアイコンを線で描く（色は現在のブラシ）
    unsafe fn draw_icon(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, icon: Icon, x: f32, y: f32) {
        let p = |a: f32, b: f32| Vector2 { X: x + a, Y: y + b };
        let sw = 1.4;
        unsafe {
            let ln = |a: f32, b: f32, c: f32, d: f32| rt.DrawLine(p(a, b), p(c, d), brush, sw, None);
            let rc = |a: f32, b: f32, c: f32, d: f32| rt.DrawRectangle(&rect(x + a, y + b, x + c, y + d), brush, sw, None);
            let fr = |a: f32, b: f32, c: f32, d: f32| rt.FillRectangle(&rect(x + a, y + b, x + c, y + d), brush);
            let ci = |cx: f32, cy: f32, r: f32| rt.DrawEllipse(&D2D1_ELLIPSE { point: p(cx, cy), radiusX: r, radiusY: r }, brush, sw, None);
            let dot = |cx: f32, cy: f32, r: f32| rt.FillEllipse(&D2D1_ELLIPSE { point: p(cx, cy), radiusX: r, radiusY: r }, brush);
            match icon {
                Icon::New => {
                    rc(3.0, 1.5, 12.0, 14.5);
                    ln(8.0, 1.5, 8.0, 6.0);
                    ln(8.0, 6.0, 12.0, 6.0);
                }
                Icon::Open => {
                    rc(1.5, 5.0, 14.5, 13.5);
                    ln(1.5, 5.0, 1.5, 3.0);
                    ln(1.5, 3.0, 6.0, 3.0);
                    ln(6.0, 3.0, 7.5, 5.0);
                }
                Icon::Save => {
                    rc(2.0, 2.0, 14.0, 14.0);
                    fr(5.0, 2.0, 11.0, 6.0);
                    rc(4.5, 9.0, 11.5, 14.0);
                }
                Icon::Undo => {
                    ln(13.0, 12.0, 13.0, 7.0);
                    ln(13.0, 7.0, 4.5, 7.0);
                    ln(4.5, 7.0, 7.5, 4.0);
                    ln(4.5, 7.0, 7.5, 10.0);
                }
                Icon::Redo => {
                    ln(3.0, 12.0, 3.0, 7.0);
                    ln(3.0, 7.0, 11.5, 7.0);
                    ln(11.5, 7.0, 8.5, 4.0);
                    ln(11.5, 7.0, 8.5, 10.0);
                }
                Icon::Cut => {
                    ln(5.0, 1.5, 10.5, 10.0);
                    ln(11.0, 1.5, 5.5, 10.0);
                    ci(4.5, 12.5, 2.2);
                    ci(11.5, 12.5, 2.2);
                }
                Icon::Copy => {
                    rc(2.0, 2.0, 10.0, 11.0);
                    rc(6.0, 5.0, 14.0, 14.5);
                }
                Icon::Paste => {
                    rc(3.0, 3.0, 13.0, 15.0);
                    fr(6.0, 1.0, 10.0, 4.5);
                }
                Icon::Find => {
                    ci(6.5, 6.5, 4.5);
                    ln(10.0, 10.0, 14.5, 14.5);
                }
                Icon::Replace => {
                    ln(2.0, 5.0, 13.0, 5.0);
                    ln(13.0, 5.0, 10.0, 2.0);
                    ln(13.0, 5.0, 10.0, 8.0);
                    ln(14.0, 11.0, 3.0, 11.0);
                    ln(3.0, 11.0, 6.0, 8.0);
                    ln(3.0, 11.0, 6.0, 14.0);
                }
                Icon::Grep => {
                    rc(1.5, 3.5, 8.5, 11.5);
                    ci(10.5, 9.0, 3.0);
                    ln(12.7, 11.2, 15.0, 14.0);
                }
                Icon::Wrap => {
                    ln(2.0, 4.0, 14.0, 4.0);
                    ln(2.0, 8.0, 11.0, 8.0);
                    ln(11.0, 8.0, 11.0, 12.0);
                    ln(11.0, 12.0, 5.0, 12.0);
                    ln(5.0, 12.0, 7.0, 10.0);
                    ln(5.0, 12.0, 7.0, 14.0);
                }
                Icon::Whitespace => {
                    dot(3.5, 6.0, 1.1);
                    dot(8.0, 6.0, 1.1);
                    dot(12.5, 6.0, 1.1);
                    ln(2.0, 11.0, 13.0, 11.0);
                    ln(13.0, 11.0, 13.0, 8.5);
                }
                Icon::Csv => {
                    rc(2.0, 3.0, 14.0, 13.0);
                    ln(2.0, 8.0, 14.0, 8.0);
                    ln(6.0, 3.0, 6.0, 13.0);
                    ln(10.0, 3.0, 10.0, 13.0);
                }
                Icon::Bookmark => {
                    ln(4.0, 2.0, 12.0, 2.0);
                    ln(12.0, 2.0, 12.0, 14.0);
                    ln(12.0, 14.0, 8.0, 10.0);
                    ln(8.0, 10.0, 4.0, 14.0);
                    ln(4.0, 14.0, 4.0, 2.0);
                }
                Icon::Next => {
                    ln(8.0, 2.5, 8.0, 12.0);
                    ln(3.5, 8.0, 8.0, 12.5);
                    ln(12.5, 8.0, 8.0, 12.5);
                }
            }
        }
    }
}
