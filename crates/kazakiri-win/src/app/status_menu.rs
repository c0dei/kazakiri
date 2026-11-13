//! ステータスバーの「文字コード」「改行コード」をクリックしたときのメニュー（秀丸のように、いまのファイルを変える）。
//!
//! - 文字コード：「指定して開き直す」（読み方を変える）と「変換して保存」（このファイルを書き換える）
//! - 改行コード：「CRLF / LF / CR にして保存」（このファイルを書き換える）
//!
//! 書き換える操作は、元に戻せないので確認してから行う。

use super::*;

impl App {
    /// ステータスバーの x の位置にあるもの（0 = 文字コード、1 = 改行コード）
    pub(super) fn status_hit(&self, x: f32) -> Option<u8> {
        let (head, enc, nl, tail) = self.status_parts()?;
        let w = self.client_size().0;
        let width = |s: &str| unsafe { self.text_width(&s.encode_utf16().collect::<Vec<_>>()) }.unwrap_or(0.0);
        let rw = width(&format!("{head}{enc}   {nl}   {tail}")).min(w * 0.75);
        let e0 = w - rw - 8.0 + width(&head);
        let e1 = e0 + width(&enc);
        let n0 = e1 + width("   ");
        let n1 = n0 + width(&nl);
        if (e0..e1).contains(&x) {
            Some(0)
        } else if (n0..n1).contains(&x) {
            Some(1)
        } else {
            None
        }
    }

    /// クリックしたときのポップアップメニュー（使い終わったら呼び出し側が DestroyMenu する）
    pub(super) unsafe fn build_status_menu(&self, kind: u8) -> Option<HMENU> {
        let doc = self.doc.as_ref()?;
        let text = |s: &str| wide(s);
        unsafe {
            let add = |menu: HMENU, id: u32, label: &str, checked: bool| {
                let w = text(label);
                let _ = AppendMenuW(menu, MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED }, id as usize, PCWSTR(w.as_ptr()));
            };
            let sub = |menu: HMENU, child: HMENU, label: &str| {
                let w = text(label);
                let _ = AppendMenuW(menu, MF_POPUP, child.0 as usize, PCWSTR(w.as_ptr()));
            };
            let m = CreatePopupMenu().ok()?;
            if kind == 0 {
                let cur = doc.encoding();
                let w = text(&format!("現在: {}", cur.name()));
                let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 0, PCWSTR(w.as_ptr()));
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                let open = CreatePopupMenu().ok()?;
                for (i, (name, enc)) in menu::ENCODINGS.iter().enumerate() {
                    add(open, menu::ID_ENC_BASE + i as u32, name, doc.forced_encoding() == *enc);
                }
                sub(m, open, "指定して開き直す（読み方を変える）");
                let conv = CreatePopupMenu().ok()?;
                for (i, (name, enc, bom)) in menu::CONV_ENCODINGS.iter().enumerate() {
                    add(conv, menu::ID_HERE_ENC_BASE + i as u32, name, cur.encoding == *enc && (cur.bom_len > 0) == *bom);
                }
                sub(m, conv, "変換して保存（このファイルを書き換える）");
            } else {
                let cur = doc.newline_name();
                for (i, (name, _)) in menu::CONV_NEWLINES.iter().enumerate() {
                    add(m, menu::ID_HERE_NL_BASE + i as u32, &format!("{name} にして保存"), name.starts_with(cur) && name.as_bytes().get(cur.len()) == Some(&b' '));
                }
            }
            Some(m)
        }
    }

    /// 書き換える前の確認の文面（保存先のないファイルでは None＝確認なしで保存先を選ぶ）
    pub(super) fn convert_here_question(&self, id: u32) -> Option<String> {
        let path = self.doc.as_ref()?.path()?.display().to_string();
        let what = if let Some(&(n, _, _)) = id.checked_sub(menu::ID_HERE_ENC_BASE).and_then(|i| menu::CONV_ENCODINGS.get(i as usize)) {
            format!("文字コードを {n}")
        } else {
            let &(n, _) = id.checked_sub(menu::ID_HERE_NL_BASE).and_then(|i| menu::CONV_NEWLINES.get(i as usize))?;
            format!("改行コードを {n}")
        };
        Some(format!("{path}\n\n{what} にして、このファイルを上書き保存します。元には戻せません。よろしいですか？"))
    }

    /// このファイルを、文字コード（改行コード）を変えて上書き保存する
    pub(super) fn convert_here(&mut self, id: u32) -> Option<Deferred> {
        let cid = if id >= menu::ID_HERE_NL_BASE { id - menu::ID_HERE_NL_BASE + menu::ID_CONV_NL_BASE } else { id - menu::ID_HERE_ENC_BASE + menu::ID_CONV_ENC_BASE };
        self.start_convert_save(cid)?;
        self.save(None)
    }
}
