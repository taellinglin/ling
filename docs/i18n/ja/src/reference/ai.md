# AI と言語モデル

Ling には 3 つの規模の機械知能があります。どれも自分のマシン上で動き、ホスト型サービスに
何かを送ることはありません。

| 系統 | できること | 必要なもの |
|---|---|---|
| `nn_*` | ループで学習させる小さな順伝播ニューラルネットワーク | `ai` プロファイル |
| `bt_*` | ゲームキャラクター用のビヘイビアツリー | `ai` プロファイル |
| `dialog_*`（学習） | キャラクターの話し方を学ぶ小さな言語モデル | `ai` プロファイル |
| `stt_*` | Whisper による音声認識 | `ai` プロファイルと Whisper モデル |
| `llm_*` | ローカルの GGUF モデル（Qwen3、GLM-4）との対話 | オプトインの `llm` フィーチャーとモデルファイル |
| `vision_*` | 画像についての質問（Moondream2） | オプトインの `vision` フィーチャーとモデルファイル |

既定のインストール（`full`）には `ai` プロファイルが含まれます。`llm` と `vision` は `full` に
含まれないので、たとえば `cargo install ling-lang --features llm` のように明示的にインストール
してください（NVIDIA GPU なら `llm-cuda`）。これらを含まないビルドでは、`llm_*` と `vision_*` の
呼び出しは `()` に評価されます。

ほかの場所で名前を見かけることがあっても、次の 2 つは存在しません。`ai_chat` や `ai_embed` という
組み込み関数はなく、ホスト型モデル API のクライアントもありません。リモートのモデルを呼ぶには、
[Web サーバーと HTTP](../reference/web.md) の `http_post_async` を使い、返答を自分で解析してください。

## 約束事

- **ハンドルは数値です。** `nn_new`、`bt_build`、`dialog_new`、`llm_load`、`vision_load` は小さな
  数値を返します。読み込みに失敗すると `-1` を返します。不正なハンドルをほかの関数に渡すと、
  中立的な値（`[]`、`0`、`""`、`false`）が返り、クラッシュすることはありません。
- **数値のリスト**（`[0.5, 1]` など）は内部で 32 ビット浮動小数点数に変換されます。数値以外は
  読み飛ばされます。
- **長いジョブはポーリングします。** `stt_start`、`llm_generate_start`、`vision_analyze_start` は
  すぐにジョブ ID を返し、バックグラウンドスレッドで処理します。`sleep(ms)` を挟むループで
  様子を確認してください。

## ニューラルネットワーク: `nn_*`

全結合の順伝播ネットワークです。入力の数を指定し、層を追加し、例ごとに `nn_train` を 1 回呼びます。
学習は二乗誤差による素朴な確率的勾配降下法で、毎フレーム実行できるほど軽量です。`nn_new` に渡す
シードによって結果は再現可能になります。

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `nn_new(n_inputs, seed)` | `建神经网` | `ニューラル作成` | `신경망생성` | `สร้างโครงข่าย` | `n_inputs` 個の数値を取る空のネットワーク |
| `nn_dense(net, units, activation)` | `密集层` | `密層追加` | `밀집층` | `ชั้นหนาแน่น` | 全結合層を追加する |
| `nn_forward(net, inputs)` | `神经前向` | `順伝播` | `순전파` | `ส่งต่อโครงข่าย` | ネットワークを実行し、リストを返す |
| `nn_train(net, inputs, targets, rate)` | `训练网` | `ニューラル学習` | `신경망학습` | `ฝึกโครงข่าย` | 学習を 1 ステップ行い、損失を返す |
| `nn_save(net, path)` | `保存网` | `網保存` | `신경망저장` | `บันทึกโครงข่าย` | `LNN1` 形式で保存する |
| `nn_load(path)` | `载入网` | `網読込` | `신경망불러오기` | `โหลดโครงข่าย` | 保存したネットワークを読み込む。失敗時は `-1` |

### 構築と学習

- `nn_new(n_inputs, seed)`: どちらも既定値は 1 です。
- `nn_dense(net, units, activation)`: 活性化関数は `"relu"`（既定）、`"tanh"`、`"sigmoid"`、
  それ以外はすべて線形です。最後の層の幅が出力のサイズになります。
- `nn_forward(net, inputs)`: 短い入力リストはゼロで埋められ、長いものは切り詰められます。
- `nn_train(net, inputs, targets, rate)`: `rate` の既定値は 0.01 です。更新前の損失を返します。
  データセット全体を何周もループしてください。

このネットワークは XOR を学習します。

