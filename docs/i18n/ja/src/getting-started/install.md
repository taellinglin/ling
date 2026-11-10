# インストール

Ling はコマンド 1 つでインストールできます。手に入るのは 2 つのプログラムです。
`ling` は Ling プログラムを実行・ビルドし、`lingfu` はプロジェクトを管理します。

## ワンライナーでインストール

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

Rust ツールチェーンがなければ、スクリプトが最初にインストールします（[rustup](https://rustup.rs)
と同じ方法です）。そのあと crates.io から `ling` と `lingfu` をビルドします。

## Cargo でインストール

Rust がすでに入っているなら、crates.io から 2 つのプログラムをインストールできます。

```sh
cargo install ling-lang lingfu
```

どちらも同じフォルダー（`~/.cargo/bin`）に入ります。`lingfu` はまず自分の隣にある `ling` を探し、
見つからなければ `PATH` から探します。

## ソースからビルド

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# 実行ファイルは target/release/ にあります（ling と lingc、lx、ling-repl）
```

同じチェックアウトから `lingfu` をビルドするには、`cargo build --release -p lingfu` を実行します。

## 動作を確認する

引数なしで `ling` を実行します。使い方の 1 行目にバージョンが表示されます。

```sh
ling
# ling 2030.1.53 — The Omniglot Systems Language
# Usage:
#   ling run <file.ling>   ...
lingfu --version
# lingfu v2030.1.19
```

`ling` には `--version` オプションがありません。バージョンはこの使い方の表示で確認します。

## インストールプロファイル

`cargo install ling-lang` はすべてをインストールします（`full` プロファイル）。ビルドする量を
減らしたいときは、デフォルトを無効にして必要なプロファイルを選びます。

```sh
cargo install ling-lang --no-default-features --features core        # コンパイラーとインタプリタだけ
cargo install ling-lang --no-default-features --features web         # + HTTP サーバー/クライアント、SQLite
cargo install ling-lang --no-default-features --features game,audio  # サウンド付きのゲームキット
```

| プロファイル | 追加されるもの |
|---|---|
| `core` | 字句解析器、構文解析器、Cranelift JIT と AOT、インタプリタ、ソフトウェアウィンドウと描画（`draw_*`、フォント、画像、メッシュ）。すべてのプロファイルに含まれます。 |
| `web` | `http_*` サーバーとクライアント（[Web サーバーと HTTP](../reference/web.md)）、`db_*` SQLite（[データベース（SQLite）](../reference/database.md)）、静的ファイル、TOTP、zip/tar、動画再生、プロセス、ハッシュと `password_hash`。 |
| `privacy` | 耐量子 ID、`lingtp://`、ピアツーピア `p2p_*`、Ling Wallet `lw_*`、`ssh_*`。`web` と `ui_*` ウィジェットも含みます。 |
| `game` | 物理（ソフトボディ、剛体、液体、glTF）、GPU ラスタライズ、アニメーション、ダイアログ、ゲームパッド、音響ボディ、`ui_*` ウィジェット。 |
| `ai` | ニューラルネットワーク（`nn_*`）、ビヘイビアツリー（`bt_*`）、学習型ダイアログ、音声認識（`stt_*`）。 |
| `audio` | サウンドエンジン、音楽のデコード・合成・MIDI、マイク、LingStationX DAW。 |
| `os` | `ling build --platform kernel`、`rpi`、`lingos`、`lingos-rpi`（LingOS イメージ）。 |
| `full` | 上記すべて（デフォルト）。LingAnime ペイントエンジンの `art`（`art_*`）も含みます。 |

ビルドに含まれていないプロファイルの組み込み関数を呼び出すと、プログラムはエラーで止まり、
どのプロファイルで再インストールすればよいかを表示します。重い追加機能はどのプロファイルでも
個別に有効にする必要があります：`llm`、`llm-cuda`、`vision`、`cuda`、`gpu-cuda`、`gpu`、`llvm`、`emu`。

## 動作環境

- Rust 1.85 以降（インストールスクリプトが用意してくれます）。
- Linux：オーディオに `libasound2-dev` が必要です。
- macOS と Windows：追加の依存関係はありません。
- デスクトップ向けの `ling build` は小さな Rust プロジェクトをコンパイルするので、
  Rust ツールチェーンが必要です。[ling コマンド](../tools/ling.md)を参照してください。

## 次のステップ

- 最初のプログラムを書く：[ハローワールド](hello.md)。
- コマンドを覚える：[プログラムの実行](running.md)。
- [Ling ブック](../books.md)を読む。初級、中級、上級、完全リファレンスの 4 巻からなる無料の PDF シリーズです。
