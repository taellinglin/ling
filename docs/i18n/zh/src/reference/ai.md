# AI 与语言模型

Ling 提供三种规模的机器智能。它们都在你自己的机器上运行，不会把任何数据发送给托管服务。

| 系列 | 作用 | 需要 |
|---|---|---|
| `nn_*` | 在循环中训练的小型前馈神经网络 | `ai` 安装配置 |
| `bt_*` | 游戏角色的行为树 | `ai` 安装配置 |
| `dialog_*`（学习） | 学习角色说话风格的微型语言模型 | `ai` 安装配置 |
| `stt_*` | 用 Whisper 做语音转文字 | `ai` 安装配置，外加 Whisper 模型 |
| `llm_*` | 与本地 GGUF 模型（Qwen3、GLM-4）对话 | 可选的 `llm` 特性，外加模型文件 |
| `vision_*` | 回答关于图像的问题（Moondream2） | 可选的 `vision` 特性，外加模型文件 |

默认安装（`full`）包含 `ai` 安装配置。`llm` 和 `vision` 不属于 `full`，需要显式安装，例如
`cargo install ling-lang --features llm`（NVIDIA GPU 用 `llm-cuda`）。在没有这些特性的构建中，调用
`llm_*` 和 `vision_*` 的结果是 `()`。

有两样东西并不存在，即使你可能在别处见过这些名字：没有 `ai_chat` 或 `ai_embed` 内置函数，也没有托管
模型 API 的客户端。要调用远程模型，请使用 [Web 服务器与 HTTP](../reference/web.md) 中的
`http_post_async`，并自己解析回复。

## 约定

- **句柄是数字。** `nn_new`、`bt_build`、`dialog_new`、`llm_load` 和 `vision_load` 返回一个小数字。
  加载失败返回 `-1`。把无效句柄传给其他函数会得到一个中性值（`[]`、`0`、`""` 或 `false`），不会崩溃。
- **数字列表**（如 `[0.5, 1]`）在内部转换为 32 位浮点数。非数字会被跳过。
- **长任务靠轮询。** `stt_start`、`llm_generate_start` 和 `vision_analyze_start` 立即返回任务 ID，并在
  后台线程中工作。请在循环中用 `sleep(ms)` 检查进度。

## 神经网络：`nn_*`

一个全连接的前馈网络：给出输入个数，添加若干层，然后对每个样本调用一次 `nn_train`。训练是普通的随机
梯度下降，使用平方误差损失，开销小到可以每帧运行。传给 `nn_new` 的种子让结果可以复现。

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `nn_new(n_inputs, seed)` | `建神经网` | `ニューラル作成` | `신경망생성` | `สร้างโครงข่าย` | 接受 `n_inputs` 个数字的空网络 |
| `nn_dense(net, units, activation)` | `密集层` | `密層追加` | `밀집층` | `ชั้นหนาแน่น` | 追加一个全连接层 |
| `nn_forward(net, inputs)` | `神经前向` | `順伝播` | `순전파` | `ส่งต่อโครงข่าย` | 运行网络；返回列表 |
| `nn_train(net, inputs, targets, rate)` | `训练网` | `ニューラル学習` | `신경망학습` | `ฝึกโครงข่าย` | 一步训练；返回损失 |
| `nn_save(net, path)` | `保存网` | `網保存` | `신경망저장` | `บันทึกโครงข่าย` | 以 `LNN1` 格式保存 |
| `nn_load(path)` | `载入网` | `網読込` | `신경망불러오기` | `โหลดโครงข่าย` | 加载已保存的网络，或返回 `-1` |

### 构建与训练

- `nn_new(n_inputs, seed)`：两个参数默认都是 1。
- `nn_dense(net, units, activation)`：激活函数有 `"relu"`（默认）、`"tanh"`、`"sigmoid"`，其他任何
  值都表示线性。最后一层的宽度就是输出大小。
- `nn_forward(net, inputs)`：输入列表太短会补零，太长会被截断。
- `nn_train(net, inputs, targets, rate)`：`rate` 默认 0.01。返回更新前的损失。请对整个数据集循环
  很多轮。

这个网络学习异或（XOR）：

