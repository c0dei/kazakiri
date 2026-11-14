//! 設定（%APPDATA%\kazakiri\settings.ini）。
//!
//! `キー = 値` の行だけの簡単な形式。知らないキーは無視し、壊れていたら既定値を使う。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// フォント名（空なら BIZ UDゴシック → MS ゴシック → Consolas の順に探す）
    pub font: String,
    pub font_size: f32,
    /// メニュー・タブ・ツールバー・ステータスバーなど、画面の部品のフォント（空なら Yu Gothic UI → Meiryo UI → Segoe UI）
    pub ui_font: String,
    pub ui_font_size: f32,
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
    /// 拡張子で決まる言語（JSON・SQL・XML など）の色分け
    pub syntax: bool,
    /// ツールバーと機能ボタンの列
    /// 通知領域（タスクトレイ）にアイコンを出す（クリックで新規作成・開くなどのメニュー）
    pub tray: bool,
    /// GREP を始めたら、ダイアログを閉じる
    pub grep_close: bool,
    /// マウスホイール 1 回で動かす行数
    pub wheel_lines: usize,
    /// Markdown ファイルを開いたとき、右にプレビューを並べる
    pub md_auto: bool,
    /// 上書き保存するとき、元のファイルをバックアップする
    pub backup_save: bool,
    /// バックアップを置くフォルダ（空なら、元のファイルと同じフォルダ）
    pub backup_dir: String,
    /// 古いバックアップを残す（.bak2、.bak3 …）。オフなら .bak を上書き
    pub backup_numbered: bool,
    /// 自動保存の間隔（分）。0 なら自動保存しない
    pub autosave_min: usize,
    pub toolbar: bool,
    /// ツールバーの下の、CSV の区切りなどのボタンの行
    pub modebar: bool,
    pub fkeys: bool,
    /// 前回のウィンドウの位置と大きさ（x, y, 幅, 高さ, 最大化）。終了時に保存して、次の起動で戻す
    pub window: Option<(i32, i32, i32, i32, bool)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { font: String::new(), font_size: 14.0, ui_font: String::new(), ui_font_size: 13.0, dark: false, whitespace: false, log_colors: true, tab_width: 4, wrap: false, grep_backup: true, ime_inline: true, syntax: true, tray: true, grep_close: false, wheel_lines: 3, md_auto: false, backup_save: false, backup_dir: String::new(), backup_numbered: false, autosave_min: 0, toolbar: true, modebar: true, fkeys: false, window: None }
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
                "ui_font" => s.ui_font = v.to_string(),
                "ui_font_size" => {
                    if let Ok(x) = v.parse::<f32>() {
                        s.ui_font_size = x.clamp(MIN_FONT, 40.0);
                    }
                }
                "dark" => s.dark = flag(v),
                "whitespace" => s.whitespace = flag(v),
                "log_colors" => s.log_colors = flag(v),
                "wrap" => s.wrap = flag(v),
                "grep_backup" => s.grep_backup = flag(v),
                "ime_inline" => s.ime_inline = flag(v),
                "syntax" => s.syntax = flag(v),
                // 旧キー menubar / native_dark_menu は、標準のメニューバーだけになったので無視する
                "window" => {
                    let n: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if n.len() == 5 && n[2] >= 200 && n[3] >= 150 {
                        s.window = Some((n[0], n[1], n[2], n[3], n[4] != 0));
                    }
                }
                "tray" => s.tray = flag(v),
                "grep_close" => s.grep_close = flag(v),
                "wheel_lines" => s.wheel_lines = v.parse::<usize>().map_or(3, |n| n.clamp(1, 30)),
                "md_auto" => s.md_auto = flag(v),
                "backup_save" => s.backup_save = flag(v),
                "backup_dir" => s.backup_dir = v.to_string(),
                "backup_numbered" => s.backup_numbered = flag(v),
                "autosave_min" => s.autosave_min = v.parse::<usize>().map_or(0, |n| n.min(1440)),
                "toolbar" => s.toolbar = flag(v),
                "modebar" => s.modebar = flag(v),
                "fkeys" => s.fkeys = flag(v),
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
             font = {}\nfont_size = {}\nui_font = {}\nui_font_size = {}\ndark = {}\nwhitespace = {}\nlog_colors = {}\ntab_width = {}\nwrap = {}\ngrep_backup = {}\nime_inline = {}\nsyntax = {}\ntray = {}\ngrep_close = {}\nwheel_lines = {}\nmd_auto = {}\nbackup_save = {}\nbackup_dir = {}\nbackup_numbered = {}\nautosave_min = {}\ntoolbar = {}\nmodebar = {}\nfkeys = {}\n{}",
            self.font, self.font_size, self.ui_font, self.ui_font_size, self.dark, self.whitespace, self.log_colors, self.tab_width, self.wrap, self.grep_backup, self.ime_inline, self.syntax, self.tray, self.grep_close, self.wheel_lines, self.md_auto, self.backup_save, self.backup_dir, self.backup_numbered, self.autosave_min, self.toolbar, self.modebar,
            self.fkeys,
            self.window.map_or(String::new(), |(x, y, w, h, m)| format!("window = {x}, {y}, {w}, {h}, {}\n", i32::from(m)))
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
        let s = Settings { font: "Consolas".into(), font_size: 16.5, ui_font: "Meiryo UI".into(), ui_font_size: 12.0, dark: true, whitespace: true, log_colors: false, tab_width: 8, wrap: true, grep_backup: false, ime_inline: false, syntax: false, tray: false, grep_close: true, wheel_lines: 5, md_auto: true, backup_save: true, backup_dir: "D:\\bk".into(), backup_numbered: true, autosave_min: 3, toolbar: false, modebar: false, fkeys: false, window: Some((10, 20, 900, 700, true)) };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let broken = Settings::parse("font_size = abc\nunknown = 1\ntab_width = 99\n");
        assert_eq!(broken.font_size, 14.0);
        assert_eq!(broken.tab_width, 16);
    }
}
