# AI and Language Models

Ling has machine intelligence at three sizes. Each runs on your own machine; nothing is sent
to a hosted service.

| Family | What it does | Needs |
|---|---|---|
| `nn_*` | Small feed-forward neural networks you train in a loop | `ai` profile |
| `bt_*` | Behaviour trees for game characters | `ai` profile |
| `dialog_*` (learning) | A tiny language model that learns a character's voice | `ai` profile |
| `stt_*` | Speech to text with Whisper | `ai` profile, plus a Whisper model |
| `llm_*` | Chat with a local GGUF model (Qwen3, GLM-4) | opt-in `llm` feature, plus a model file |
| `vision_*` | Questions about an image (Moondream2) | opt-in `vision` feature, plus a model file |

The default install (`full`) includes the `ai` profile. `llm` and `vision` are not part of
`full`; install them explicitly, for example `cargo install ling-lang --features llm`
(`llm-cuda` for an NVIDIA GPU). In a build without them, calls to `llm_*` and `vision_*`
evaluate to `()`.

Two things don't exist, even though you may see the names elsewhere: there is no `ai_chat` or
`ai_embed` builtin, and there is no client for hosted model APIs. To call a remote model, use
`http_post_async` from [Web Server and HTTP](../reference/web.md) and parse the reply yourself.

## Conventions

- **Handles are numbers.** `nn_new`, `bt_build`, `dialog_new`, `llm_load` and `vision_load`
  return a small number. A failed load returns `-1`. A bad handle passed to any other function
  returns a neutral value (`[]`, `0`, `""` or `false`) and never crashes.
- **Number lists** such as `[0.5, 1]` are converted to 32-bit floats inside. Non-numbers are
  skipped.
- **Long jobs are polled.** `stt_start`, `llm_generate_start` and `vision_analyze_start` return
  a job id at once and work on a background thread. Check on them in a loop with `sleep(ms)`.

## Neural networks: `nn_*`

A dense feed-forward network: give the number of inputs, add layers, then call `nn_train` once
per example. Training is plain stochastic gradient descent with a squared-error loss, cheap
enough to run every frame. The seed you pass to `nn_new` makes results reproducible.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `nn_new(n_inputs, seed)` | `建神经网` | `ニューラル作成` | `신경망생성` | `สร้างโครงข่าย` | Empty network taking `n_inputs` numbers |
| `nn_dense(net, units, activation)` | `密集层` | `密層追加` | `밀집층` | `ชั้นหนาแน่น` | Append a fully connected layer |
| `nn_forward(net, inputs)` | `神经前向` | `順伝播` | `순전파` | `ส่งต่อโครงข่าย` | Run the network; returns a list |
| `nn_train(net, inputs, targets, rate)` | `训练网` | `ニューラル学習` | `신경망학습` | `ฝึกโครงข่าย` | One training step; returns the loss |
| `nn_save(net, path)` | `保存网` | `網保存` | `신경망저장` | `บันทึกโครงข่าย` | Save in the `LNN1` format |
| `nn_load(path)` | `载入网` | `網読込` | `신경망불러오기` | `โหลดโครงข่าย` | Load a saved network, or `-1` |

### Building and training

- `nn_new(n_inputs, seed)`: both default to 1.
- `nn_dense(net, units, activation)`: activations are `"relu"` (default), `"tanh"`,
  `"sigmoid"`, or anything else for linear. The last layer's width is the output size.
- `nn_forward(net, inputs)`: shorter input lists are padded with zeros, longer ones cut.
- `nn_train(net, inputs, targets, rate)`: `rate` defaults to 0.01. Returns the loss before the
  update. Loop over your whole data set for many rounds.

This network learns XOR:

