# lingfu

`lingfu`（灵符、「霊の札」）は Ling のプロジェクト管理ツールです。プロジェクトの作成、実行とテスト、
実行ファイルのビルド、パッケージレジストリとのやり取りを行い、プロジェクト全体を別の人間の言語に
書き換えることもできます。実行とビルドには `ling` を呼び出します。`ling` はまず自分の隣、次に `PATH` から探します。

```text
lingfu <command> [arguments]
lingfu --help | -h | help
lingfu --version | -V | version
```

`lingfu` は現在のフォルダーとその親フォルダーからマニフェストを探して、プロジェクトを見つけます：
`Ling.toml`、`灵符.toml`、`霊符.toml`、`영부.toml`、`ลิงฟู.toml`。

## コマンド一覧

| コマンド | 別名 | 用途 |
|---|---|---|
| `new <name>` | `create` | プロジェクトフォルダーを作ります。 |
| `init` | `initialize` | 現在のフォルダーにプロジェクトを作ります。 |
| `run` | `execute`, `start` | プロジェクトを実行します。 |
| `test` | `verify` | プロジェクトのテストを実行します。 |
| `build` | `compile`, `make` | ネイティブ実行ファイルをビルドします。 |
| `clean` | `purge`, `clear` | ビルド結果を削除します。 |
| `tree` | `structure`, `ls` | プロジェクトのツリーを表示します。 |
| `manifest` | `show`, `info` | マニフェストを表示します。 |
| `normalize <lang>` | `norm`, `localize` | プロジェクトを別の言語に書き換えます。 |
| `add <package>` | `install`, `include` | レジストリからパッケージをインストールします。 |
| `search <query>` | `find`, `seek` | レジストリを検索します。 |
| `publish` | `release`, `share` | プロジェクトを公開します。 |
| `login <api-key>` | `sign-in`, `auth` | レジストリの認証情報を保存します。 |
| `logout` | `sign-out`, `deauth` | 認証情報を消去します。 |
| `doc` | `docs`, `document` | ドキュメントをビルドします。 |
| `bench`, `update` | | `cargo bench` / `cargo update` を実行します。 |
| `wizard` | `guide`, `create-wizard` | 対話式のプロジェクト作成。 |
| `seed` | | プロジェクトの完全性フィンガープリント。 |
| `ast`, `convert` | | `ling ast` / `ling convert` に転送します。 |
| `check`, `fmt`, `translate` | | 今はまだ使わないでください（後述）。 |

共通オプション：`--lang`/`-l <code>`（雛形の言語：`en`、`zh`、`ja`、`ko`、`th`、`ru`）、
`--type`/`-t <kind>`、`--root-lang <code>`、`--crates`、`--path <dir>`、`--dry-run`/`-n`。

各コマンドにはほかの言語の名前もあります。たとえば `test` は中国語で `试`、タイ語で `ทดสอบ` です。
これらは実行ファイル自体をローカライズした名前（`霊符` や `灵符.exe` など）にしたときだけ使えます。
`lingfu` という名前で起動するときは、英語のコマンドを使ってください。

### `lingfu new` と `lingfu init`

```text
lingfu new <name> [--type bin|lib|game|ui|ai|crypto|web|polyglot] [--lang en|zh|ja|ko|th|ru]
```

マニフェスト、エントリープログラムの入ったソースフォルダー、テストフォルダー、レキシコンフォルダー、
`README.md`、`.gitignore`、`ling.ignore` を含む `<name>/` を作ります。`lingfu init` は同じことを
現在のフォルダーで行い、フォルダー名をプロジェクト名にします。

`--lang` は生成されるファイルの言語**と**フォルダー名を決めます。

| 言語 | マニフェスト | ソース | エントリー | テスト | ビルド |
|---|---|---|---|---|---|
| 英語 | `Ling.toml` | `src` | `start.ling` | `tests` | `build` |
| 中国語 | `灵符.toml` | `灵源` | `启.灵` | `灵镜` | `灵碑` |
| 日本語 | `霊符.toml` | `霊源` | `啓.霊` | `霊鏡` | `霊碑` |
| 韓国語 | `영부.toml` | `령원` | `시작.령` | `령경` | `령비` |
| タイ語 | `ลิงฟู.toml` | `ต้นกำเนิด` | `เริ่ม.ลิง` | `ทดสอบ` | `สร้าง` |

