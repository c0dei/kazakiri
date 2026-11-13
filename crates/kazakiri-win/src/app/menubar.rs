//! 自分で描くメニューバー（ダークテーマでは必ず。ライトテーマでは設定 `menubar = custom` のとき）。
//!
//! ネイティブのメニューバーは Windows が白く塗り、暗くする公式の方法がない。
//! そこでダークテーマのあいだはネイティブのバーを外し（`SetMenu(None)`）、タブバーと同じように
//! 窓の一番上へ自分で描く。項目を押すと、同じ `HMENU` をポップアップで出す
//! （項目・チェック・WM_COMMAND はネイティブのときと同じ）。Alt+F などの呼び出しにも対応する。

use super::*;

/// 項目の左右の余白
const PAD: f32 = 10.0;

pub(super) struct BarItem {
    pub label: String,
    pub menu: HMENU,
    pub x0: f32,
    pub x1: f32,
    /// Alt と一緒に押すキー（`&F` の F）
    pub mnemonic: Option<char>,
}

impl App {
    /// 自前のメニューバーの高さ（使っていなければ 0）
    pub(super) fn menu_h(&self) -> f32 {
        if self.custom_menu { self.ui_line_h + 8.0 } else { 0.0 }
    }

    /// タブバーより上の部分（メニューバー＋タブバー）の高さ
    pub(super) fn chrome_h(&self) -> f32 {
        self.menu_h() + self.tb_h() + self.tab_h()
    }

    /// ネイティブのバーから項目を読み取って、並べる
    pub(super) fn bar_items(&self) -> Vec<BarItem> {
        let mut out = Vec::new();
        let mut x = 2.0;
        unsafe {
            let n = GetMenuItemCount(Some(self.menus.bar)).max(0);
            for i in 0..n {
                let mut buf = [0u16; 64];
                let len = GetMenuStringW(self.menus.bar, i as u32, Some(&mut buf), MF_BYPOSITION) as usize;
                let raw = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
                let Some(menu) = Some(GetSubMenu(self.menus.bar, i)).filter(|m| !m.is_invalid()) else { continue };
                // 「ファイル(&F)」→ 表示は「ファイル(F)」、Alt+F で開く
                let mnemonic = raw.split_once('&').and_then(|(_, r)| r.chars().next());
                let label = raw.replace('&', "");
                let units: Vec<u16> = label.encode_utf16().collect();
                let w = self.ui_text_width(&units).unwrap_or(units.len() as f32 * self.ui_char_w) + PAD * 2.0;
                out.push(BarItem { label, menu, x0: x, x1: x + w, mnemonic });
                x += w;
            }
        }
        out
    }

    pub(super) unsafe fn draw_menu_bar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, w: f32) -> Result<()> {
        let h = self.menu_h();
        let t = self.theme;
        unsafe {
            brush.SetColor(&t.gutter_bg);
            rt.FillRectangle(&rect(0.0, 0.0, w, h), brush);
            for (i, b) in self.bar_items().iter().enumerate() {
                if self.menu_open == Some(i) {
                    brush.SetColor(&t.panel_head);
                    rt.FillRoundedRectangle(&rrect(b.x0 + 2.0, 3.0, b.x1 - 2.0, h - 3.0, 4.0), brush);
                }
                brush.SetColor(&t.text);
                self.draw_ui_str(rt, brush, &b.label, b.x0 + PAD, (h - self.ui_line_h) / 2.0, b.x1)?;
            }
        }
        Ok(())
    }

    /// バーの中の x にある項目の番号
    pub(super) fn bar_item_at(&self, x: f32) -> Option<usize> {
        self.bar_items().iter().position(|b| x >= b.x0 && x < b.x1)
    }

    /// 項目 `i` のポップアップを出す位置（画面座標）と、そのメニュー
    pub(super) fn bar_popup(&mut self, i: usize) -> Option<(HMENU, i32, i32)> {
        let items = self.bar_items();
        let b = items.get(i)?;
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let mut pt = POINT { x: (b.x0 * scale) as i32, y: (self.menu_h() * scale) as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        self.menu_open = Some(i);
        self.invalidate();
        Some((b.menu, pt.x, pt.y))
    }

    /// 自前のバーを使うとき（`menubar = custom`）は、ネイティブのバーを外す。戻すときは付け直す
    pub(super) fn sync_menu_bar(&mut self) {
        // 標準のバーが既定（ダークテーマでは、WM_UAHDRAWMENU で暗く塗る）。`menubar = custom` のときだけ自前
        let want = !self.settings.native_menu;
        if want == self.custom_menu {
            return;
        }
        self.custom_menu = want;
        self.menu_open = None;
        unsafe {
            let _ = SetMenu(self.hwnd, if want { None } else { Some(self.menus.bar) });
        }
        self.layout_cache.borrow_mut().clear();
        self.update_scrollbars();
        self.invalidate();
    }

    /// Alt を押しながら押したキーに対応する項目
    pub(super) fn bar_mnemonic_key(&self, wparam: WPARAM) -> Option<usize> {
        // 英字は仮想キーコード（'A'〜'Z'）も、文字コードも、同じ値
        char::from_u32(wparam.0 as u32).and_then(|c| self.bar_mnemonic(c))
    }

    /// Alt+文字：その項目を開く
    pub(super) fn bar_mnemonic(&self, c: char) -> Option<usize> {
        self.bar_items().iter().position(|b| b.mnemonic.is_some_and(|m| m.eq_ignore_ascii_case(&c)))
    }
}