```ling
bind start = do {
    bind net = nn_new(2, 7)
    nn_dense(net, 8, "tanh")
    nn_dense(net, 1, "sigmoid")
    for round in 0..2000 {
        nn_train(net, [0, 0], [0], 0.5)
        nn_train(net, [0, 1], [1], 0.5)
        nn_train(net, [1, 0], [1], 0.5)
        nn_train(net, [1, 1], [0], 0.5)
    }
    print(nn_forward(net, [0, 1])[0] > 0.9)
    print(nn_forward(net, [1, 1])[0] < 0.1)
}
```

```output
true
true
```

### Saving

`nn_save(net, path)` writes a small little-endian file (default `"model.lnn"`) and returns
`true`. `nn_load(path)` returns a new handle, or `-1` if the file is missing or invalid. The
format has no checksum or signature, so don't load `.lnn` files from untrusted sources.

## Behaviour trees: `bt_*`

A behaviour tree is the decision layer of a game character. You write it in a small text
language and tick it against a **blackboard** of number facts (health, distance, ammo). A tick
returns the name of the chosen action; your program decides how to carry it out.

```text
selector {                  # try children in order until one succeeds
    sequence {              # every child must succeed
        ?enemy_visible > 0  # condition on a blackboard fact
        ?ammo >= 1
        !attack             # action
    }
    !patrol                 # fallback
}
```

Composites are `selector`, `sequence`, `parallel` (succeeds if all children do) and
`not <child>`. Conditions are `?key OP value` with `>`, `<`, `>=`, `<=`, `==` and `!=`. A key
that was never set reads as `0`. Comments start with `#`.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `bt_build(spec)` | `建行为树` | `行動木構築` | `행동트리구성` | `สร้างต้นไม้พฤติกรรม` | Compile a tree, or `-1` on a syntax error |
| `bt_set(tree, key, value)` | `设事实` | `事実設定` | `사실설정` | `ตั้งข้อเท็จจริง` | Set a blackboard fact |
| `bt_tick(tree)` | `行为树滴答` | `行動木更新` | `행동트리틱` | `เดินต้นไม้พฤติกรรม` | Run once; returns the action name or `""` |
| `bt_status(tree)` | `行为树状态` | `行動木状態` | `행동트리상태` | `สถานะต้นไม้พฤติกรรม` | Last result: `0` failure, `1` success, `2` running |

```ling
bind start = do {
    bind brain = bt_build("selector { sequence { ?enemy_visible > 0 ?ammo >= 1 !attack } sequence { ?enemy_visible > 0 !flee } !patrol }")
    print(bt_tick(brain))
    bt_set(brain, "enemy_visible", 1)
    bt_set(brain, "ammo", 5)
    print(bt_tick(brain))
    bt_set(brain, "ammo", 0)
    print(bt_tick(brain))
}
```

```output
patrol
attack
flee
```

## Dialog learning: `dialog_*`

These builtins give a character a miniature neural language model. It learns the **cadence and
vocabulary** of the lines you train it on and writes fresh lines in that voice. It is tiny
(kilobytes, one CPU core) and will repeat phrases from a small corpus. The `dialog_show` family
of dialog *boxes* is a different, game-profile feature.

Feed it lines one at a time with `dialog_learn`, or a whole file in the Ling Dialog format
(`.lingdialog`), where `= label` starts a conversation and `speaker: text` is one turn:

```text
= welcome
bram: welcome traveler rest your boots
hero: any news from the road
bram: bandits on the north pass
```

