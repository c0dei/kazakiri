# kazakiri（風切）

数GBのログ・CSV・TSV・テキストを一瞬で開ける、Windows 向けの軽量テキストエディタ（開発中）。
EmEditor の「大きいファイルが速い」体験を、買い切り不要のオープンソースで目指す。

![Wine 上での動作画面（1GB の Shift_JIS CSV、1,500万行目付近で検索）](docs/screenshot-wine.png)

## いまできること（v0.2：閲覧専用ビューア）

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
| Ctrl+O | ファイルを開く |
| F5 | 再読み込み（追記だけなら増えた分だけ読み込む） |
| Ctrl+T | 末尾の追従（tail -f 相当）のオン・オフ |
| Ctrl+F | 検索語の入力（Enter: 次、Shift+Enter: 前、Ctrl+V: 貼り付け、Esc: 取り消し） |
| F3 / Shift+F3 | 次を検索 / 前を検索 |
| Alt+C / Alt+R | 大文字小文字の区別 / 正規表現 の切り替え |
| Esc | 検索の中止、選択の解除 |
| Ctrl+C / Ctrl+A | コピー / すべて選択（コピーは 256MB まで） |
| Ctrl+G | 行番号へ移動 |
| ↑↓ PgUp PgDn | スクロール |
| Ctrl+Home / Ctrl+End | 先頭 / 末尾 |
| ← → Home | 横スクロール |

正規表現は Rust の [regex](https://docs.rs/regex/) の書き方。`^` `$` は行頭・行末にマッチし、一致は 1 行の中に限る。

フォントは BIZ UDゴシック → MS ゴシック → Consolas の順に探す。環境変数 `KAZAKIRI_FONT` で指定できる。

## 構成

```
crates/
  kazakiri-core/   UI に依存しないコア（テスト・ベンチ可能）
    source.rs      メモリマップでファイルを開く
    encoding.rs    文字コード判定・デコード
    line_index.rs  並列・疎な行インデックス
    document.rs    行の取り出し
    search.rs      並列検索（文字列・大文字小文字無視・正規表現、前方・後方）
  kazakiri-win/    Windows UI（Win32 + Direct2D/DirectWrite）
  kazakiri-cli/    ベンチマーク・テストデータ生成
docs/PLAN.md     調査結果と開発ロードマップ
```
