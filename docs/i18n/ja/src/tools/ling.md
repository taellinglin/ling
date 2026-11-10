# ling コマンド

`ling` は Ling プログラムを実行し、ビルドし、図にし、変換します。このページではすべての
サブコマンドとオプションを紹介します。もっとやさしい入門は[プログラムの実行](../getting-started/running.md)から始めてください。

```text
ling run [--interp|--wasm|--O0|--O1|--O2|--O3] <file.ling>
ling <file.ling>                       (shorthand for: ling run <file.ling>)
ling build <file.ling|dir> [--out dir] [--platform list] [--icon file] [--pack] [--aot]
ling ast [path] [--technical|--artwork|--ling|--all] [--out dir]
ling visualize <file.ling>
ling convert <asset> [-o out.ling] [--no-compression]
```

| サブコマンド | 用途 |
|---|---|
| `run` | プログラムを実行します（デフォルトは JIT）。 |
| `build` | ファイルやプロジェクトを 1 つ以上のプラットフォーム向けの配布物にコンパイルします。 |
| `ast` | ファイルやプロジェクトの構文木を SVG ファイルとして描きます。 |
| `visualize` | ファイルの構文木を 1 枚の SVG として標準出力に書き出します。 |
| `convert` | アセット（モデル、サウンド、MIDI、SVG）を読み込み可能な `.ling` ファイルに変換します。 |

引数なしで `ling` を実行するか、認識できないものを渡すと、使い方を表示して終了ステータス 0 で終わります。
1 行目にバージョンが表示されます。`--version` や `--help` オプションはなく、`check`、`fmt`、
`translate`、`test` といったサブコマンドもありません。

ソースファイルの拡張子は `.ling`、`.灵`、`.霊`、`.령`、`.ลิง` のどれでも使えます。

### `ling run`

ファイルを解析し、エントリーポイントを見つけて実行します。

| オプション | 効果 |
|---|---|
| *（なし）* | Cranelift JIT（デフォルト）。JIT でコンパイルできなければ、プログラム全体をインタプリタで実行します。 |
| `--interp` | ツリーウォーキングインタプリタ。意味の基準となる実装で、エラーメッセージがいちばん詳しく出ます。 |
| `--wasm`（または `--web`） | プログラムを WebAssembly にビルドし、ローカルで配信してブラウザーを開きます。`ling` の隣に `lingc` が必要です。 |
| `--O0` `--O1` `--O2` `--O3` | JIT の最適化レベル（デフォルト `--O3`）。とても大きなプログラムのコンパイルが遅いときに下げます。 |

オプションはファイル名の前でも後でもかまいません。Ling の拡張子を持つ最初の引数がプログラムです。
そのあとの引数はプログラム用に残り、`get_args()` と `cli_arg(name, default)` で読み取れます。

知っておくと役立つこと：

- `http_serve` や `lingtp_serve`（またはタイ語の `เว็บเสิร์ฟ` などの別名）を呼び出すプログラムは、
  自動的にインタプリタで実行されます。JIT がまだクロージャ引数に対応していないためです。
  短縮形の `ling file.ling` はこの判定をしないので、サーバーには `ling run` を使ってください。
- JIT では、未知の関数は黙って `()` を返します。インタプリタは `unknown function` を報告します。
  [既知の制限](../language/limits.md)を参照してください。
- エントリーポイントのないファイルはライブラリとして扱われ、`error[runtime]: no entry point` で失敗します。
- 英語以外で書かれたソースファイルでは、`ling` は実行前に `[detected language: ...]` を標準エラー出力に表示します。
- ファイルがなければ `[ling] error: file does not exist: <path>` と表示します。構文エラーは
  `error[parse]: ...`、実行時エラーはトレースバック付きの `error[runtime]: ...` です。
  これらはすべて終了ステータス 1 で終わります。

### `ling build`

プログラムやプロジェクトを配布物にコンパイルします。

| オプション | 意味 |
|---|---|
| `<target>` | `.ling` ファイルかディレクトリー（デフォルト `.`）。ディレクトリーはマニフェスト（`灵符.toml`、`ลิงฟู.toml`、`Ling.toml` の順）を通して読み込まれます。マニフェストがなければ、`main.ling`、`start.ling`、`启.灵`、`เริ่ม.ลิง`、`src` フォルダー、または任意の `.ling` ファイルを探します。 |
| `--out <dir>` | 出力フォルダー。デフォルトは `dist`。プラットフォームごとに `dist/web` や `dist/windows` のようなサブフォルダーに入ります。 |
| `--platform <list>` | カンマ区切りのターゲット（下の表）。`all` は `win,lin,mac,web` です。 |
| `--icon <file>` | アプリのアイコン（`.svg`、`.png`、`.jpg`、`.bmp`、`.ico`）。マニフェストの `icon` より優先されます。 |
| `--pack` | `[includes]` のリソースとプログラムを実行ファイルに埋め込み、1 つの自己完結したファイルにします。 |
| `--aot` | インタプリタを埋め込む代わりに、事前にネイティブコードへコンパイルします。 |

