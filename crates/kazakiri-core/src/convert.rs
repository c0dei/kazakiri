//! 文字コード・改行コードを変えて書き出す（「文字コードを変えて保存」）。
//!
//! 元のバイト列を文字コードの境界を意識しながら順に読み（ストリーム処理）、改行を変え、
//! 新しい文字コードにして書く。巨大なファイルでもメモリは一定。
//! 新しい文字コードで表せない文字があれば、置き換えずにエラーにする（黙って `&#...;` にしない）。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use encoding_rs::{Encoding, EncoderResult, CoderResult, UTF_16BE, UTF_16LE, UTF_8};

use crate::document::Document;

/// 書き出しの設定
#[derive(Clone, Copy, Debug)]
pub struct Convert {
    pub encoding: &'static Encoding,
    /// BOM を付ける
    pub bom: bool,
    /// 改行コード（`"\r\n"`・`"\n"`・`"\r"`）。None なら元のまま（ただし統一はしない＝そのまま通す）
    pub newline: Option<&'static str>,
}

const BLOCK: usize = 4 << 20;

impl Document {
    /// 変換しながら書き出す
    pub fn write_converted(&self, w: &mut dyn Write, written: &AtomicU64, cancel: &AtomicBool, conv: &Convert) -> io::Result<()> {
        self.write_converted_with(w, written, cancel, conv, BLOCK)
    }

    fn write_converted_with(&self, w: &mut dyn Write, written: &AtomicU64, cancel: &AtomicBool, conv: &Convert, block: usize) -> io::Result<()> {
        let det = self.encoding();
        let mut st = State {
            dec: det.encoding.new_decoder_without_bom_handling(),
            enc: (conv.encoding != UTF_16LE && conv.encoding != UTF_16BE).then(|| conv.encoding.new_encoder()),
            out: Writer { w, written, conv: *conv, buf: Vec::new() },
            pending_cr: false,
            text: String::new(),
            skip: det.bom_len,
        };
        if conv.bom {
            st.out.raw(match conv.encoding {
                e if e == UTF_16LE => &[0xFF, 0xFE][..],
                e if e == UTF_16BE => &[0xFE, 0xFF][..],
                e if e == UTF_8 => &[0xEF, 0xBB, 0xBF][..],
                _ => &[][..],
            })?;
        }
        self.for_each_segment(&mut |seg| st.feed(seg, block, cancel))?;
        // 終端：デコーダに残りを吐かせる
        st.text.clear();
        st.text.reserve(16);
        let _ = st.dec.decode_to_string(&[], &mut st.text, true);
        let text = std::mem::take(&mut st.text);
        finish(&mut st.out, &mut st.enc, &text, &mut st.pending_cr, true)
    }
}

struct State<'a> {
    dec: encoding_rs::Decoder,
    enc: Option<encoding_rs::Encoder>,
    out: Writer<'a>,
    pending_cr: bool,
    text: String,
    /// まだ読み飛ばす BOM のバイト数
    skip: usize,
}

impl State<'_> {
    fn feed(&mut self, seg: &[u8], block: usize, cancel: &AtomicBool) -> io::Result<()> {
        let n = self.skip.min(seg.len());
        self.skip -= n;
        for chunk in seg[n..].chunks(block) {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "保存を中止しました"));
            }
            let mut rest = chunk;
            loop {
                self.text.clear();
                self.text.reserve(self.dec.max_utf8_buffer_length(rest.len()).unwrap_or(rest.len() * 3 + 16));
                let (res, read, _) = self.dec.decode_to_string(rest, &mut self.text, false);
                let text = std::mem::take(&mut self.text);
                finish(&mut self.out, &mut self.enc, &text, &mut self.pending_cr, false)?;
                self.text = text;
                rest = &rest[read..];
                if res == CoderResult::InputEmpty {
                    break;
                }
            }
        }
        Ok(())
    }
}

struct Writer<'a> {
    w: &'a mut dyn Write,
    written: &'a AtomicU64,
    conv: Convert,
    buf: Vec<u8>,
}

