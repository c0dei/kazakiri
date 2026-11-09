//! 文字コード判定。
//!
//! 1. BOM があればそれを信用する
//! 2. 先頭サンプル（既定 1MB）が UTF-8 として正しければ UTF-8
//! 3. NUL バイトの偏りから BOM なし UTF-16 を推定
//! 4. Shift_JIS / EUC-JP をデコードしてみて、不正バイト数などのスコアで選ぶ

use encoding_rs::{Encoding, EUC_JP, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8};

/// 判定に使う先頭サンプルの大きさ
pub const SAMPLE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub encoding: &'static Encoding,
    /// BOM のバイト数（本文はこの位置から始まる）
    pub bom_len: usize,
}

impl Detected {
    pub fn name(&self) -> String {
        let base = match self.encoding.name() {
            "Shift_JIS" => "Shift_JIS(CP932)",
            n => n,
        };
        if self.bom_len > 0 {
            format!("{base} (BOM)")
        } else {
            base.to_string()
        }
    }

    pub fn is_utf16(&self) -> bool {
        self.encoding == UTF_16LE || self.encoding == UTF_16BE
    }
}

pub fn detect(bytes: &[u8]) -> Detected {
    if let Some((enc, len)) = Encoding::for_bom(bytes) {
        return Detected { encoding: enc, bom_len: len };
    }
    let sample = &bytes[..bytes.len().min(SAMPLE_BYTES)];
    let truncated = sample.len() < bytes.len();

    if let Some(enc) = guess_utf16(sample) {
        return Detected { encoding: enc, bom_len: 0 };
    }
    if is_utf8(sample, truncated) {
        return Detected { encoding: UTF_8, bom_len: 0 };
    }
    let sjis = japanese_score(SHIFT_JIS, sample);
    let euc = japanese_score(EUC_JP, sample);
    let encoding = if euc < sjis { EUC_JP } else { SHIFT_JIS };
    Detected { encoding, bom_len: 0 }
}

fn is_utf8(sample: &[u8], truncated: bool) -> bool {
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        // サンプル末尾で多バイト文字が切れているだけなら UTF-8 とみなす
        Err(e) => truncated && e.error_len().is_none(),
    }
}

fn guess_utf16(sample: &[u8]) -> Option<&'static Encoding> {
    let n = sample.len() / 2 * 2;
    if n < 4 {
        return None;
    }
    let (mut even_zero, mut odd_zero) = (0usize, 0usize);
    for pair in sample[..n].chunks_exact(2) {
        even_zero += (pair[0] == 0) as usize;
        odd_zero += (pair[1] == 0) as usize;
    }
    let units = n / 2;
    // ASCII 中心の UTF-16 なら片側がほぼ 0 になる
    if odd_zero * 10 > units * 4 && even_zero * 20 < units {
        Some(UTF_16LE)
    } else if even_zero * 10 > units * 4 && odd_zero * 20 < units {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// 小さいほど「その文字コードらしい」
fn japanese_score(enc: &'static Encoding, sample: &[u8]) -> usize {
    let (text, _) = enc.decode_without_bom_handling(sample);
    let mut score = 0usize;
    for c in text.chars() {
        match c {
            '\u{FFFD}' => score += 100,
            // EUC-JP を Shift_JIS と誤読すると半角カナが大量に出る
            '\u{FF61}'..='\u{FF9F}' => score += 3,
            // 日本語テキストでまず出ない領域（誤読の兆候）
            '\u{0080}'..='\u{02FF}' => score += 5,
            _ => {}
        }
    }
    score
}

/// 行のバイト列を表示用文字列にデコードする。
pub fn decode<'a>(det: &Detected, bytes: &'a [u8]) -> std::borrow::Cow<'a, str> {
    det.encoding.decode_without_bom_handling(bytes).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom() {
        assert_eq!(detect(b"\xEF\xBB\xBFabc").encoding, UTF_8);
        assert_eq!(detect(b"\xEF\xBB\xBFabc").bom_len, 3);
        assert_eq!(detect(b"\xFF\xFEa\0").encoding, UTF_16LE);
        assert_eq!(detect(b"\xFE\xFF\0a").encoding, UTF_16BE);
    }

    #[test]
    fn utf8_and_ascii() {
        assert_eq!(detect("日本語のテキスト\n".as_bytes()).encoding, UTF_8);
        assert_eq!(detect(b"plain ascii\n").encoding, UTF_8);
    }

    #[test]
    fn sjis_vs_euc() {
        let s = "これは日本語のログです。エラーが発生しました。\r\n".repeat(20);
        let (sj, _, _) = SHIFT_JIS.encode(&s);
        let (eu, _, _) = EUC_JP.encode(&s);
        assert_eq!(detect(&sj).encoding, SHIFT_JIS);
        assert_eq!(detect(&eu).encoding, EUC_JP);
    }

    #[test]
    fn utf16_without_bom() {
        let le: Vec<u8> = "hello world\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let be: Vec<u8> = "hello world\r\n".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(detect(&le).encoding, UTF_16LE);
        assert_eq!(detect(&be).encoding, UTF_16BE);
    }
}
