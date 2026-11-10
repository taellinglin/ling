# プログラムの実行

1 つのファイルは `ling run` で、プロジェクト全体は `lingfu run` で実行します。このページでは
その両方と、Ling がコードを実行する 2 つの方法、そしてプログラムを配布できる形にする方法を説明します。

## ファイルを実行する

```sh
ling run hello.ling
ling hello.ling          # 同じことを短く書いたもの
```

`ling run` はファイルを解析し、エントリーポイント（`束縛 開始 = 実行 { ... }`）を見つけて実行します。
ソースファイルの拡張子は `.ling`、`.灵`、`.霊`、`.령`、`.ลิง` のどれでもかまいません。

ファイル名のあとに書いたものは、プログラムが読めるようにコマンドラインに残ります。
`get_args()` はコマンドライン全体を返し、`cli_arg("port", "8080")` は `--port 99` のような
オプションを読み取ります（なければ `"8080"` を返します）。

## JIT かインタプリタか

Ling には 2 つのバックエンドがあります。

| コマンド | バックエンド | 使いどころ |
|---|---|---|
| `ling run file.ling` | Cranelift JIT（デフォルト） | 速さ。プログラムを先にネイティブコードへコンパイルします。 |
| `ling run --interp file.ling` | ツリーウォーキングインタプリタ | 診断。Ling の動作の基準です。 |

JIT がプログラムをコンパイルできない場合、`ling run` はプログラム全体をインタプリタで実行します。
とても大きなプログラムのコンパイルに時間がかかるときは、`--O0`、`--O1`、`--O2`、`--O3`
（デフォルトは `--O3`）で JIT の最適化レベルを下げられます。

### プログラムが「何もしない」ときは `--interp`

JIT では、存在しない関数を呼び出しても何も返らず、プログラムはそのまま続きます。
次のプログラムには `draw_rectt` というタイプミスがあります。

```ling
束縛 開始 = 実行 {
    印刷("前")
    draw_rectt(1, 2)
    印刷("後")
}
```
```output
前
後
```

インタプリタは止まって、何が問題かを教えてくれます。

```text
$ ling run --interp typo.ling
[detected language: Japanese (日本語)]
前
error[runtime]: unknown function 'draw_rectt'
 --> typo.ling
  traceback (deepest call last):
    0: start
  hint: check the spelling, or `use` the module that defines it
```

何かが無視されているように見えたら、いつも `--interp` で試してください。
バックエンドのそのほかの違いは[既知の制限](../language/limits.md)にまとめてあります。

### サーバーはインタプリタで動く

`http_serve` や `lingtp_serve` を呼び出すプログラムは、`--interp` を付けなくても必ず
インタプリタで実行されます。リクエストハンドラは組み込み関数に渡すクロージャで、JIT はまだ
それに対応していないためです。Web サービスはほとんどの時間ネットワークを待っているので、
目に見える違いはありません。[小さな Web サービス](../tutorials/web-service.md)を参照してください。

### ブラウザーで実行する

`ling run --wasm file.ling`（または `--web`）はプログラムを WebAssembly にビルドし、ローカルの
Web サーバーで配信してブラウザーを開きます。最初のビルドではランタイムをコンパイルするので、
1 分ほどかかります。`ling` の隣に `lingc` が必要です。

## lingfu でプロジェクトを作る

1 ファイルより大きいものは、`lingfu` にプロジェクトを作らせましょう。

```sh
lingfu new demo
cd demo
lingfu run
```

```text
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

`lingfu new` は次の構成を作ります。

```text
demo/
├── Ling.toml          マニフェスト
├── README.md
├── .gitignore
├── ling.ignore        公開時に除外するファイル
├── lexicons/en.ling
├── src/start.ling     エントリープログラム
└── tests/test.ling
```

| コマンド | 内容 |
|---|---|
| `lingfu run` | エントリープログラムを `ling run` で実行します。追加の引数はそのまま渡されます。 |
| `lingfu test` | `tests/` の下にある `.ling` ファイルを 1 つずつ実行します。 |
| `lingfu build` | `ling build --aot` でネイティブ実行ファイルをビルドします。 |
| `lingfu clean` | ビルド結果を削除します。 |

`lingfu new demo --lang ja` は、フォルダー名やファイル名も含めて日本語でプロジェクトを作ります。
マニフェストは `霊符.toml`、エントリーは `霊源/啓.霊` です。すべてのコマンドは [lingfu](../tools/lingfu.md)
で説明しています。

## Ling.toml

`lingfu new` が書き出すマニフェストは次のとおりです。

```toml
[spirit]
name = "demo"
kind = "bin"
version = "2030.0.0"

[author]
ling-lin = { email = "you@example.com" }

[realm]
language = "en"
level    = "铁"
source   = "src"

[root]
core = { version = "2030.0", language = "en" }

[depends]
# Add dependencies: spirit-name = { version = "x.y.z" }
```

- `[spirit]` はプロジェクトに名前を付けます。ここに `entry = "src/start.ling"` を追加できます。
- `[realm]` はプロジェクトの言語とソースフォルダーを設定します。
- `[root]` と `[depends]` は `core` 依存関係とパッケージを列挙します。

`lingfu run` と `lingfu test` は `src/start.ling` を自分で見つけます。一方 `ling build`
（つまり `lingfu build`）は、マニフェストで指定がなければ `main.ling` を探します。
ビルドの前に **`[spirit]` の下へ `entry = "src/start.ling"` を追加してください**。

## プログラムをビルドする

`ling build` はファイルやプロジェクトフォルダーを配布できる形に変えます。

```sh
ling build game.ling                      # ホストのプラットフォーム + web を dist/ に
ling build . --platform win,web --out dist
ling build . --platform all --pack --icon logo.svg
```

| オプション | 意味 |
|---|---|
| `--out <dir>` | 出力フォルダー（デフォルト `dist`）。プラットフォームごとにサブフォルダーができます。 |
| `--platform <list>` | カンマ区切り：`web`、`win`、`lin`、`mac`、`macarm`、`pios`、`kernel`、`rpi`、`lingos`、`lingos-rpi`。`all` は `win,lin,mac,web` です。 |
| `--icon <file>` | アプリのアイコン（`.svg`、`.png`、`.ico` など）。 |
| `--pack` | マニフェストの `[includes]` のファイルを実行ファイルに埋め込みます。 |
| `--aot` | インタプリタを埋め込む代わりに、事前にネイティブコードへコンパイルします。 |

`--platform` を省くと、マニフェストの `targets` が使われます。それもなければ、ホストの
プラットフォームと `web` をビルドします。デスクトップ向けは小さな Rust プロジェクトを生成して
コンパイルするので、Rust ツールチェーンが必要で、最初のビルドには数分かかることがあります。
`web` ターゲットは `lingc` を使います。

オプションとマニフェストのキーの一覧は [ling コマンド](../tools/ling.md)にあります。