| プラットフォーム | 別名 | 出力 |
|---|---|---|
| `web` | | WebGL/WebAssembly バンドル（`lingc` でビルド） |
| `win` | `windows` | Windows 実行ファイル |
| `lin` | `linux` | Linux 実行ファイル |
| `mac` | `macos`, `darwin` | macOS（Intel）実行ファイル |
| `macarm` | `mac-arm`, `mac-arm64`, `macos-arm64`, `apple-silicon` | macOS（Apple シリコン）実行ファイル |
| `pios` | `pi`, `raspios`, `linux-arm64`, `linarm`, `arm64` | Raspberry Pi OS / 64 ビット ARM Linux |
| `kernel` | `bare` | LingOS ベアメタルカーネルイメージ（`os` プロファイル） |
| `rpi` | `raspberrypi` | Raspberry Pi 向け LingOS イメージ（`os` プロファイル） |
| `lingos` | `app`, `apps` | LingOS ユーザーランドアプリ（`os` プロファイル） |
| `lingos-rpi` | `apps-rpi`, `app-rpi` | Raspberry Pi 向け LingOS ユーザーランドアプリ（`os` プロファイル） |

`--platform` を省くと、マニフェストの `targets` が使われ、それもなければホストのプラットフォームと
`web` をビルドします。リストがマニフェストから来た場合、手元のマシンでビルドできない（そして `cross`
が入っていない）デスクトップ向けターゲットは、メッセージを出してスキップされます。
`--platform` を明示した場合はエラーになります。

ネイティブ向けは小さな Rust プロジェクトを生成してコンパイルするので、**Rust ツールチェーンが必須**で、
最初のビルドには数分かかることがあります。

#### マニフェストのキー

`ling build` はマニフェストを完全な TOML パーサーではなく、1 行ずつ読み取ります。設定はすべて
`key = value` の形で 1 行に書き、配列も 1 行に収めてください。

| キー | 意味 |
|---|---|
| `name` | プロジェクト名。実行ファイル名に使われます。 |
| `version` | バージョン文字列。デフォルトは `0.1.0`。 |
| `entry` | マニフェストからの相対パスで書くエントリープログラム。デフォルトは `main.ling`。 |
| `kind` | `bin`（デフォルト）、`web`、`game`、`ui`、`ai`、`crypto`、`lib`、`polyglot`、`kernel`。 |
| `icon` | アイコンファイル。 |
| `targets` | デフォルトのプラットフォーム。例：`targets = ["win", "lin", "web"]`。 |
| `pack` | `pack = true` は `--pack` と同じです。 |
| `includes` | リソースのグロブ。または 1 行に 1 つ引用符付きパターンを書く `[includes]` セクション。`*` はフォルダー内、`**` はフォルダーをまたいでマッチします。 |
| `ling-features` | 実行ファイルで有効にする `ling` の Cargo フィーチャー。例：`db_*` を使うプログラムには `["web"]`。 |

```toml
# Ling.toml
[project]
name = "my-game"
version = "1.0.0"
entry = "main.ling"
icon = "images/logo.svg"
targets = ["win", "lin", "web"]
pack = true

[includes]
"/music/*.wav"
"/font/*.otf"
"/levels/**/*.json"
```

含めたファイルは同じ構成のまま実行ファイルの隣にコピーされるか、`--pack` で埋め込まれます。
どちらの場合も `read_file` などのファイル用組み込み関数はそのまま動きます。

### `ling ast`

ファイルやプロジェクト全体の構文木を SVG で描きます。ディレクトリーは再帰的にたどられ（隠しフォルダー、
`target`、`dist`、`node_modules`、`AST`、`灵碑` は除外）、1 つのプログラムとして扱われます。

| オプション | 意味 |
|---|---|
| `[path]` | ファイルまたはディレクトリー。デフォルトは `.`。 |
| `--technical`、`--artwork`、`--ling` | 描画スタイルを 1 つ以上選びます。どれも指定しなければ 3 つすべてを作ります。 |
| `--all` | 3 つのスタイルすべて。 |
| `--out <dir>` | 出力フォルダー。デフォルトは `AST`。 |

```text
$ ling ast hello.ling
ling ast: 'hello' — 1 file(s), 0 fn(s)
  ✓ AST\hello.technical.svg
  ✓ AST\hello.artwork.svg
  ✓ AST\hello.ling.svg
```

### `ling visualize`

ファイルの構文木を 1 枚の SVG として標準出力に書き出します。ファイルにリダイレクトしてください。

```sh
ling visualize hello.ling > hello.svg
```

ファイルを解析するだけなので、プログラムが構文的に正しいかを手早く確かめる方法にもなります。

### `ling convert`

アセットを `使う` で読み込める `.ling` ファイルに変換します。[アセット変換](convert.md)を参照してください。

```sh
ling convert model.glb -o model.ling
```

## 終了ステータス

`ling` は成功すると 0 を、エラーを報告すると 1 を返します。