```ling
令 始 = 执 {
    令 网络 = 建神经网(2, 7)
    密集层(网络, 8, "tanh")
    密集层(网络, 1, "sigmoid")
    历 轮次 于 0..2000 {
        训练网(网络, [0, 0], [0], 0.5)
        训练网(网络, [0, 1], [1], 0.5)
        训练网(网络, [1, 0], [1], 0.5)
        训练网(网络, [1, 1], [0], 0.5)
    }
    打印(神经前向(网络, [0, 1])[0] > 0.9)
    打印(神经前向(网络, [1, 1])[0] < 0.1)
}
```

```output
true
true
```

### 保存

`nn_save(net, path)` 写入一个小的小端序文件（默认 `"model.lnn"`），并返回 `true`。`nn_load(path)`
返回新句柄；文件不存在或无效时返回 `-1`。该格式没有校验和或签名，所以不要加载来源不可信的 `.lnn` 文件。

## 行为树：`bt_*`

行为树是游戏角色的决策层。你用一种小型文本语言编写它，并基于一块由数字事实（生命值、距离、弹药）组成的
**黑板**来运行（tick）它。每次 tick 返回所选动作的名字；如何执行这个动作由你的程序决定。

```text
selector {                  # 依次尝试子节点，直到有一个成功
    sequence {              # 所有子节点都必须成功
        ?看见敌人 > 0        # 对黑板事实的条件
        ?弹药 >= 1
        !攻击               # 动作
    }
    !巡逻                   # 兜底
}
```

组合节点有 `selector`、`sequence`、`parallel`（所有子节点成功才成功）和 `not <child>`。条件写作
`?key OP value`，可用 `>`、`<`、`>=`、`<=`、`==` 和 `!=`。从未设置过的键读作 `0`。注释以 `#` 开头。

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `bt_build(spec)` | `建行为树` | `行動木構築` | `행동트리구성` | `สร้างต้นไม้พฤติกรรม` | 编译行为树，语法错误时返回 `-1` |
| `bt_set(tree, key, value)` | `设事实` | `事実設定` | `사실설정` | `ตั้งข้อเท็จจริง` | 设置黑板事实 |
| `bt_tick(tree)` | `行为树滴答` | `行動木更新` | `행동트리틱` | `เดินต้นไม้พฤติกรรม` | 运行一次；返回动作名或 `""` |
| `bt_status(tree)` | `行为树状态` | `行動木状態` | `행동트리상태` | `สถานะต้นไม้พฤติกรรม` | 上次结果：`0` 失败，`1` 成功，`2` 运行中 |

```ling
令 始 = 执 {
    令 大脑 = 建行为树("selector { sequence { ?看见敌人 > 0 ?弹药 >= 1 !攻击 } sequence { ?看见敌人 > 0 !逃跑 } !巡逻 }")
    打印(行为树滴答(大脑))
    设事实(大脑, "看见敌人", 1)
    设事实(大脑, "弹药", 5)
    打印(行为树滴答(大脑))
    设事实(大脑, "弹药", 0)
    打印(行为树滴答(大脑))
}
```

```output
巡逻
攻击
逃跑
```

## 对话学习：`dialog_*`

这些内置函数给角色一个微型神经语言模型。它学习训练语句的**节奏和词汇**，并用这种语气说出新的句子。
它非常小（几 KB，单个 CPU 核心），在小语料上会重复其中的短语。用于显示对话*框*的 `dialog_show` 系列
是另一个属于 game 安装配置的功能。

可以用 `dialog_learn` 一次喂一行，也可以喂一个 Ling 对话格式（`.lingdialog`）的文件。在这种格式中，
`= 标签` 开始一段对话，`说话人: 文本` 是一个回合：

```text
= 欢迎
布拉姆: 欢迎，旅人，歇歇脚吧
英雄: 路上有什么消息吗
布拉姆: 北边山口有强盗
```