Only the text of each turn is used for training. Text is lower-cased and split into words.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `dialog_new(context, embed, hidden, seed)` | `建对话模型` | `対話モデル作成` | `대화모델생성` | `สร้างโมเดลสนทนา` | Untrained model (defaults 3, 32, 64, 1) |
| `dialog_learn(model, text)` | `对话学习` | `対話学習` | `대화학습` | `เรียนรู้สนทนา` | Add one line of training text |
| `dialog_load(model, path)` | `对话载入` | `対話読込` | `대화불러오기` | `โหลดชุดสนทนา` | Add a `.lingdialog` file; returns conversations added |
| `dialog_train(model, epochs, rate)` | `对话训练` | `対話訓練` | `대화훈련` | `ฝึกสนทนา` | Train (defaults 20, 0.1); returns the final loss |
| `dialog_say(model, prompt, max_tokens, temperature)` | `对话生成` | `対話生成` | `대화생성` | `พูดสนทนา` | Generate a line (defaults 24, 0.8) |
| `dialog_save(model, path)` | `对话存模` | `対話モデル保存` | `대화모델저장` | `บันทึกโมเดลสนทนา` | Save the trained model |
| `dialog_load_model(path)` | `对话载模` | `対話モデル読込` | `대화모델불러오기` | `โหลดโมเดลสนทนา` | Load a saved model, or `-1` |

`context` is how many previous words the model sees. A `temperature` of `0` or less always
picks the most likely word; higher values are more varied. For a given seed, corpus and
arguments, the output is the same every run. If you add text after training, train again.

```ling
bind start = do {
    bind npc = dialog_new(3, 16, 32, 1)
    dialog_learn(npc, "welcome traveler rest your boots by the fire")
    dialog_learn(npc, "keep your blade sharp and your purse close traveler")
    print(dialog_train(npc, 40, 0.1) < 2)
    print(len(dialog_say(npc, "welcome traveler", 8, 0.7)) > 0)
}
```

```output
true
true
```

## Speech to text: `stt_*`