```ling
束縛 開始 = 実行 {
    束縛 網 = ニューラル作成(2, 7)
    密層追加(網, 8, "tanh")
    密層追加(網, 1, "sigmoid")
    繰 周回 の中 0..2000 {
        ニューラル学習(網, [0, 0], [0], 0.5)
        ニューラル学習(網, [0, 1], [1], 0.5)
        ニューラル学習(網, [1, 0], [1], 0.5)
        ニューラル学習(網, [1, 1], [0], 0.5)
    }
    印刷(順伝播(網, [0, 1])[0] > 0.9)
    印刷(順伝播(網, [1, 1])[0] < 0.1)
}
```

```output
true
true
```

### 保存

`nn_save(net, path)` は小さなリトルエンディアンのファイル（既定は `"model.lnn"`）を書き出し、
`true` を返します。`nn_load(path)` は新しいハンドルを返し、ファイルがないか不正なら `-1` を
返します。この形式にはチェックサムも署名もないので、信頼できない出所の `.lnn` ファイルは
読み込まないでください。

## ビヘイビアツリー: `bt_*`

ビヘイビアツリーはゲームキャラクターの意思決定の層です。小さなテキスト言語で書き、数値の事実
（体力、距離、弾薬）を集めた **ブラックボード** に対してティックします。ティックは選ばれた
アクションの名前を返し、それをどう実行するかはプログラムが決めます。

```text
selector {                  # 成功するまで子を順に試す
    sequence {              # すべての子が成功する必要がある
        ?enemy_visible > 0  # ブラックボードの事実に対する条件
        ?ammo >= 1
        !attack             # アクション
    }
    !patrol                 # 最後の手段
}
```

複合ノードは `selector`、`sequence`、`parallel`（すべての子が成功すれば成功）、`not <child>` です。
条件は `?key OP value` の形で、`>`、`<`、`>=`、`<=`、`==`、`!=` が使えます。一度も設定されていない
キーは `0` として読まれます。コメントは `#` で始めます。キーとアクションの名前には日本語も使えます。

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `bt_build(spec)` | `建行为树` | `行動木構築` | `행동트리구성` | `สร้างต้นไม้พฤติกรรม` | ツリーをコンパイルする。構文エラーなら `-1` |
| `bt_set(tree, key, value)` | `设事实` | `事実設定` | `사실설정` | `ตั้งข้อเท็จจริง` | ブラックボードの事実を設定する |
| `bt_tick(tree)` | `行为树滴答` | `行動木更新` | `행동트리틱` | `เดินต้นไม้พฤติกรรม` | 1 回実行し、アクション名か `""` を返す |
| `bt_status(tree)` | `行为树状态` | `行動木状態` | `행동트리상태` | `สถานะต้นไม้พฤติกรรม` | 直前の結果: `0` 失敗、`1` 成功、`2` 実行中 |

```ling
束縛 開始 = 実行 {
    束縛 頭脳 = 行動木構築("selector { sequence { ?敵が見える > 0 ?弾薬 >= 1 !攻撃 } sequence { ?敵が見える > 0 !逃走 } !巡回 }")
    印刷(行動木更新(頭脳))
    事実設定(頭脳, "敵が見える", 1)
    事実設定(頭脳, "弾薬", 5)
    印刷(行動木更新(頭脳))
    事実設定(頭脳, "弾薬", 0)
    印刷(行動木更新(頭脳))
}
```

```output
巡回
攻撃
逃走
```

## 対話の学習: `dialog_*`

これらの組み込み関数は、キャラクターに小さなニューラル言語モデルを与えます。学習させた台詞の
**リズムと語彙** を覚え、その話し方で新しい台詞を作ります。とても小さく（キロバイト単位、CPU 1 コア）、
コーパスが小さいと言い回しをそのまま繰り返します。会話 *ボックス* を表示する `dialog_show` 系は、
ゲームプロファイルの別機能です。

`dialog_learn` で 1 行ずつ与えるか、Ling Dialog 形式（`.lingdialog`）のファイルをまとめて与えます。
この形式では `= label` が会話の始まり、`speaker: text` が 1 回の発言です。

```text
= welcome
bram: welcome traveler rest your boots
hero: any news from the road
bram: bandits on the north pass
```