マニフェストには次のテーブルがあります：`[spirit]`（`name`、`kind`、`version`、省略可能な `entry`）、
`[author]`、`[realm]`（`language`、`level`、`source`）、`[root]`（`core` 依存関係）、`[depends]`。
例は[プログラムの実行](../getting-started/running.md#lingtoml)にあります。

### `lingfu run`

プロジェクトのエントリープログラムを `ling run` で実行します。エントリーは次のうち最初に存在するファイルです：
マニフェストの `entry`、`<src>/<エントリーファイル>`、`<root>/<エントリーファイル>`、`<src>/main.ling`、
`<root>/main.ling`。追加の引数は `ling run` に渡されます。

```text
$ lingfu run
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

### `lingfu test`

テストフォルダーの下にある `.ling` ファイルを 1 つずつ `ling run` で実行し、最初に失敗したところで止まります。
テストはエントリーポイントを持つ普通のプログラムです（`実行` ブロックを値とする `束縛` ならどれでもエントリーになります）。

```text
$ lingfu test
Testing Ling project...
  test: ...\demo\tests\test.ling
✨ All mirrors are clear ✨
🔮 Ling tests passed for demo
✓ All tests passed!
```

テストは JIT で実行され、JIT は未知の関数があっても止まらないので、壊れたテストが「合格」することがあります。
各テストに PASS か FAIL をはっきり表示させるか、自分で `ling run --interp` で実行してください。
テストフォルダーに置いた補助ファイルもテストとして扱われます。

### `lingfu build`

`ling build <project> --aot` を呼び出し、`--out`、`--platform`、`--icon`、`--pack` を渡します。
`--no-aot` を付けるとインタプリタを埋め込む方式になり、`--path <dir>` でプロジェクトフォルダーを指定できます。
マニフェストの隣に `Cargo.toml` があれば、代わりに `cargo build` を実行します。

最初のビルドの前に、`[spirit]` の下へ `entry = "src/start.ling"` を追加してください。そうしないと
`ling build` は `main.ling` を探し、`entry '...' not found` で止まります。ビルドには Rust ツールチェーンが
必要です。[ling コマンド](ling.md#ling-build)を参照してください。

### `lingfu clean`、`tree`、`manifest`

- `clean` はビルドフォルダーとロックフォルダー（`build` と `.lock`、またはそのローカライズ名）を削除し、
  Rust のクレートがあれば `cargo clean` も実行します。
- `tree` はプロジェクトのフォルダーツリーを表示します。
- `manifest` はマニフェストのパスと内容を表示します。

### `lingfu normalize`

```text
lingfu normalize <lang> [--dry-run|-n] [--content-only] [--files-only]
```

現在のフォルダーにあるプロジェクト全体を、1 つの人間の言語に書き換えます。すべてのソースファイルの
キーワードと組み込み関数名を翻訳し、ソースファイルと標準フォルダーの名前を変え、インポートを合わせて
直します。`<lang>` は `en`、`zh`、`ja`、`ko`、`th` などのコードです（`japanese` や `日本語` のような名前も使えます）。
省略するとメニューが表示されます。

| オプション | 効果 |
|---|---|
| `--dry-run`, `-n` | 何も変更せず、変更点の一覧だけを表示します。 |
| `--content-only` | ファイルの内容だけを書き換え、名前はそのままにします。 |
| `--files-only` | ファイルとフォルダーの名前だけを変え、内容はそのままにします。 |

```text
$ lingfu normalize zh --dry-run
Normalizing to Chinese  [dry-run]
  ~ lexicons\en.ling
  ~ src\start.ling
  ~ tests\test.ling
  mv tests\test.ling → tests\测试.ling
  mv build → 构建
  mv src → 源码
  mv tests → 测试

Dry run complete — no files changed.
  ✓ 3 file(s) rewritten
  ~ 1 file(s) renamed
  ↕ 3 folder(s) renamed
```

> **警告：** `normalize` はフォルダー内の**すべての** Ling ファイルをその場で書き換えます。
> 先にプロジェクトをコミット（またはコピー）し、ドライランをしてください。

識別子、コメント、文字列の中のテキストはそのまま残り、変わるのはキーワードと組み込み関数だけです。
また `normalize` は独自のフォルダー名を使うため、`lingfu new --lang` が作る名前とは異なることがあります
（中国語なら `源码`、`测试`、`构建` で、`lingfu new --lang zh` の `灵源`、`灵镜`、`灵碑` とは違います）。

### `lingfu check`、`fmt`、`translate`

これらのコマンドは存在しますが、`ling` に存在しないサブコマンド（`ling check`、`ling fmt`、
`ling <from> <to> <file>`）を呼び出します。`check` と `fmt` は何もせずに成功を報告し、
`translate` は `ling` の使い方を表示するだけです。プロジェクトの言語を変えるには `lingfu normalize` を使ってください。

### レジストリのコマンド

レジストリのデフォルトは `https://fu.ling-lang.org` です。別のレジストリを使うには `LINGFU_REGISTRY` を設定します。

| コマンド | 内容 |
|---|---|
| `login <api-key>` | キーを `~/.lingfu/credentials.toml` に保存します。 |
| `logout` | 保存した認証情報を削除します。 |
| `publish` | `ling.ignore` に書いたファイルを除いてプロジェクトを `.tar.gz` にまとめ、アップロードします。先に `login` が必要です。 |
| `search <query>` | レジストリを検索します。`--crates` を付けると crates.io を検索します。 |
| `add <package>` | パッケージを `./ling_modules/<name>/` にダウンロードし、読み込むための `use` 行を表示します。`--crates` を付けると `cargo add` を実行します。 |

### `lingfu doc`、`bench`、`update`

`doc` は `docs/book.toml` か `book/book.toml` があれば `mdbook build` を、なければ
`cargo doc --no-deps` を実行します。`bench` と `update` は `cargo bench` と `cargo update` を実行します。
Rust のクレートを含むプロジェクトでだけ役に立ちます。

### `lingfu seed`

git プロジェクトの暗号化された完全性フィンガープリントを作ります。追跡されているすべてのファイルの
BLAKE3 マークルルートを、再生できる `seed.wav` として書き出します。`--check` は保存したシードと
プロジェクトを比較します。git リポジトリーが必要です。

### `lingfu wizard`

対話式の質問（言語、プロジェクトの種類、名前など）に答えると、最後に `new` と同じようにプロジェクトを作ります。
ターミナルから入力を読むので、スクリプトにはできません。

## 終了ステータス

`lingfu` は成功すると 0 を返します。失敗すると `Error: <message>`（たとえば `build failed` や
`tests failed`）を表示して 1 を返します。
