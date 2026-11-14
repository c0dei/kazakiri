# 引き継ぎメモ（2026-11-15 時点）

## 状態

- ブランチ `main`。**GitHub にプッシュ済みは `10772d8`（v0.13.0 のタグもここ）まで**。
- ローカルのみ（未プッシュ）:
  - `cc0eff8` モードバーを文字ではなくアイコンに（説明はツールチップ）…実機で見た目を確認済み
  - 作業中の変更（未コミット→引き継ぎ時に WIP コミットする）: 設定ダイアログの作り直し（下の「作業中」）
- ビルド: `cargo build --release -p kazakiri-win` → `target\release\kazakiri.exe`。**exe を起動中だと上書きできない**（閉じてもらう）。
- 確認: `cargo test --workspace`（コア 75 + 6、win 16）、`cargo clippy --workspace --all-targets`（警告ゼロを保つ）。

## 今日までにやったこと（v0.13.0 に含まれる）

モックのデザイン（https://claude.ai/artifact/SzVMKt8MuhV2hn2QvG8B7P ）に寄せた UI の全面整理。

- いちばん上の行がタイトルバー兼タブバー（`app/caption.rs`）。標準のタイトルバー・メニューバーはなし。メニューは ☰（F10）
- スクロールバーは子ウィンドウ（`layout.rs` の `layout_scrollbars`）
- コマンドパレット Ctrl+Shift+P（`app/palette.rs`、メニュー項目は `menu.rs` の `palette_items`）
- 検索・置換は本文右上の浮かぶバー（`app/findbar.rs`）。GREP だけ従来ダイアログ（`find_dialog.rs`）
- 列フィルタの演算子チップ、「対象の列」ドロップダウン（`app/coldrop.rs`）、モードバーは CSV/Markdown のときだけ
- ステータスバー左に行数・列数、設定ダイアログをカード形式に（Direct2D の背景 + 透明ボタンのスイッチ）
- メニュー ID の重なりバグの修正と、重なりのテスト（`menu.rs` の `id_ranges_do_not_overlap`。新しい ID を足したら、このテストの一覧にも足す）

## 作業中（未コミット。ビルドは通るが、見た目は未確認）

ユーザーの依頼 3 点のうちの残り:

1. **設定ウィンドウのタイトルバーにアプリアイコン** … `settings_dialog.rs` の `create()` で `WM_SETICON`（実装済み・未確認）
2. **設定ウィンドウのサイズ変更** … `WS_THICKFRAME | WS_MAXIMIZEBOX`、`WM_SIZE` → `relayout()`、`WM_GETMINMAXINFO`（実装済み・未確認）。コントロールの置き方は `Anchor`（`CardRight` / `CardWidth` / `ListHeight` / `Corner`）
3. **設定画面をモックの内容に近づける**（実装済み・未確認）
   - テーマの選び方（システム／ライト／ダーク。`settings.theme_mode`、Windows のテーマに追従は `WM_SETTINGCHANGE` の `ImmersiveColorSet`）
   - アクセントカラー（見本 6 色。`settings.accent`、`theme::set_accent_override`）
   - フォント名のコンボボックス（インストール済みフォントの一覧）と、大きさの − [数] ＋
   - 小見出し（テーマ／フォント／本文／画面の部品）、行番号のオン／オフ（`settings.line_numbers`）
   - 旧「フォント選択ダイアログ」は削除

また、ツールバー（モードバー）をアイコン表示にした件は `cc0eff8` に入っている。

### 次にやること

1. `cargo build --release -p kazakiri-win` と `cargo test --workspace` を回す（直前のビルドは完了を確認できていない）。
2. 設定ダイアログを開いて確認する（ライト・ダーク両方）。確認の仕方は下。
   - 外観ページ: テーマの切り替えが効くか、アクセント見本を押して色が変わるか、フォントのドロップダウンで選べるか、− ＋ で大きさが変わるか、プレビューが変わるか
   - ウィンドウを広げ・狭めて、カードの幅と右寄せの部品が追従するか（最小サイズ 640×460 DIP）
   - 基本ページの「行番号」オフで、本文の左の欄が消えるか（CSV の固定見出し行の番号も）
   - 設定ウィンドウのタイトルバーにアイコンが出るか
3. 問題がなければコミットし、プッシュする。リリースにするなら `Cargo.toml`（`[workspace.package]` の version）と `cargo update --workspace` で `Cargo.lock` を揃えてから、`v0.14.0` のタグ。**タグを打つ前に `Cargo.lock` の更新を忘れない**（v0.12.0 で一度やり直した）。
4. README の「設定ダイアログ」の説明（左に分類…）を、新しい項目に合わせて直す。

## 知っておくと良いこと

- 画面の確認は、自分のアプリのウィンドウだけを `PrintWindow` で撮る（画面全体のスクリーンショットは撮らない。他のアプリの内容が写るため）。PowerShell で DPI 対応（`SetProcessDPIAware`）にしないと、切り取られる。`PostMessage` は `EntryPoint="PostMessageW"` を指定する（指定しないと日本語が化ける）。
- `settings.ini`（`%APPDATA%\kazakiri\`）はユーザーの実際の設定。テストで切り替えた設定は、元に戻す。
- ユーザーの方針: 確認が必要なものは「確認してほしいこと」として明示する。コミットは区切りごと、プッシュとタグはユーザーの指示があってから。
- 既知の未対応: 列フィルタの一致件数、メニューバーを戻す設定、Alt+文字のメニューキー（F10 の ☰ が代わり）、検索バーの「選択範囲のみ置換」（GREP ダイアログの置換タブにある）。