`stt_*` transcribes audio files with Whisper, locally. It uses the built-in Whisper engine when
the `llm` feature is present and a model is installed; otherwise it runs
[faster-whisper](https://github.com/SYSTRAN/faster-whisper) in Python. To set up the Python
route once:

```sh
python -m venv venv
venv/bin/pip install faster-whisper
huggingface-cli download Systran/faster-whisper-small
```

Set `LING_STT_PYTHON` to that Python, or create the venv in Ling's default folder
(`%LOCALAPPDATA%\LingStation\stt\venv` on Windows, `~/.local/share/lingstation/stt/venv`
elsewhere). Models are read from the local Hugging Face cache; no network is used.

| Builtin | Purpose |
|---|---|
| `stt_available()` | `true` if a Whisper engine can run |
| `stt_models()` | Installed models as a flat list `[name, size_mb, ...]` |
| `stt_start(audio, model, lang, words)` | Start a job; returns its id, or `""` if it can't start |
| `stt_poll(job)` | Progress from 0 to 1 |
| `stt_done(job)` | `true` when the job has finished |
| `stt_error(job)` | `""`, or why the job failed |
| `stt_info(job)` | Status text, such as `"loaded small on cuda"` |
| `stt_language(job)` | Detected language code |
| `stt_segments(job)` | `[start, end, text, ...]` for the segments so far |
| `stt_words(job)` | `[start, end, word, probability, ...]` for the words so far |
| `stt_srt(job, max_chars)` | SubRip subtitles; cues of at most `max_chars` characters (default 42, `0` = whole segments) |
| `stt_align(job, lyrics)` | Time each line of pasted text against what was heard: `[start, end, line, ...]` |
| `stt_align_srt(job, lyrics)` | The same, as SubRip subtitles |
| `stt_cancel(job)` | Stop the job |

None of them has a localized alias. `stt_start` takes a model name (default `"small"`), a
language (`"auto"` by default, or a code such as `"en"` or `"ja"`), and whether to record word
timestamps (default `true`). Times are in seconds.

```ling-gui
bind start = do {
    if !stt_available() {
        print("no Whisper engine installed")
    } else {
        bind job = stt_start("interview.wav", "small", "auto", true)
        while !stt_done(job) {
            print(stt_poll(job))
            sleep(500)
        }
        if stt_error(job) == "" {
            print(stt_language(job))
            write_file("interview.srt", stt_srt(job, 42))
        }
    }
}
```

## Local language models: `llm_*`

`llm_*` runs a chat model inside the `ling` process. It loads a quantised **GGUF** checkpoint
plus its `tokenizer.json`, on the CPU or a CUDA GPU. Supported architectures are `qwen3` and
`glm4`. Needs the opt-in `llm` feature (`llm-cuda` for GPU).

Generation never blocks your program: `llm_generate_start` returns a job id at once, and you
poll `llm_generate_poll` for the text so far. This matters most inside web handlers, where a
long generation would stall every visitor.

Conversation history is one string in this format, ending with a user turn:

```text
<|user|>
What is a lens?
<|assistant|>
A curved piece of glass that bends light.
<|user|>
And a mirror?
```

| Builtin | Purpose |
|---|---|
| `llm_load(gguf_path, tokenizer_path, device)` | Load a model; `device` negative for CPU, else a CUDA index. Returns a handle or `-1` |
| `llm_generate_start(model, system_prompt, history, max_tokens, temperature, top_p)` | Start a reply (defaults 512, 0.7, 0.9); returns a job id |
| `llm_generate_poll(job)` | The text generated so far |
| `llm_generate_done(job)` | `true` when finished |
| `llm_job_status(job)` | `"pending"` or `"done"` |
| `llm_job_cancel(job)` | Mark the job done (see the note) |
| `llm_job_tool_name(job)`, `llm_job_provide_tool_result(job, text)` | Reserved for tool calling; not implemented yet |

None of them has a localized alias.

```ling-gui
bind start = do {
    bind model = llm_load("models/Qwen3-0.6B-Q8_0.gguf", "models/tokenizer.json", 0 - 1)
    if model < 0 {
        print("could not load the model")
    } else {
        bind job = llm_generate_start(model, "Answer in one sentence.", "<|user|>\nWhy is the sky blue?\n", 128, 0.7, 0.9)
        while !llm_generate_done(job) {
            sleep(250)
        }
        print(llm_generate_poll(job))
    }
}
```

Notes:

- Loading reads the whole model into memory and can take seconds. Load once at start-up.
- Only one generation runs at a time per model.
- `llm_job_cancel` only marks the job as done. The generation keeps running in the background
  until it reaches `max_tokens`, so keep `max_tokens` modest.
- There is no moderation layer. Filter output before you show it to untrusted users, and never
  pass model output straight to files, SQL or commands.

## Image understanding: `vision_*`

`vision_*` answers questions about an image with a quantised **Moondream2** model. It works
like `llm_*`: load once, then poll a background job. Needs the opt-in `vision` feature (which
includes `llm`). It does not do text recognition (OCR).

| Builtin | Purpose |
|---|---|
| `vision_load(gguf_path, tokenizer_path, device)` | Load the model; returns a handle or `-1` |
| `vision_analyze_start(model, image_base64, question, max_tokens, temperature, top_p)` | Start a job (defaults `"Describe this image."`, 256, 0.7, 0.9) |
| `vision_analyze_poll(job)` | The text produced so far |
| `vision_analyze_done(job)` | `true` when finished, successfully or not |
| `vision_analyze_error(job)` | `""`, or why it failed |

The image goes in as a **base64 string** of PNG or JPEG bytes, because Ling strings can't hold
raw bytes. From a web page, post `canvas.toDataURL()` and strip the `data:image/png;base64,`
prefix.

```ling-gui
bind start = do {
    bind model = vision_load("models/moondream2-q4.gguf", "models/moondream-tokenizer.json", 0 - 1)
    bind job = vision_analyze_start(model, read_file("shot.b64"), "What is on the screen?", 128, 0.2, 0.9)
    while !vision_analyze_done(job) {
        sleep(250)
    }
    if vision_analyze_error(job) == "" {
        print(vision_analyze_poll(job))
    }
}
```

## See also

- [Web Server and HTTP](../reference/web.md) for serving a model to a browser without blocking.