学習に使われるのは各発言のテキストだけです。テキストは小文字にされ、空白で単語に分けられます。

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `dialog_new(context, embed, hidden, seed)` | `建对话模型` | `対話モデル作成` | `대화모델생성` | `สร้างโมเดลสนทนา` | 未学習のモデル（既定 3、32、64、1） |
| `dialog_learn(model, text)` | `对话学习` | `対話学習` | `대화학습` | `เรียนรู้สนทนา` | 学習用テキストを 1 行追加する |
| `dialog_load(model, path)` | `对话载入` | `対話読込` | `대화불러오기` | `โหลดชุดสนทนา` | `.lingdialog` ファイルを追加し、追加した会話の数を返す |
| `dialog_train(model, epochs, rate)` | `对话训练` | `対話訓練` | `대화훈련` | `ฝึกสนทนา` | 学習する（既定 20、0.1）。最終的な損失を返す |
| `dialog_say(model, prompt, max_tokens, temperature)` | `对话生成` | `対話生成` | `대화생성` | `พูดสนทนา` | 台詞を 1 行生成する（既定 24、0.8） |
| `dialog_save(model, path)` | `对话存模` | `対話モデル保存` | `대화모델저장` | `บันทึกโมเดลสนทนา` | 学習済みモデルを保存する |
| `dialog_load_model(path)` | `对话载模` | `対話モデル読込` | `대화모델불러오기` | `โหลดโมเดลสนทนา` | 保存したモデルを読み込む。失敗時は `-1` |

`context` はモデルが参照する直前の単語数です。`temperature` が `0` 以下だと常に最も確からしい単語を
選び、値が大きいほど変化に富みます。シード、コーパス、引数が同じなら、出力は毎回同じです。
学習後にテキストを追加したら、もう一度学習させてください。

```ling
束縛 開始 = 実行 {
    束縛 店主 = 対話モデル作成(3, 16, 32, 1)
    対話学習(店主, "ようこそ 旅の方 火のそばで 靴を 休めて ください")
    対話学習(店主, "刃は 鋭く 財布は しっかり 持って おきなさい 旅の方")
    印刷(対話訓練(店主, 40, 0.1) < 2)
    印刷(長さ(対話生成(店主, "ようこそ 旅の方", 8, 0.7)) > 0)
}
```

```output
true
true
```

## 音声認識: `stt_*`

