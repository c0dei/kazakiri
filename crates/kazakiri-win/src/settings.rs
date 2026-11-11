//! 設定（%APPDATA%\kazakiri\settings.ini）。
//!
//! `キー = 値` の行だけの簡単な形式。知らないキーは無視し、壊れていたら既定値を使う。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// フォント名（空なら BIZ UDゴシック → MS ゴシック → Consolas の順に探す）
    pub font: String,
    pub font_size: f32,
    pub dark: bool,
    /// 空白・タブ・改行を表示する
    pub whitespace: bool,
    /// ERROR / WARN などに色を付ける
    pub log_colors: bool,
    /// タブの幅（半角の文字数）
    pub tab_width: usize,
    /// 長い行を画面の幅で折り返して表示する
    pub wrap: bool,
    /// GREP 置換の前に、各ファイルのバックアップ（.bak）を作る
    pub grep_backup: bool,
    /// IME の変換中の文字を、IME の窓ではなくエディタの中に描く
    pub ime_inline: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { font: String::new(), font_size: 14.0, dark: false, whitespace: false, log_colors: true, tab_width: 4, wrap: false, grep_backup: true, ime_inline: true }
    }
}

pub const MIN_FONT: f32 = 6.0;
pub const MAX_FONT: f32 = 72.0;

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("kazakiri").join("settings.ini"))
}

impl Settings {
    pub fn load() -> Settings {
        let text = path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        Settings::parse(&text)
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let flag = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
            match k {
                "font" => s.font = v.to_string(),
                "font_size" => {
                    if let Ok(x) = v.parse::<f32>() {
                        s.font_size = x.clamp(MIN_FONT, MAX_FONT);
                    }
                }
                "dark" => s.dark = flag(v),
                "whitespace" => s.whitespace = flag(v),
                "log_colors" => s.log_colors = flag(v),
                "wrap" => s.wrap = flag(v),
                "grep_backup" => s.grep_backup = flag(v),
                "ime_inline" => s.ime_inline = flag(v),
                "tab_width" => {
                    if let Ok(x) = v.parse::<usize>() {
                        s.tab_width = x.clamp(1, 16);
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "# kazakiri の設定（アプリから変更すると上書きされます）\n\
             font = {}\nfont_size = {}\ndark = {}\nwhitespace = {}\nlog_colors = {}\ntab_width = {}\nwrap = {}\ngrep_backup = {}\nime_inline = {}\n",
            self.font, self.font_size, self.dark, self.whitespace, self.log_colors, self.tab_width, self.wrap, self.grep_backup, self.ime_inline
        )
    }

    /// 保存する（失敗は無視：使い勝手に関わるだけなので）
    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, self.to_text());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let s = Settings { font: "Consolas".into(), font_size: 16.5, dark: true, whitespace: true, log_colors: false, tab_width: 8, wrap: true, grep_backup: false, ime_inline: false };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let broken = Settings::parse("font_size = abc\nunknown = 1\ntab_width = 99\n");
        assert_eq!(broken.font_size, 14.0);
        assert_eq!(broken.tab_width, 16);
    }
}
