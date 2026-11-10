# クレート概要

Ling は Rust で書かれていて、複数のクレートからなるワークスペースに分かれています。メインクレートの
`ling-lang` には、コンパイラーのフロントエンド、インタプリタ、JIT ドライバー、`ling` コマンドが入っています。
下のクレートは言語の部品と組み込み関数のライブラリを提供します。このページが必要になるのは、
自分の Rust コードからこれらのクレートを使いたいときか、どのクレートがどの組み込み関数を支えているかを
知りたいときだけです。

下のバージョンは執筆時点のリポジトリーのものです。ほとんどのクレートは Ling の `2030.x` の番号に従いますが、
`ling-http` と `ling-web` は独自の番号を持っています。

## メインクレート

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-lang` | 2030.1.53 | 言語本体：字句解析器、構文解析器、インタプリタ、Cranelift JIT と AOT、組み込み関数。ライブラリ名は `ling`、実行ファイルは `ling`、`lingc`、`lx`、`ling-repl`。 |
| `lingfu`（`crates/ling-fu`） | 2030.1.19 | プロジェクト管理ツール：`lingfu new`、`run`、`build`、`normalize` など。[lingfu](../tools/lingfu.md) を参照。 |

## コンパイラー

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-core` | 2030.0.9 | コアの型、エラー、コンパイラー基盤。 |
| `ling-lex` | 2030.0.8 | 字句解析と多言語トークナイズ。 |
| `ling-ast` | 2030.0.8 | 抽象構文木の型。 |
| `ling-polyglot` | 2030.0.8 | 16 以上の人間の言語でキーワードと組み込み関数の名前を提供するレキシコンシステム。 |
| `ling-mir` | 2030.0.12 | コード生成器のための中間表現（MIR）。 |
| `ling-codegen` | 2030.0.13 | コード生成バックエンド（バイトコード、WASM、ネイティブ）。 |
| `ling-macro` | 2030.0.8 | Ling エコシステム向けの手続きマクロ。 |
| `ling-runtime` | 2030.0.8 | ランタイム支援：ガベージコレクター、アロケーター、標準ライブラリ。 |
| `ling-wasm` | 2030.0.16 | `ling build --platform web` が使う WebGL2 WebAssembly ランタイム。 |

## グラフィックス、UI、入力

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-graphics` | 2030.0.13 | 3D/4D レンダリング、ジオメトリー、アニメーション、フォントツール。 |
| `ling-gl` | 2030.0.10 | 投影シーン用の GPU ラスタライザー（wgpu：Vulkan、Metal、DX12。wasm では WebGL2）。 |
| `ling-gpu` | 2030.0.10 | GPU 計算（cudarc 経由の CUDA、CPU フォールバック付き）。 |
| `ling-ui` | 2030.0.9 | 保持モードのウィジェットとフレックスレイアウト（`ui_*`）。 |
| `ling-animation` | 2030.0.8 | Anima：統一されたアニメーションシステム。1 つのリグをどんな対象にも。 |
| `ling-input` | 2030.0.9 | Sensorium：ゲームパッド、ジョイスティック、触覚フィードバック、モーション、XR コントローラー、タッチ操作。 |
| `ling-paint` | 2030.0.1 | LingAnime ペイントエンジン：レイヤー、ブラシ、ベクター、ハーフトーン、2.5D リグ、ペンタブレット（`art_*`）。 |
| `ling-icon` | 2030.0.9 | SVG/PNG アイコンをマルチ解像度の `.ico` にラスタライズし、Windows 実行ファイルに埋め込みます（`ling build --icon`）。 |

## ゲームと物理

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-game` | 2030.1.9 | ゲームエンジン：ECS、物理、シーン管理。 |
| `ling-physics` | 2030.1.18 | 物理、ワールド生成、双曲幾何、glTF の読み込み。 |
| `ling-body` | 2030.0.2 | 音響ボディ：人型 SDF の骨組み、FDTD 波動ソルバー、膜、メッシュと WAV のコーデック。 |
| `ling-emu` | 2030.0.1 | ゲーム機エミュレーションのホスト（`emu_*`）：SNES と N64 のコア。 |

## オーディオと音楽

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-audio` | 2030.0.13 | 4D 定位オーディオ合成と WAV の BGM。 |
| `ling-music` | 2030.0.9 | 音楽のデコード、BPM とキーの解析、合成、リズムゲームとカラオケのツール。 |
| `ling-mic` | 2030.0.12 | マイク入力とリアルタイムの音声キャプチャー。 |
| `ling-daw` | 2030.0.2 | LingStationX を支える DAW エンジン：シンセ、ドラム、エフェクト、ミキサー、シーケンサー。 |
| `ling-osdaw` | 2030.0.2 | LingOS 版 LingStation：`no_std` のトラッカー型シーケンサーとシンセサイザー。 |

## ネットワーク、Web、セキュリティ

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-http` | 0.1.8 | axum 上の HTTPS/JSON/SQLite フレームワーク。Ling の `http_serve` はこの上で動きます。[ling-http と ling-web](web.md) を参照。 |
| `ling-web` | 0.1.9 | `ling-http` の上に載る Flask 風のレイヤー：セッション、フラッシュメッセージ、テンプレート、静的ファイル、ブループリント、CSRF。 |
| `ling-net` | 2030.0.9 | 非同期ネットワーク：HTTP、WebSocket、QUIC、ピアツーピア。 |
| `ling-crypto` | 2030.1.13 | 耐量子暗号と古典暗号：ML-KEM-768、ハイブリッド KEM、AES-GCM、XChaCha20、BLAKE3、SHA-3、Argon2id、Shamir、Schnorr ゼロ知識証明、VRF。`no_std` 対応。 |

## AI

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-ai` | 2030.0.10 | テンソル、ニューラルネットワーク、LLM 推論。 |
| `ling-llm` | 2030.0.2 | Qwen3 向けの `no_std` GGUF 推論エンジン。OS なしで CPU 上で動きます（LingOS 用）。 |
| `ling-diffusion` | 2030.0.1 | プロセス内の Stable Diffusion（SD 1.x / SDXL）。`sd_*` 組み込み関数を支えます。 |

## LingOS

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-kernel` | 2030.1.37 | 最小限のカーネルランタイム：`no_std` の CPU 組み込み命令、VGA、シリアル、ブート。 |
| `ling-user` | 2030.1.4 | LingOS 上の Ling プログラム用ユーザーランドランタイム。 |

## バインディング

| クレート | バージョン | 内容 |
|---|---|---|
| `ling-py` | 2030.0.8 | `ling-audio`、`ling-physics`、`ling-net` の Python バインディング。 |

## どのプロファイルがどのクレートを含むか

[インストールプロファイル](../getting-started/install.md)によって、どのクレートが `ling` にコンパイルされるかが決まります。

| プロファイル | クレート |
|---|---|
| `web` | `ling-http`、`ling-crypto`、`ling-icon`、`lingfu` |
| `privacy` | `web` に加えて `ling-net`、`ling-ui` |
| `game` | `ling-physics`、`ling-gpu`、`ling-game`、`ling-animation`、`ling-input`、`ling-body`、`ling-ui`、`ling-icon` |
| `ai` | `ling-ai`、`ling-music` |
| `audio` | `ling-audio`、`ling-music`、`ling-mic`、`ling-daw` |
| `art`（`full` に含まれる） | `ling-paint` |