训练只使用每个回合的文本。文本会被转成小写，并按空格和标点切分成词（中文没有空格，所以两个标点之间的
短语会被当作一个词）。

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `dialog_new(context, embed, hidden, seed)` | `建对话模型` | `対話モデル作成` | `대화모델생성` | `สร้างโมเดลสนทนา` | 未训练的模型（默认 3、32、64、1） |
| `dialog_learn(model, text)` | `对话学习` | `対話学習` | `대화학습` | `เรียนรู้สนทนา` | 添加一行训练文本 |
| `dialog_load(model, path)` | `对话载入` | `対話読込` | `대화불러오기` | `โหลดชุดสนทนา` | 添加 `.lingdialog` 文件；返回添加的对话数 |
| `dialog_train(model, epochs, rate)` | `对话训练` | `対話訓練` | `대화훈련` | `ฝึกสนทนา` | 训练（默认 20、0.1）；返回最终损失 |
| `dialog_say(model, prompt, max_tokens, temperature)` | `对话生成` | `対話生成` | `대화생성` | `พูดสนทนา` | 生成一句话（默认 24、0.8） |
| `dialog_save(model, path)` | `对话存模` | `対話モデル保存` | `대화모델저장` | `บันทึกโมเดลสนทนา` | 保存训练好的模型 |
| `dialog_load_model(path)` | `对话载模` | `対話モデル読込` | `대화모델불러오기` | `โหลดโมเดลสนทนา` | 加载已保存的模型，或返回 `-1` |

`context` 是模型能看到的前文词数。`temperature` 为 `0` 或更小时总是选择最可能的词；值越大越多变。
对于给定的种子、语料和参数，每次运行的输出都相同。训练之后如果又添加了文本，请重新训练。

```ling
令 始 = 执 {
    令 角色 = 建对话模型(3, 16, 32, 1)
    对话学习(角色, "欢迎旅人，在炉火旁歇歇脚")
    对话学习(角色, "刀要磨快，钱包要看紧，旅人")
    打印(对话训练(角色, 40, 0.1) < 2)
    打印(长度(对话生成(角色, "欢迎旅人", 8, 0.7)) > 0)
}
```

```output
true
true
```

## 语音转文字：`stt_*`