`stt_*` は Whisper を使い、音声ファイルをローカルで文字に起こします。`llm` フィーチャーがあり
モデルがインストールされていれば組み込みの Whisper エンジンを使い、そうでなければ Python の
[faster-whisper](https://github.com/SYSTRAN/faster-whisper) を実行します。Python 経由の準備は
一度だけ行います。

```sh
python -m venv venv
venv/bin/pip install faster-whisper
huggingface-cli download Systran/faster-whisper-small
```

`LING_STT_PYTHON` にその Python を設定するか、Ling の既定のフォルダ（Windows では
`%LOCALAPPDATA%\LingStation\stt\venv`、それ以外では `~/.local/share/lingstation/stt/venv`）に
venv を作ってください。モデルはローカルの Hugging Face キャッシュから読まれ、ネットワークは
使いません。

| 組み込み関数 | 用途 |
|---|---|
| `stt_available()` | Whisper エンジンが動かせれば `true` |
| `stt_models()` | インストール済みモデルを平らなリスト `[name, size_mb, ...]` で返す |
| `stt_start(audio, model, lang, words)` | ジョブを開始し ID を返す。開始できなければ `""` |
| `stt_poll(job)` | 0〜1 の進捗 |
| `stt_done(job)` | ジョブが終わっていれば `true` |
| `stt_error(job)` | `""`、または失敗した理由 |
| `stt_info(job)` | `"loaded small on cuda"` のような状態テキスト |
| `stt_language(job)` | 検出された言語コード |
| `stt_segments(job)` | これまでのセグメントの `[start, end, text, ...]` |
| `stt_words(job)` | これまでの単語の `[start, end, word, probability, ...]` |
| `stt_srt(job, max_chars)` | SubRip 字幕。各キューは最大 `max_chars` 文字（既定 42、`0` ならセグメント全体） |
| `stt_align(job, lyrics)` | 貼り付けたテキストの各行を聞き取った音声に合わせて時刻付けする: `[start, end, line, ...]` |
| `stt_align_srt(job, lyrics)` | 同じく、SubRip 字幕で返す |
| `stt_cancel(job)` | ジョブを止める |

どれにもローカライズされた別名はありません。`stt_start` は、モデル名（既定は `"small"`）、言語
（既定は `"auto"`、または `"en"` や `"ja"` などのコード）、単語ごとのタイムスタンプを記録するか
（既定は `true`）を取ります。時刻の単位は秒です。

```ling-gui
束縛 開始 = 実行 {
    もし !stt_available() {
        印刷("Whisper エンジンがインストールされていません")
    } 他 {
        束縛 ジョブ = stt_start("interview.wav", "small", "auto", 真)
        間 !stt_done(ジョブ) {
            印刷(stt_poll(ジョブ))
            スリープ(500)
        }
        もし stt_error(ジョブ) == "" {
            印刷(stt_language(ジョブ))
            write_file("interview.srt", stt_srt(ジョブ, 42))
        }
    }
}
```

## ローカル言語モデル: `llm_*`

`llm_*` は `ling` のプロセス内で対話モデルを動かします。量子化された **GGUF** チェックポイントと
その `tokenizer.json` を、CPU または CUDA GPU に読み込みます。対応するアーキテクチャは `qwen3` と
`glm4` です。オプトインの `llm` フィーチャー（GPU なら `llm-cuda`）が必要です。

生成がプログラムをブロックすることはありません。`llm_generate_start` はすぐにジョブ ID を返し、
`llm_generate_poll` でそれまでのテキストをポーリングします。これは特に Web ハンドラの中で重要です。
長い生成がすべての訪問者を待たせてしまうからです。

会話履歴は次の形式の 1 つの文字列で、ユーザーの発言で終わります。

```text
<|user|>
What is a lens?
<|assistant|>
A curved piece of glass that bends light.
<|user|>
And a mirror?
```

| 組み込み関数 | 用途 |
|---|---|
| `llm_load(gguf_path, tokenizer_path, device)` | モデルを読み込む。`device` が負なら CPU、それ以外は CUDA の番号。ハンドルか `-1` を返す |
| `llm_generate_start(model, system_prompt, history, max_tokens, temperature, top_p)` | 返答の生成を始める（既定 512、0.7、0.9）。ジョブ ID を返す |
| `llm_generate_poll(job)` | これまでに生成されたテキスト |
| `llm_generate_done(job)` | 終わっていれば `true` |
| `llm_job_status(job)` | `"pending"` または `"done"` |
| `llm_job_cancel(job)` | ジョブを完了済みにする（注意を参照） |
| `llm_job_tool_name(job)`、`llm_job_provide_tool_result(job, text)` | ツール呼び出し用に予約。未実装 |

どれにもローカライズされた別名はありません。

```ling-gui
束縛 開始 = 実行 {
    束縛 モデル = llm_load("models/Qwen3-0.6B-Q8_0.gguf", "models/tokenizer.json", 0 - 1)
    もし モデル < 0 {
        印刷("モデルを読み込めませんでした")
    } 他 {
        束縛 ジョブ = llm_generate_start(モデル, "一文で答えてください。", "<|user|>\nなぜ空は青いのですか？\n", 128, 0.7, 0.9)
        間 !llm_generate_done(ジョブ) {
            スリープ(250)
        }
        印刷(llm_generate_poll(ジョブ))
    }
}
```

注意:

- 読み込みではモデル全体をメモリに読むので、数秒かかることがあります。起動時に一度だけ読み込んで
  ください。
- 1 つのモデルで同時に実行できる生成は 1 つだけです。
- `llm_job_cancel` はジョブを完了済みにするだけです。生成は `max_tokens` に達するまでバックグラウンドで
  続くので、`max_tokens` は控えめにしてください。
- モデレーションの層はありません。信頼できないユーザーに見せる前に出力をフィルタし、モデルの出力を
  ファイル、SQL、コマンドに直接渡さないでください。

## 画像理解: `vision_*`

`vision_*` は、量子化された **Moondream2** モデルで画像についての質問に答えます。`llm_*` と同じく、
一度読み込んでからバックグラウンドのジョブをポーリングします。オプトインの `vision` フィーチャー
（`llm` を含みます）が必要です。文字認識（OCR）は行いません。

| 組み込み関数 | 用途 |
|---|---|
| `vision_load(gguf_path, tokenizer_path, device)` | モデルを読み込む。ハンドルか `-1` を返す |
| `vision_analyze_start(model, image_base64, question, max_tokens, temperature, top_p)` | ジョブを開始する（既定 `"Describe this image."`、256、0.7、0.9） |
| `vision_analyze_poll(job)` | これまでに生成されたテキスト |
| `vision_analyze_done(job)` | 成功・失敗にかかわらず終わっていれば `true` |
| `vision_analyze_error(job)` | `""`、または失敗した理由 |

Ling の文字列は生のバイト列を保持できないので、画像は PNG または JPEG のバイト列を **base64 文字列**
にして渡します。Web ページからは `canvas.toDataURL()` の結果を送り、`data:image/png;base64,` の
接頭辞を取り除いてください。

```ling-gui
束縛 開始 = 実行 {
    束縛 モデル = vision_load("models/moondream2-q4.gguf", "models/moondream-tokenizer.json", 0 - 1)
    束縛 ジョブ = vision_analyze_start(モデル, read_file("shot.b64"), "画面には何が映っていますか？", 128, 0.2, 0.9)
    間 !vision_analyze_done(ジョブ) {
        スリープ(250)
    }
    もし vision_analyze_error(ジョブ) == "" {
        印刷(vision_analyze_poll(ジョブ))
    }
}
```

## 関連項目

- ブロックせずにモデルをブラウザに提供する方法は [Web サーバーと HTTP](../reference/web.md) を参照してください。
