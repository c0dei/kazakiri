//! 自動保存。一定の間隔で、未保存の編集のあるファイルを `名前.autosave` に書き出す（元のファイルは書き換えない）。
//! 保存すると、通常どおり閉じると、消す。異常終了したときだけ残るので、次に開くときに知らせる。
//! 書き出しは別スレッドで行い、編集の履歴（元に戻す）には触れない。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kazakiri_core::backup;

use super::*;

impl App {
    fn autosave_dir(&self) -> Option<PathBuf> {
        let d = self.settings.backup_dir.trim();
        (!d.is_empty()).then(|| PathBuf::from(d))
    }

    /// 設定に合わせて、自動保存のタイマーを張り直す
    pub(super) fn restart_autosave_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_AUTOSAVE);
            if self.settings.autosave_min > 0 {
                SetTimer(Some(self.hwnd), TIMER_AUTOSAVE, (self.settings.autosave_min as u32).saturating_mul(60_000), None);
            }
        }
    }

    pub(super) fn autosave_tick(&mut self) {
        if self.settings.autosave_min == 0 || self.autosave_busy.load(Ordering::Relaxed) {
            return;
        }
        let dir = self.autosave_dir();
        let todo: Vec<_> = self.modified_docs().into_iter().filter(|(_, p, v)| self.autosave_done.get(p) != Some(v)).collect();
        if todo.is_empty() {
            return;
        }
        for (_, p, v) in &todo {
            self.autosave_done.insert(p.clone(), *v);
        }
        self.autosave_busy.store(true, Ordering::Relaxed);
        let busy = self.autosave_busy.clone();
        std::thread::spawn(move || {
            for (doc, path, _) in todo {
                let dest = backup::autosave_path(&path, dir.as_deref());
                if let Some(d) = &dir {
                    let _ = std::fs::create_dir_all(d);
                }
                let (written, cancel) = (AtomicU64::new(0), AtomicBool::new(false));
                if let Ok(temp) = doc.write_temp_with(&dest, &written, &cancel, None) {
                    let _ = kazakiri_core::commit_save(&temp, &dest);
                }
            }
            busy.store(false, Ordering::Relaxed);
        });
    }

    /// そのファイルの自動保存のファイルを消す（どちらの置き場所のものも）
    pub(super) fn remove_autosave(&mut self, path: &Path) {
        self.autosave_done.remove(path);
        let _ = std::fs::remove_file(backup::autosave_path(path, None));
        if let Some(d) = self.autosave_dir() {
            let _ = std::fs::remove_file(backup::autosave_path(path, Some(&d)));
        }
    }

    /// 終了するとき：開いているファイルの自動保存のファイルを消す
    pub(super) fn remove_all_autosaves(&mut self) {
        for p in self.tab_paths() {
            self.remove_autosave(&p);
        }
        if let Some(p) = self.doc.as_ref().and_then(|d| d.path()).map(Path::to_path_buf) {
            self.remove_autosave(&p);
        }
    }

    /// ファイルを開いたとき、元のファイルより新しい自動保存があれば知らせる（異常終了で残ったもの）
    pub(super) fn check_autosave(&mut self, path: &Path) {
        let mut found = vec![backup::autosave_path(path, None)];
        if let Some(d) = self.autosave_dir() {
            found.push(backup::autosave_path(path, Some(&d)));
        }
        let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let orig = mtime(path);
        if let Some(a) = found.into_iter().find(|a| mtime(a).is_some_and(|t| orig.is_none_or(|o| t > o))) {
            self.message = format!("未保存の編集が自動保存されています: {}（開いて確かめてください）", a.display());
        }
    }
}