impl Writer<'_> {
    fn raw(&mut self, b: &[u8]) -> io::Result<()> {
        self.w.write_all(b)?;
        self.written.fetch_add(b.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

/// 改行を変え、新しい文字コードにして書く
fn finish(out: &mut Writer, enc: &mut Option<encoding_rs::Encoder>, text: &str, pending_cr: &mut bool, last: bool) -> io::Result<()> {
    let nl = out.conv.newline;
    let converted;
    let s: &str = match nl {
        None => text,
        Some(nl) => {
            let mut t = String::with_capacity(text.len() + 16);
            for c in text.chars() {
                if *pending_cr {
                    *pending_cr = false;
                    t.push_str(nl);
                    if c == '\n' {
                        continue;
                    }
                }
                match c {
                    '\r' => *pending_cr = true,
                    '\n' => t.push_str(nl),
                    c => t.push(c),
                }
            }
            if last && *pending_cr {
                *pending_cr = false;
                t.push_str(nl);
            }
            converted = t;
            &converted
        }
    };
    match enc {
        None => {
            let le = out.conv.encoding == UTF_16LE;
            let mut bytes = Vec::with_capacity(s.len() * 2);
            for u in s.encode_utf16() {
                bytes.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
            }
            out.raw(&bytes)
        }
        Some(e) => {
            let mut src = s;
            loop {
                out.buf.clear();
                out.buf.reserve(e.max_buffer_length_from_utf8_without_replacement(src.len()).unwrap_or(src.len() * 4 + 16));
                let (res, read) = e.encode_from_utf8_to_vec_without_replacement(src, &mut out.buf, last);
                let buf = std::mem::take(&mut out.buf);
                out.raw(&buf)?;
                out.buf = buf;
                match res {
                    EncoderResult::InputEmpty => return Ok(()),
                    EncoderResult::OutputFull => src = &src[read..],
                    EncoderResult::Unmappable(c) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{} で表せない文字があります: {c}（U+{:04X}）", out.conv.encoding.name(), c as u32),
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::{EUC_JP, SHIFT_JIS};

    fn ready(doc: &Document) {
        while !doc.progress().done {
            std::thread::yield_now();
        }
    }

    fn convert(src: &[u8], forced: Option<&'static Encoding>, conv: Convert, block: usize) -> io::Result<Vec<u8>> {
        let doc = Document::from_bytes_with(src.to_vec(), forced);
        ready(&doc);
        let mut v = Vec::new();
        doc.write_converted_with(&mut v, &AtomicU64::new(0), &AtomicBool::new(false), &conv, block)?;
        Ok(v)
    }

    #[test]
    fn utf8_to_sjis_and_crlf() {
        let conv = Convert { encoding: SHIFT_JIS, bom: false, newline: Some("\r\n") };
        let out = convert("あいう\nabc\n".as_bytes(), None, conv, 4).unwrap();
        let (s, _, _) = SHIFT_JIS.decode(&out);
        assert_eq!(s, "あいう\r\nabc\r\n");
    }

    #[test]
    fn block_boundary_inside_a_character() {
        let src: String = "日本語のテキスト、🙂も。\n".repeat(50);
        let conv = Convert { encoding: UTF_16LE, bom: true, newline: None };
        // 1〜7 バイトごとに区切っても同じ結果
        let expect = convert(src.as_bytes(), None, conv, 1 << 20).unwrap();
        for block in 1..8 {
            assert_eq!(convert(src.as_bytes(), None, conv, block).unwrap(), expect, "block {block}");
        }
        assert_eq!(&expect[..2], &[0xFF, 0xFE]);
        let (s, _, _) = UTF_16LE.decode(&expect);
        assert_eq!(s, src);
    }

    #[test]
    fn newline_variants() {
        let conv = Convert { encoding: UTF_8, bom: false, newline: Some("\n") };
        // CRLF・CR・LF が混ざっていても統一される。ブロック境界で CR と LF が分かれても 1 つの改行
        for block in 1..6 {
            let out = convert(b"a\r\nb\rc\nd\r\n", None, conv, block).unwrap();
            assert_eq!(out, b"a\nb\nc\nd\n", "block {block}");
        }
        let conv = Convert { encoding: UTF_8, bom: false, newline: Some("\r") };
        assert_eq!(convert(b"a\nb\r\n", None, conv, 3).unwrap(), b"a\rb\r");
        // 末尾の CR だけの行
        let conv = Convert { encoding: UTF_8, bom: false, newline: Some("\r\n") };
        assert_eq!(convert(b"a\r", None, conv, 1).unwrap(), b"a\r\n");
    }

    #[test]
    fn from_sjis_with_bom_handling() {
        let (sj, _, _) = SHIFT_JIS.encode("ｱｲｳ漢字\r\n");
        let conv = Convert { encoding: UTF_8, bom: true, newline: None };
        let out = convert(&sj, Some(SHIFT_JIS), conv, 3).unwrap();
        assert_eq!(&out[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(std::str::from_utf8(&out[3..]).unwrap(), "ｱｲｳ漢字\r\n");
        // UTF-8 の BOM は元の文字コードの BOM として取り除かれ、二重にならない
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("x\n".as_bytes());
        let out = convert(&with_bom, None, conv, 2).unwrap();
        assert_eq!(out, b"\xEF\xBB\xBFx\n");
        let conv = Convert { encoding: EUC_JP, bom: false, newline: None };
        assert_eq!(EUC_JP.decode(&convert(&with_bom, None, conv, 2).unwrap()).0, "x\n");
    }

    #[test]
    fn unmappable_is_an_error() {
        let conv = Convert { encoding: SHIFT_JIS, bom: false, newline: None };
        let err = convert("abc🙂def".as_bytes(), None, conv, 1 << 20).unwrap_err();
        assert!(err.to_string().contains("U+1F642"), "{err}");
    }

    #[test]
    fn utf16_source() {
        let src: Vec<u8> = [0xFF, 0xFE].into_iter().chain("あ\r\nい".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        let conv = Convert { encoding: UTF_8, bom: false, newline: Some("\n") };
        assert_eq!(convert(&src, None, conv, 3).unwrap(), "あ\nい".as_bytes());
    }
}
