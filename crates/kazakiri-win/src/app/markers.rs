//! カラーマーカー：選んだ文字列（なければカーソルの単語）に色を付けて、目印にする（検索 → カラーマーカー）。
//! 同じ文字列をもう一度マークすると外れる。色は 6 色。ファイルの中の同じ文字列すべてに色が付く。

use super::*;

/// マーク 1 つ
pub(super) struct Marker {
    units: Vec<u16>,
    color: usize,
}

pub(super) const MARK_COLORS: usize = 6;

/// 色（背景）。ライト・ダークそれぞれの 6 色
fn marker_color(i: usize, dark: bool) -> D2D1_COLOR_F {
    let l = [(0xFF, 0xF1, 0x76), (0xA5, 0xF3, 0xA5), (0xA8, 0xD8, 0xFF), (0xFF, 0xC4, 0xE1), (0xFF, 0xC9, 0x8B), (0xD5, 0xC2, 0xFF)];
    let d = [(0x6B, 0x5E, 0x10), (0x1F, 0x5E, 0x2E), (0x1F, 0x4E, 0x7A), (0x7A, 0x2B, 0x55), (0x7A, 0x45, 0x12), (0x4B, 0x3A, 0x7A)];
    let (r, g, b) = if dark { d[i % MARK_COLORS] } else { l[i % MARK_COLORS] };
    rgb(r, g, b)
}

impl App {
    /// マークする文字列：選択（1 行・200 文字まで）、なければカーソルの単語
    fn marker_text(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        if let Some(s) = self.selected_text(2048).filter(|s| !s.contains(['\r', '\n']) && s.chars().count() <= 200) {
            return Some(s);
        }
        let c = self.caret();
        let (ls, le) = doc.line_of_offset(c).and_then(|l| doc.line_range(l))?;
        let (from, to) = (c.saturating_sub(256).max(ls), (c + 256).min(le));
        let before: Vec<char> = doc.decode(&doc.read(from, c)).chars().collect();
        let after: Vec<char> = doc.decode(&doc.read(c, to)).chars().collect();
        let is_w = |ch: &char| ch.is_alphanumeric() || *ch == '_';
        let head: String = before.iter().rev().take_while(|ch| is_w(ch)).collect::<Vec<_>>().into_iter().rev().collect();
        let tail: String = after.iter().take_while(|ch| is_w(ch)).collect();
        let word = format!("{head}{tail}");
        (!word.is_empty()).then_some(word)
    }

    pub(super) fn toggle_marker(&mut self, color: usize) {
        let Some(text) = self.marker_text() else {
            self.message = "マークする文字列を選ぶか、単語の上にカーソルを置いてください".into();
            return self.invalidate();
        };
        let units: Vec<u16> = text.encode_utf16().collect();
        if let Some(i) = self.markers.iter().position(|m| m.units == units) {
            self.markers.remove(i);
            self.message = format!("「{text}」のマークを外しました");
        } else {
            self.markers.push(Marker { units, color });
            self.message = format!("「{text}」にマークを付けました（色 {}）", color + 1);
        }
        self.invalidate();
    }

    pub(super) fn clear_markers(&mut self) {
        let n = self.markers.len();
        self.markers.clear();
        self.message = format!("マークをすべて外しました（{n} 件）");
        self.invalidate();
    }

    /// 1 行（または折り返しの 1 段）にマークの色を塗る
    pub(super) unsafe fn draw_markers(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, layout: &IDWriteTextLayout, units: &[u16], x: f32, y: f32) -> Result<()> {
        for m in &self.markers {
            let n = m.units.len();
            if n == 0 || units.len() < n {
                continue;
            }
            let color = marker_color(m.color, self.settings.dark);
            let mut i = 0;
            while i + n <= units.len() {
                if units[i..i + n] == m.units[..] {
                    unsafe { self.fill_units(rt, brush, layout, units, i as u32, (i + n) as u32, x, y, color)? };
                    i += n;
                } else {
                    i += 1;
                }
            }
        }
        Ok(())
    }
}
