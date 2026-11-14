//! 本体の状態を借りていない場所で行う操作（ダイアログ、ポップアップ、確認など）。

use super::*;

pub(super) unsafe fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::SaveAs => {
            let current = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.to_path_buf()))).flatten();
            match unsafe { edit::save_dialog(hwnd, current) } {
                Some(path) => {
                    if let Some(Some(d)) = with_app(|app| app.save(Some(path))) {
                        unsafe { run_deferred(hwnd, d) };
                    }
                }
                // 名前を付けずにやめたら、保留していた操作も取りやめる
                None => {
                    with_app(|app| {
                        app.convert = None;
                        app.pending = None;
                        app.clear_discard();
                    });
                }
            }
        }
        Deferred::ShowFind(tab) => unsafe { find_dialog::show(hwnd, tab) },
        Deferred::ShowSettings => unsafe { settings_dialog::show(hwnd) },
        Deferred::ColMenu => unsafe {
            if let Some(Some(menu)) = with_app(|app| app.build_col_menu()) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, hwnd, None);
                let _ = DestroyMenu(menu);
            }
        },
        Deferred::ContextMenu => unsafe {
            if let Some(Ok(menu)) = with_app(|app| menu::build_context(&app.keymap)) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, hwnd, None);
                let _ = DestroyMenu(menu);
            }
        },
        Deferred::TrayMenu(right) => unsafe {
            if let Some(Some(menu)) = with_app(|app| app.build_tray_menu(right)) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                // メニューの外を押したときに閉じるよう、先にウィンドウを前面にする
                let _ = SetForegroundWindow(hwnd);
                let _ = TrackPopupMenu(menu, TPM_RIGHTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                let _ = DestroyMenu(menu);
            }
        },
        Deferred::ShowKeys => unsafe { keys_dialog::show(hwnd) },
        Deferred::DiffMenu => unsafe {
            if let Some(Some(menu)) = with_app(|app| app.build_diff_menu()) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, hwnd, None);
                let _ = DestroyMenu(menu);
            }
        },
        Deferred::StatusMenu(kind) => unsafe {
            if let Some(Some(menu)) = with_app(|app| app.build_status_menu(kind)) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
                let _ = DestroyMenu(menu);
            }
        },
        Deferred::ConfirmConvertHere(id) => {
            let question = with_app(|app| app.convert_here_question(id)).flatten();
            let go = match question {
                Some(q) => {
                    let text = wide(&q);
                    unsafe { MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), w!("kazakiri"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES }
                }
                None => true,
            };
            if go {
                if let Some(Some(d)) = with_app(|app| app.convert_here(id)) {
                    unsafe { run_deferred(hwnd, d) };
                }
            }
        }
        Deferred::SaveResults => {
            let suggested = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.with_extension("results.txt")))).flatten();
            if let Some(out) = unsafe { edit::save_dialog(hwnd, suggested.or_else(|| Some(PathBuf::from("results.txt")))) } {
                with_app(|app| app.save_results(out));
            }
        }
        Deferred::ExternalSort(desc) => {
            let current = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.to_path_buf()))).flatten();
            let suggested = current.map(|p| {
                p.with_file_name(format!(
                    "{}.sorted.{}",
                    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into()),
                    p.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "csv".into())
                ))
            });
            if let Some(out) = unsafe { edit::save_dialog(hwnd, suggested) } {
                with_app(|app| app.start_external_sort(out, desc));
            }
        }
        Deferred::Join => {
            let paths = unsafe { open_dialog(hwnd) };
            if paths.len() < 2 {
                with_app(|app| {
                    app.message = "結合するファイルを 2 つ以上選んでください（Ctrl や Shift を押しながら）".into();
                    app.invalidate();
                });
                return;
            }
            let suggested = paths[0].with_file_name(format!(
                "{}.joined.{}",
                paths[0].file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into()),
                paths[0].extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "txt".into())
            ));
            if let Some(out) = unsafe { edit::save_dialog(hwnd, Some(suggested)) } {
                with_app(|app| app.start_join(paths, out));
            }
        }
        Deferred::ConfirmGrepReplace(question) => {
            let text = wide(&question);
            let r = unsafe { MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), w!("GREP 置換"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) };
            if r == IDYES {
                with_app(|app| app.run_grep_replace());
            }
        }
        Deferred::Confirm(action) => {
            let name = with_app(|app| app.doc.as_ref().and_then(|d| d.path().map(|p| p.display().to_string()))).flatten();
            let text = wide(&format!("{} への変更を保存しますか？", name.unwrap_or_else(|| "（無題）".into())));
            let r = unsafe { MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), w!("kazakiri"), MB_YESNOCANCEL | MB_ICONQUESTION) };
            if r == IDYES {
                let next = with_app(|app| {
                    app.pending = Some(action);
                    app.save(None)
                });
                if let Some(Some(d)) = next {
                    unsafe { run_deferred(hwnd, d) };
                }
            } else if r == IDNO {
                unsafe { run_deferred(hwnd, Deferred::Perform(action)) };
            } else {
                // キャンセル：終了の手続きも取りやめる
                with_app(|app| app.clear_discard());
            }
        }
        Deferred::Perform(action) => match action {
            Pending::OpenDialog => {
                let paths = unsafe { open_dialog(hwnd) };
                with_app(|app| {
                    for p in paths {
                        app.open(p, None);
                    }
                });
            }
            Pending::OpenPath(p) => {
                with_app(|app| app.open(p, None));
            }
            Pending::OpenAt(p, line, s, e) => {
                with_app(|app| {
                    app.open(p, None);
                    app.pending_goto = Some((line, s, e));
                    app.apply_pending_goto();
                });
            }
            Pending::Reopen(enc) => {
                with_app(|app| {
                    app.reopen(enc);
                    let name = menu::ENCODINGS.iter().find(|(_, e)| *e == enc).map_or("", |(n, _)| n);
                    app.message = format!("{name} で開き直しました");
                });
            }
            Pending::CloseTab => {
                with_app(|app| app.close_active_tab());
            }
            // すべてのタブを閉じる（未保存なら、タブごとに確認する）
            Pending::CloseAll => {
                let more = with_app(|app| {
                    app.close_active_tab();
                    app.tabs.len() > 1 || !app.is_blank()
                })
                .unwrap_or(false);
                if more {
                    if let Some(Some(next)) = with_app(|app| app.guarded(Pending::CloseAll)) {
                        unsafe { run_deferred(hwnd, next) };
                    }
                }
            }
            // 未保存のタブを順に保存してから、すべて閉じる
            Pending::SaveCloseAll => {
                let next = with_app(|app| {
                    let i = (0..app.tabs.len()).find(|&i| app.tab_modified(i))?;
                    app.switch_to(i);
                    app.pending = Some(Pending::SaveCloseAll);
                    Some(app.save(None))
                })
                .flatten();
                match next {
                    Some(d) => {
                        if let Some(d) = d {
                            unsafe { run_deferred(hwnd, d) };
                        }
                    }
                    None => unsafe { run_deferred(hwnd, Deferred::Perform(Pending::CloseAll)) },
                }
            }
            // 未保存のタブが残っていれば、そのタブを表示して確認する。なければ終了
            Pending::Close => match with_app(|app| app.continue_close()) {
                Some(Some(next)) => unsafe { run_deferred(hwnd, next) },
                _ => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
            },
        },
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// 「開く」ダイアログ。複数のファイルを選べる
pub(super) unsafe fn open_dialog(hwnd: HWND) -> Vec<PathBuf> {
    let initial = with_app(|app| app.open_dir.take()).flatten().map(|d| wide(&d.display().to_string()));
    let mut buf = vec![0u16; 65536];
    let filter = wide("テキストファイル\0*.txt;*.log;*.csv;*.tsv\0すべてのファイル\0*.*\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 2,
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrInitialDir: initial.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_ALLOWMULTISELECT,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return Vec::new();
    }
    parse_multi_select(&buf)
}

/// 複数選択の結果を読む。1 つだけなら「フルパス\0\0」、複数なら「フォルダ\0名前1\0名前2\0\0」
pub(super) fn parse_multi_select(buf: &[u16]) -> Vec<PathBuf> {
    let mut parts = buf.split(|&c| c == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy);
    let Some(first) = parts.next() else { return Vec::new() };
    let rest: Vec<String> = parts.collect();
    if rest.is_empty() {
        vec![PathBuf::from(first)]
    } else {
        rest.into_iter().map(|name| PathBuf::from(&first).join(name)).collect()
    }
}
