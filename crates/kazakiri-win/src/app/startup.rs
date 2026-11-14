//! 起動：ウィンドウクラスの登録・ウィンドウと描画の準備・メッセージループ。

use super::*;

pub(super) unsafe fn run_inner() -> std::result::Result<(), (&'static str, windows::core::Error)> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| ("GetModuleHandle", e))?;
        let class = w!("KazakiriMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_IBEAM).map_err(|e| ("LoadCursor", e))?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let keymap = keymap::Keymap::load();
        let menus = menu::build(&keymap).map_err(|e| ("CreateMenu", e))?;
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("kazakiri"),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_HSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
            None,
            Some(menus.bar),
            Some(instance.into()),
            None,
        )
        .map_err(|e| ("CreateWindow", e))?;

        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(|e| ("D2D1CreateFactory", e))?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| ("DWriteCreateFactory", e))?;
        let settings = Settings::load();
        let format = create_format(&dw, &settings.font, settings.font_size).map_err(|e| ("CreateTextFormat", e))?;
        // フォントが見つからない環境でも起動できるよう、計測失敗時は既定値
        let fs = settings.font_size;
        let (char_w, line_h) = measure(&dw, &format).unwrap_or((fs * 0.5, (fs * 1.4).ceil()));
        let _ = format.SetIncrementalTabStop(char_w * settings.tab_width as f32);
        let font_prop = is_proportional(&dw, &format);

        let ui_format = create_ui_format(&dw, &settings.ui_font, settings.ui_font_size).map_err(|e| ("CreateTextFormat", e))?;
        let ufs = settings.ui_font_size;
        let (ui_char_w, ui_line_h) = measure(&dw, &ui_format).unwrap_or((ufs * 0.5, (ufs * 1.4).ceil()));

        theme::load_accent();
        let mut app = App {
            hwnd,
            d2d,
            dw,
            ui_format,
            ui_line_h,
            ui_char_w,
            ui_width_cache: RefCell::new(HashMap::new()),
            font_prop,
            font_overridden: false,
            format,
            gfx: None,
            menus,
            keymap,
            bookmarks: Default::default(),
            bm_lines: 0,
            bm_col0: false,
            hover: None,
            filter_pending: false,
            last_message: String::new(),
            open_dir: None,
            diff_ign_edge: false,
            diff_ign_case: false,
            split_sync: false,
            csv_letters: false,
            cell_mode: false,
            tip: None,
            dabbrev: None,
            markers: Vec::new(),
            diffview: None,
            md: None,
            md_hover: false,
            autosave_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            autosave_done: HashMap::new(),
            tracking_leave: false,
            line_h,
            char_w,
            doc: None,
            editor: None,
            goal_x: None,
            high_surrogate: None,
            saving: None,
            convert: None,
            pending: None,
            results: None,
            rjob: None,
            last_replace: String::new(),
            grep_folder: String::new(),
            grep_files: results::GREP_DEFAULT_FILES.into(),
            pending_goto: None,
            csv: None,
            theme: if settings.dark { &theme::DARK } else { &theme::LIGHT },
            settings,
            tabs: vec![tabs::Tab::default()],
            active: 0,
            tab_scroll: 0.0,
            discard: false,
            top: 0,
            x_off: 0.0,
            prompt: None,
            input: String::new(),
            input2: String::new(),
            prompt_focus: 0,
            last_find: String::new(),
            last_filter: String::new(),
            history: crate::history::History::load(),
            replace_scope: None,
            dlg_scope: None,
            grep_recursive: true,
            grep_enc: 0,
            grep_max_on: false,
            grep_max: 10000,
            grep_output: 0,
            grep_extract: false,
            comp: None,
            csv_cont: None,
            want_dialog: None,
            want_sort: None,
            opts: SearchOptions { case_sensitive: false, regex: false, whole_word: false },
            search: None,
            next_job: 1,
            hit: None,
            sel: None,
            dragging: false,
            overwrite: false,
            rect: None,
            multi: None,
            alt_used: false,
            tab_drag: None,
            message: "Ctrl+O: 開く / Ctrl+N: 新規 / ドラッグ&ドロップでも開けます".into(),
            opened_at: None,
            recent: recent::load(),
            follow: false,
            stick_bottom: false,
            layout_cache: RefCell::new(HashMap::new()),
        };
        // 引数のファイルをそれぞれタブで開く
        app.new_doc();
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.is_empty() {
            app.restore_session();
        }
        for arg in args {
            app.open(PathBuf::from(arg), None);
        }
        if let Some(first) = app.keymap.problems.first() {
            app.message = format!("keys.ini: {first}（ほか {} 件）", app.keymap.problems.len() - 1);
        }
        // 描画の確認用（IME のない環境で、変換中の表示を見る）：KAZAKIRI_TEST_COMP=文字列
        if let Some(t) = std::env::var_os("KAZAKIRI_TEST_COMP") {
            let units: Vec<u16> = t.to_string_lossy().encode_utf16().collect();
            let attrs = (0..units.len()).map(|i| if i < units.len() / 2 { 1 } else { 0 }).collect();
            app.comp = Some(edit::Comp { cursor: units.len() / 2, attrs, units });
        }
        // 前回のウィンドウの位置と大きさを戻す（どのモニターにも掛からない位置なら、既定の位置のまま）
        if let Some((x, y, w, h, maximized)) = app.settings.window {
            let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
            if !MonitorFromRect(&r, MONITOR_DEFAULTTONULL).is_invalid() {
                let wp = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    showCmd: if maximized { SW_SHOWMAXIMIZED.0 as u32 } else { SW_SHOWNORMAL.0 as u32 },
                    rcNormalPosition: r,
                    ..Default::default()
                };
                let _ = SetWindowPlacement(hwnd, &wp);
            }
        }
        app.apply_title_theme();
        if app.settings.tray {
            app.tray_add();
        }
        app.restart_autosave_timer();
        APP.with(|a| *a.borrow_mut() = Some(app));
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // 検索ダイアログ：Tab での移動・Enter・Esc を任せる
            let dlg = find_dialog::FIND_DLG.load(Ordering::Relaxed);
            if dlg != 0 && find_dialog::accelerator(hwnd, &msg) {
                continue;
            }
            if dlg != 0 && IsDialogMessageW(HWND(dlg as *mut _), &msg).as_bool() {
                continue;
            }
            let sdlg = settings_dialog::SET_DLG.load(Ordering::Relaxed);
            if sdlg != 0 && IsDialogMessageW(HWND(sdlg as *mut _), &msg).as_bool() {
                continue;
            }
            let kdlg = keys_dialog::KEYS_DLG.load(Ordering::Relaxed);
            if kdlg != 0 && IsDialogMessageW(HWND(kdlg as *mut _), &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().take() {
                app.cancel_search();
            }
        });
        Ok(())
    }
}