`stt_*` 用 Whisper 在本地转写音频文件。当构建中有 `llm` 特性并且安装了模型时，它使用内置的 Whisper
引擎；否则在 Python 中运行 [faster-whisper](https://github.com/SYSTRAN/faster-whisper)。Python 方式只需
设置一次：

```sh
python -m venv venv
venv/bin/pip install faster-whisper
huggingface-cli download Systran/faster-whisper-small
```

把 `LING_STT_PYTHON` 设为这个 Python，或者在 Ling 的默认文件夹中创建 venv（Windows 上是
`%LOCALAPPDATA%\LingStation\stt\venv`，其他系统是 `~/.local/share/lingstation/stt/venv`）。模型从本地
Hugging Face 缓存读取，不使用网络。

| 内置函数 | 用途 |
|---|---|
| `stt_available()` | 能运行 Whisper 引擎时为 `true` |
| `stt_models()` | 已安装的模型，扁平列表 `[name, size_mb, ...]` |
| `stt_start(audio, model, lang, words)` | 启动任务；返回任务 ID，无法启动时为 `""` |
| `stt_poll(job)` | 进度，从 0 到 1 |
| `stt_done(job)` | 任务结束时为 `true` |
| `stt_error(job)` | `""`，或任务失败的原因 |
| `stt_info(job)` | 状态文本，如 `"loaded small on cuda"` |
| `stt_language(job)` | 检测到的语言代码 |
| `stt_segments(job)` | 目前为止各片段的 `[start, end, text, ...]` |
| `stt_words(job)` | 目前为止各词的 `[start, end, word, probability, ...]` |
| `stt_srt(job, max_chars)` | SubRip 字幕；每条最多 `max_chars` 个字符（默认 42，`0` 表示整段） |
| `stt_align(job, lyrics)` | 把粘贴文本的每一行与听到的内容对齐计时：`[start, end, line, ...]` |
| `stt_align_srt(job, lyrics)` | 同上，输出 SubRip 字幕 |
| `stt_cancel(job)` | 停止任务 |

它们都没有本地化别名。`stt_start` 接受模型名（默认 `"small"`）、语言（默认 `"auto"`，或 `"en"`、`"ja"`
这样的代码），以及是否记录词级时间戳（默认 `true`）。时间单位是秒。

```ling-gui
令 始 = 执 {
    若 !stt_available() {
        打印("没有安装 Whisper 引擎")
    } 否则 {
        令 任务 = stt_start("interview.wav", "small", "auto", 真)
        循 !stt_done(任务) {
            打印(stt_poll(任务))
            睡眠(500)
        }
        若 stt_error(任务) == "" {
            打印(stt_language(任务))
            write_file("interview.srt", stt_srt(任务, 42))
        }
    }
}
```

## 本地语言模型：`llm_*`

`llm_*` 在 `ling` 进程内运行对话模型。它加载一个量化的 **GGUF** 检查点及其 `tokenizer.json`，在 CPU
或 CUDA GPU 上运行。支持的架构是 `qwen3` 和 `glm4`。需要可选的 `llm` 特性（GPU 用 `llm-cuda`）。

生成永远不会阻塞你的程序：`llm_generate_start` 立即返回任务 ID，你轮询 `llm_generate_poll` 获取目前为止
的文本。这在 Web 处理函数中最为重要，因为一次很长的生成会卡住所有访问者。

对话历史是一个以下格式的字符串，以用户回合结尾：

```text
<|user|>
什么是透镜？
<|assistant|>
一块能使光线弯折的弧形玻璃。
<|user|>
那镜子呢？
```

| 内置函数 | 用途 |
|---|---|
| `llm_load(gguf_path, tokenizer_path, device)` | 加载模型；`device` 为负数表示 CPU，否则是 CUDA 编号。返回句柄或 `-1` |
| `llm_generate_start(model, system_prompt, history, max_tokens, temperature, top_p)` | 开始生成回复（默认 512、0.7、0.9）；返回任务 ID |
| `llm_generate_poll(job)` | 目前为止生成的文本 |
| `llm_generate_done(job)` | 完成时为 `true` |
| `llm_job_status(job)` | `"pending"` 或 `"done"` |
| `llm_job_cancel(job)` | 把任务标记为完成（见下方说明） |
| `llm_job_tool_name(job)`、`llm_job_provide_tool_result(job, text)` | 为工具调用预留；尚未实现 |

它们都没有本地化别名。

```ling-gui
令 始 = 执 {
    令 模型 = llm_load("models/Qwen3-0.6B-Q8_0.gguf", "models/tokenizer.json", 0 - 1)
    若 模型 < 0 {
        打印("无法加载模型")
    } 否则 {
        令 任务 = llm_generate_start(模型, "用一句话回答。", "<|user|>\n天空为什么是蓝色的？\n", 128, 0.7, 0.9)
        循 !llm_generate_done(任务) {
            睡眠(250)
        }
        打印(llm_generate_poll(任务))
    }
}
```

说明：

- 加载会把整个模型读入内存，可能需要几秒钟。请在启动时加载一次。
- 每个模型同一时间只运行一个生成任务。
- `llm_job_cancel` 只是把任务标记为完成。生成会在后台继续运行，直到达到 `max_tokens`，所以请把
  `max_tokens` 设得适中。
- 没有内容审核层。在把输出展示给不可信的用户之前请先过滤，并且绝不要把模型输出直接交给文件、SQL 或命令。

## 图像理解：`vision_*`

`vision_*` 用量化的 **Moondream2** 模型回答关于图像的问题。它的用法与 `llm_*` 相同：加载一次，然后轮询
后台任务。需要可选的 `vision` 特性（其中包含 `llm`）。它不做文字识别（OCR）。

| 内置函数 | 用途 |
|---|---|
| `vision_load(gguf_path, tokenizer_path, device)` | 加载模型；返回句柄或 `-1` |
| `vision_analyze_start(model, image_base64, question, max_tokens, temperature, top_p)` | 启动任务（默认 `"Describe this image."`、256、0.7、0.9） |
| `vision_analyze_poll(job)` | 目前为止生成的文本 |
| `vision_analyze_done(job)` | 结束时为 `true`，无论成功与否 |
| `vision_analyze_error(job)` | `""`，或失败的原因 |

图像以 PNG 或 JPEG 字节的 **base64 字符串**传入，因为 Ling 字符串不能保存原始字节。在网页中，可以提交
`canvas.toDataURL()` 的内容，并去掉 `data:image/png;base64,` 前缀。

```ling-gui
令 始 = 执 {
    令 模型 = vision_load("models/moondream2-q4.gguf", "models/moondream-tokenizer.json", 0 - 1)
    令 任务 = vision_analyze_start(模型, read_file("shot.b64"), "屏幕上显示的是什么？", 128, 0.2, 0.9)
    循 !vision_analyze_done(任务) {
        睡眠(250)
    }
    若 vision_analyze_error(任务) == "" {
        打印(vision_analyze_poll(任务))
    }
}
```

## 另请参阅

- [Web 服务器与 HTTP](../reference/web.md)：在不阻塞的情况下把模型提供给浏览器。
