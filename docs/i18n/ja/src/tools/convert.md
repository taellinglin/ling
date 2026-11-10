# アセット変換

`ling convert` は外部のアセット（3D モデル、サウンド、MIDI の曲、SVG の図）を、自己完結した
`.ling` ファイルに変換します。そのファイルをプログラムで `使う` と、Ling 自身の組み込み関数で
アセットが再構築されます。実行時にローダーや元のファイルは要りません。

```sh
ling convert model.glb                  # model.ling を書き出す（圧縮・可逆）
ling convert sfx.wav -o sfx.ling        # 出力先を指定する
ling convert logo.svg --no-compression  # 読んだり差分を取ったりできる普通の配列
lingfu convert song.mid                 # lingfu は ling convert に転送する
```

| オプション | 意味 |
|---|---|
| `-o <out.ling>`（または `--out`） | 出力パス。デフォルトは入力ファイル名に `.ling` を付けたもの。 |
| `--no-compression` | 数値を圧縮ブロブではなく普通のリストで書き出します。 |

## 各形式の変換結果

| 入力 | 生成される束縛 | プロファイル |
|---|---|---|
| `.gltf`, `.glb` | メッシュごとに `<name>_<mesh>_pos`、`_nrm`、`_uv`、`_idx` と、ワイヤーフレームを描く `draw_<name>_<mesh>(ox, oy, oz, scale)`。`draw_<name>(ox, oy, oz, scale)` はモデル全体を描きます。ノード名と変換はコメントとして残ります。 | `game` |
| `.wav`, `.ogg`, `.flac`, `.mp3` | `<name>_pcm`（モノラルのサンプル）、`<name>_rate`、`<name>_dur` | `audio` |
| `.mid`, `.midi` | `<name>_notes`（音符ごとに `[time, dur, midi, vel, channel]` を並べた平らなリスト）と `<name>_dur` | `audio` |
| `.svg` | `<name>_xy`（点）、`<name>_lens`（折れ線ごとの点の数）、`draw_<name>(ox, oy, scale)` | すべて |
| `.blend` | Blender で glTF に書き出してから、`.glb` と同じように変換します。Blender が `PATH` にあるか `$BLENDER` で指定されている必要があります。 | `game` |

`<name>` は入力ファイル名を、正しい識別子になるように整えたものです。SVG では、`<path>` のデータ
（`M`、`L`、`H`、`V`、`C`、`Q`、`Z`）と `<line>`、`<rect>`、`<polyline>`、`<polygon>` 要素が折れ線に
変換されます。それ以外の拡張子は `unsupported extension` で失敗します。

## 圧縮するか、しないか

デフォルトでは、長い数値のリストは zlib で圧縮して base64 にしたテキストとして可逆に保存され、
2 つの組み込み関数で復元されます。

- `blob_f32("...")` は数値のリスト（リトルエンディアンの 32 ビット浮動小数点数）に戻します。
- `blob_i32("...")` は整数のリスト（32 ビット整数）に戻します。インデックスや長さに使います。

`--no-compression` は同じ数値をリテラルのリスト `[ ... ]` として書き出します。ファイルは大きくなりますが、
読みやすくなります。数値が 8 個以下のリストは常に普通の形で書かれます。

## 例：SVG

`<rect>` と `<path>` が 1 つずつある小さな `logo.svg` があるとします。

```text
$ ling convert logo.svg
[convert] logo.svg → logo.ling  (1 KB, deflate+base64 lossless)
```

生成された `logo.ling` は次のように始まります。

```ling-module
# 2 polylines, 7 points
bind logo_xy = blob_f32("eNpdibENAAAIgzjFV/RmDzTRwaYDAwCRLF2Huvf/A/L0CM0=")
bind logo_lens = [2, 5]
```

そのあとに `draw_logo(ox, oy, scale)` 関数が続きます。プログラムからは次のように使います。

```ling-gui
使う "logo.ling"

束縛 開始 = 実行 {
    ウィンドウ開く(400, 400, "ロゴ")
    間 開いている() {
        塗り潰し(0, 0, 0)
        色設定(255, 255, 255)
        draw_logo(100, 100, 2.0)
        表示()
    }
}
```

## 補足

- オーディオは元のサンプルレートのまま、モノラルにミックスダウンして保存されます。生成されたファイルが
  持つのはサンプルだけで、再生や解析はプログラム側で行います。
- 生成されたコードでは、関数にタイ語のキーワード（`ฟังก์ชัน`）、束縛に `bind` が使われます。
  これは正常です。どの言語もどのファイルでも使えます。
- モデルの描画関数は `draw_line_3d` でワイヤーフレームを描きます。塗りつぶしやテクスチャ付きの
  メッシュについては [3D 形状とメッシュ](../reference/shapes.md)を参照してください。
