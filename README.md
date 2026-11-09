# hayate（疾風）

数GBのログ・CSV・TSV・テキストを一瞬で開ける、Windows 向けの軽量テキストエディタ（開発中）。
EmEditor の「大きいファイルが速い」体験を、買い切り不要のオープンソースで目指す。

![Wine 上での動作画面（1GB の Shift_JIS CSV、1,500万行目付近で検索）](docs/screenshot-wine.png)

## いまできること（v0.1：閲覧専用ビューア）

- 数GBのファイルを即座に表示（メモリマップ + バックグラウンド行インデックス）
- 文字コード自動判定：UTF-8 / UTF-8 BOM / UTF-16LE・BE（BOM 有無）/ Shift_JIS(CP932) / EUC-JP
- 改行コード判定：CRLF / LF / CR
- 行番号表示、縦横スクロール、マウスホイール（Shift+ホイールで横）
- 検索（Ctrl+F → Enter、F3 で次）、一致箇所のハイライト
- 行番号ジャンプ（Ctrl+G）
- ドラッグ&ドロップ、コマンドライン引数、Ctrl+O で開く
- ステータスバー：索引の進捗、行位置、文字コード、改行コード、サイズ

## 計測結果（Linux 上でコアを計測。ページキャッシュに載った状態）

| ファイル | 行数 | 先頭画面の表示 | 全行索引 | ジャンプ+60行取得 | 全文検索 | 消費メモリ（RssAnon） |
|---|---|---|---|---|---|---|
| 3GB UTF-8 ログ | 3,359万 | 0.9 ms | 0.21 秒 | 9 µs | 5〜6.6 GB/s | 約 2 MB |
| 1GB Shift_JIS CSV | 2,141万 | 11 ms | 0.09 秒 | 10 µs | 3 GB/s | 約 3 MB |

ファイル本体は OS のページキャッシュに置かれ、アプリ固有のメモリはほぼ増えない。
初回（ディスクから読む）の索引時間はディスク速度に依存する（NVMe SSD なら数GB/秒）。

## ビルド

Windows で（Rust の stable を [rustup](https://rustup.rs/) で入れておく）:

```
cargo build --release
target\release\hayate.exe 開きたいファイル.log
```

Linux からのクロスビルド:

```
rustup target add x86_64-pc-windows-gnu
sudo apt-get install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu -p hayate-win
```

ベンチマーク（どの OS でも可）:

```
cargo run --release -p hayate-cli -- gen big.log 3072 log     # 3GB のテストログを作る
cargo run --release -p hayate-cli -- open big.log "ERROR"     # 開く・索引・検索の時間を測る
```

## キー操作

| キー | 動作 |
|---|---|
| Ctrl+O | ファイルを開く |
| Ctrl+F / Enter / Esc | 検索語の入力 / 検索 / 取り消し |
| F3 | 次を検索 |
| Ctrl+G | 行番号へ移動 |
| ↑↓ PgUp PgDn | スクロール |
| Ctrl+Home / Ctrl+End | 先頭 / 末尾 |
| ← → Home | 横スクロール |

フォントは BIZ UDゴシック → MS ゴシック → Consolas の順に探す。環境変数 `HAYATE_FONT` で指定できる。

## 構成

```
crates/
  hayate-core/   UI に依存しないコア（テスト・ベンチ可能）
    source.rs      メモリマップでファイルを開く
    encoding.rs    文字コード判定・デコード
    line_index.rs  並列・疎な行インデックス
    document.rs    行の取り出し、検索
  hayate-win/    Windows UI（Win32 + Direct2D/DirectWrite）
  hayate-cli/    ベンチマーク・テストデータ生成
docs/PLAN.md     調査結果と開発ロードマップ
```
