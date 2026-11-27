# 開発の手順と約束

最終更新：2026-11-27。設計の考え方は [DESIGN](DESIGN.md)。

## ビルド

Windows で、Rust（rustup）と Visual Studio Build Tools の「C++ によるデスクトップ開発」を入れる（詳しくは README）。

```
cargo build --release -p kazakiri-win        # target\release\kazakiri.exe
.\build-release.ps1 [-Test] [-Zip]           # 手元用のスクリプト（リポジトリに含めていない）
```

起動中の exe は上書きできない（閉じてもらう）。

## 確認

変更したら、次の 3 つを通す。CI（`.github/workflows/ci.yml`）も同じものを動かす。

```
cargo test --release --workspace
cargo clippy --release --workspace --all-targets -- -D warnings
cargo test --release -p kazakiri-core --test perf_guard -- --ignored --nocapture
```

| テスト | 内容 |
|---|---|
| core の単体テスト | 各モジュール |
| `tests/edit_model.rs` | 編集結果を、素朴な実装と突き合わせる |
| `tests/share_mode.rs`（Windows のみ） | 開いているファイルに、ほかのプロセスができること。追記・切り詰め後の読み直し |
| `tests/perf_guard.rs`（`--ignored`） | 時間の予算。`KAZAKIRI_PERF_MB`（既定 128）、`KAZAKIRI_PERF_SLACK`（CI は 3） |
| win の単体テスト | 画面を作らない部分（設定、キー割り当て、メニュー ID の重なりなど） |

CI の Linux ジョブは core と cli だけを確認する。

計測用：`cargo run --release -p kazakiri-cli -- gen big.log 3072 log` と `-- open big.log "ERROR"`。`crates/kazakiri-core/examples/` にも個別の計測がある。

## 画面の確認

- **アプリの起動や画面の自動操作は、作業者（自動化を含む）が勝手に行わない。** 画面の確認が必要な項目は「確認してほしいこと」として持ち主に渡し、未確認と明記する。許可は、その回の作業に限る。
- 許可を得て操作するときは、起動するプロセスの環境変数 `APPDATA`（と `TEMP`）を作業用のフォルダに向ける。持ち主の設定・履歴・セッションに触れずに済む。操作は、起動した自分のウィンドウへのメッセージ送信（`WM_COMMAND` にメニューの ID、入力欄に `WM_SETTEXT`）にすると、ほかのアプリに影響しない。
- 持ち主の許可を得て撮るときは、自分のアプリのウィンドウだけを `PrintWindow` で撮る（画面全体は、ほかのアプリが写る）。PowerShell は `SetProcessDPIAware` を呼ばないと切り取られる。`PostMessage` は `EntryPoint="PostMessageW"` を指定する（指定しないと日本語が化ける）。
- `%APPDATA%\kazakiri\` は持ち主の実際の設定。向け先を変えずに操作したときは、変えたものを戻す。
- 起動で固まる・落ちるときは、`%TEMP%\kazakiri-start.log` と `kazakiri-crash.log` を見る。

## コードを足すときの注意

| こと | すること |
|---|---|
| トップレベルのウィンドウを足す | `util.rs` の `set_window_icon` を呼ぶ（`CLAUDE.md`） |
| メニューの ID を足す | `menu.rs` の `id_ranges_do_not_overlap` の一覧にも足す |
| 設定を足す | `settings.rs` の構造体・既定値・`parse`・`to_text`・往復のテスト。画面に出すなら `settings_dialog.rs` の ID・`sync`・`commit_edits`・`on_command` の入力欄の一覧・行の追加。README の設定の一覧 |
| 設定画面の入力欄の行 | 説明文の幅は約 150 DIP で、超えると「…」になる。長い説明はページの下の `STATIC` にして、`notes` に高さを足す |
| ダイアログを出す | `App` を借りたまま出さない。`Deferred` を返す |
| 時間のかかる処理 | 別スレッドにして、中止と進捗を付ける。タブの切り替え（`leave_tab`）とファイルの読み直しで古くなる状態を考える |
| バイト位置を覚える状態を足す | `tabs.rs` の `Tab`、`files.rs` の `forget_stale_positions` に足すか確認する |
| Direct2D の色 | 共有ブラシの色の使い回しで崩れたことがある。同じ症状は専用のブラシで直す |

そのほか、標準部品のダーク対応で分かったことは [STANDARD_CONTROLS](STANDARD_CONTROLS.md)。

## 版を出す

持ち主の指示があるときだけ。

1. `Cargo.toml` の `[workspace.package]` の `version` を上げ、`cargo update --workspace` で `Cargo.lock` を揃える。
2. コミットして、`v0.x.y` のタグを付けてプッシュする。CI が zip を GitHub Releases に添付する。

コミットは区切りごと。プッシュ・タグ・PR は指示があってから。
未追跡の手元のファイル（`New text document.txt`、`build-release.ps1`、`.lnk`）はコミットしない。

## ドキュメント

コードの変更と同じ作業の中で直す。

| 変わったこと | 直す場所 |
|---|---|
| 機能・操作・設定・キー | `README.md` |
| 仕組み・方針・上限・判断の理由 | `docs/DESIGN.md` |
| ビルド・テスト・開発の約束 | `docs/DEVELOPMENT.md` |
| 課題の着手・完了・新しく見つけた不具合 | `docs/IMPROVEMENTS.md`（状態・更新日・更新履歴） |
| いまの状況と、次のセッションへの引き継ぎ | `docs/HANDOFF.md` |

`docs/PLAN.md`（初期の計画）と `docs/STANDARD_CONTROLS.md`（標準部品の経緯）は記録として残す。いまの仕様は DESIGN を正とする。
