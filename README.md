# kazakiri（風切）

[![CI](https://github.com/c0dei/kazakiri/actions/workflows/ci.yml/badge.svg)](https://github.com/c0dei/kazakiri/actions/workflows/ci.yml)

数GBのログ・CSV・TSV・テキストを一瞬で開いて編集できる、Windows 向けの軽量テキストエディタ（開発中）。
EmEditor の「大きいファイルが速い」体験を、買い切り不要のオープンソースで目指す。

![Wine 上での動作画面（1GB の Shift_JIS CSV、1,500万行目付近で検索）](docs/screenshot-wine.png)

## いまできること（v0.4）

### 編集
- 数GBのファイルでもそのまま編集（ピーステーブル：元ファイルはコピーせず、変更部分だけを持つ）
- カーソル移動（矢印、Ctrl+矢印で単語、Home/End、PgUp/PgDn、Ctrl+Home/End）、Shift で範囲選択
- 入力、改行（ファイルの改行コードに合わせる）、Backspace / Delete、Tab
- 切り取り・コピー・貼り付け、元に戻す・やり直し（連続入力はまとめて戻る）
- 日本語入力（IME の変換位置をカーソルに合わせる。確定文字列はまとめて挿入）
- ファイルの文字コード（Shift_JIS など）のまま保存。表せない文字（Shift_JIS での絵文字など）は入力時に止める
- 上書き保存・名前を付けて保存（別スレッドで書き出すので大きなファイルでも止まらない）
- 新規作成、閉じる・開くときに未保存の変更を確認

### 検索・置換・GREP
- 置換（Ctrl+H）：1 件ずつ確認しながら、またはすべて置換。正規表現では `$1` `${name}` や `\n` `\t` が使える
- すべて置換は 1 回の編集として適用し、Ctrl+Z 1 回でまとめて戻せる（3GB のログで 840 万件を約 1.6 秒）
- すべて検索して一覧（検索欄で Alt+Enter）：下のパネルに一覧を出し、クリックや F8 / Shift+F8 で移動
- GREP（Ctrl+Shift+F）：フォルダ内（サブフォルダも）のファイルをまとめて検索。ファイルごとに文字コードを判定するので、
  Shift_JIS・UTF-8・UTF-16 が混ざっていても同じ検索語で見つかる。隠しフォルダ（.git など）とバイナリは飛ばす

### 閲覧

- 数GBのファイルを即座に表示（メモリマップ + バックグラウンド行インデックス）
- 文字コード自動判定：UTF-8 / UTF-8 BOM / UTF-16LE・BE（BOM 有無）/ Shift_JIS(CP932) / EUC-JP
- 文字コードを指定して開き直す（エンコードメニュー）
- 改行コード判定：CRLF / LF / CR
- 行番号表示、縦横スクロール、マウスホイール（Shift+ホイールで横）
- 検索：前方・後方（端まで行ったら反対側から続ける）、大文字小文字の区別、正規表現。別スレッドで並列に探すので UI は止まらない（Esc で中止）
- マウスでの選択（ドラッグ、Shift+クリック、ダブルクリックで単語、行番号クリックで行）とコピー
- 行番号ジャンプ（Ctrl+G）、再読み込み（F5）、末尾の追従（Ctrl+T、tail -f 相当）。追記だけなら増えた分だけ読み込む
- メニューバー、最近使ったファイル
- ドラッグ&ドロップ、コマンドライン引数、Ctrl+O で開く
- ステータスバー：索引・検索の進捗、選択サイズ、行位置、文字コード、改行コード、サイズ

## 計測結果（Linux 上でコアを計測。ページキャッシュに載った状態）

| ファイル | 行数 | 先頭画面の表示 | 全行索引 | ジャンプ+60行取得 | 全文検索 | 消費メモリ（RssAnon） |
|---|---|---|---|---|---|---|
| 3GB UTF-8 ログ | 3,359万 | 0.9 ms | 0.21 秒 | 9 µs | 17 GB/s | 約 2 MB |
| 1GB Shift_JIS CSV | 2,141万 | 11 ms | 0.09 秒 | 10 µs | 9 GB/s | 約 3 MB |

全文検索は「大文字小文字を区別する文字列検索」の値（4 コア）。大文字小文字を区別しない検索や正規表現は、
UTF-8 なら同程度（3GB ログで 8〜18 GB/s）。Shift_JIS などは一度デコードしてから探すので 0.7〜1 GB/s。

ファイル本体は OS のページキャッシュに置かれ、アプリ固有のメモリはほぼ増えない。
初回（ディスクから読む）の索引時間はディスク速度に依存する（NVMe SSD なら数GB/秒）。

## ダウンロード（ビルド不要）

GitHub Actions が Windows 用の実行ファイルを自動でビルドしている。

- 最新の開発版：[Actions](https://github.com/c0dei/kazakiri/actions/workflows/ci.yml) で main の一番新しい成功した実行を開き、
  下の「Artifacts」から `kazakiri-windows-x64` をダウンロード（GitHub へのログインが必要）
- リリース版：`v` で始まるタグ（例：`v0.3.0`）を push すると [Releases](https://github.com/c0dei/kazakiri/releases) に zip が付く

## ビルド

Windows で。事前に次の 2 つを入れておく。

1. Rust（[rustup](https://rustup.rs/)）
2. Visual Studio Build Tools の「C++ によるデスクトップ開発」（リンカー `link.exe` のため）

```
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

インストール後、ターミナルを開き直してからビルドする。

```
cargo build --release
target\release\kazakiri.exe 開きたいファイル.log
```

`link.exe not found` と出たら 2. が入っていない。GNU 版ツールチェーン（`x86_64-pc-windows-gnu`）を使う場合は
MinGW の `dlltool.exe` が別途必要（windows クレートが使うため）。

Linux からのクロスビルド:

```
rustup target add x86_64-pc-windows-gnu
sudo apt-get install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu -p kazakiri-win
```

ベンチマーク（どの OS でも可）:

```
cargo run --release -p kazakiri-cli -- gen big.log 3072 log     # 3GB のテストログを作る
cargo run --release -p kazakiri-cli -- open big.log "ERROR"     # 開く・索引・検索の時間を測る
```

## キー操作

| キー | 動作 |
|---|---|
| Ctrl+N / Ctrl+O | 新規作成 / ファイルを開く |
| Ctrl+S / Ctrl+Shift+S | 上書き保存 / 名前を付けて保存 |
| Ctrl+Z / Ctrl+Y | 元に戻す / やり直し（Ctrl+Shift+Z でもやり直し） |
| Ctrl+X / Ctrl+C / Ctrl+V | 切り取り / コピー / 貼り付け |
| Ctrl+A | すべて選択 |
| ← → ↑ ↓ / Ctrl+← → | カーソル移動 / 単語単位（Shift で選択） |
| Home / End | 行頭（インデントの後ろと交互）/ 行末 |
| Ctrl+Home / Ctrl+End | ファイルの先頭 / 末尾 |
| PgUp / PgDn | 1 画面ぶん移動 |
| Ctrl+↑ / Ctrl+↓ | カーソルを動かさずにスクロール |
| F5 | 再読み込み（追記だけなら増えた分だけ読み込む） |
| Ctrl+T | 末尾の追従（tail -f 相当）のオン・オフ |
| Ctrl+F | 検索語の入力（Enter: 次、Shift+Enter: 前、Ctrl+V: 貼り付け、Esc: 取り消し） |
| F3 / Shift+F3 | 次を検索 / 前を検索 |
| Alt+C / Alt+R | 大文字小文字の区別 / 正規表現 の切り替え |
| Esc | 検索・保存の中止、選択の解除 |
| Ctrl+G | 行番号へ移動 |
| Ctrl+H | 置換（検索語 → 置換後。Enter: 置換して次、Shift+Enter: 置換せず次、Ctrl+Enter / Alt+A: すべて置換） |
| 検索欄で Alt+Enter | すべて検索して一覧 |
| Ctrl+Shift+F | GREP（検索語 → フォルダ → 対象ファイル `*.log;*.txt` など） |
| F8 / Shift+F8 | 一覧の次 / 前の結果へ |

コピー・切り取りは 256MB まで。編集は行インデックスの作成が終わってから（数GBでも 1 秒前後）。
編集中は再読み込み・末尾の追従はできない。

正規表現は Rust の [regex](https://docs.rs/regex/) の書き方。`^` `$` は行頭・行末にマッチし、一致は 1 行の中に限る。

フォントは BIZ UDゴシック → MS ゴシック → Consolas の順に探す。環境変数 `KAZAKIRI_FONT` で指定できる。

## 構成

```
crates/
  kazakiri-core/   UI に依存しないコア（テスト・ベンチ可能）
    source.rs      メモリマップでファイルを開く
    encoding.rs    文字コード判定・デコード
    line_index.rs  並列・疎な行インデックス
    document.rs    行の取り出し、編集（新しい Document を返す）、保存
    pieces.rs      ピーステーブル
    editor.rs      元に戻す・やり直す、変更の有無
    search.rs      並列検索（文字列・大文字小文字無視・正規表現、前方・後方、全件、置換後の文字列）
    grep.rs        フォルダ内のファイルの検索
  kazakiri-win/    Windows UI（Win32 + Direct2D/DirectWrite）
    app.rs         ウィンドウ、描画、検索、選択
    app/edit.rs    カーソル移動、入力、IME、保存
    app/results.rs 置換、結果一覧パネル、GREP
  kazakiri-cli/    ベンチマーク・テストデータ生成
docs/PLAN.md     調査結果と開発ロードマップ
```
