# Ling 2030 Game-AI Toolkit (`ling-ai`)

Fast, dependency-free, deterministic AI primitives for games — usable from Ling
in all five languages. Three pillars:

1. **Optimized neural net** (`nn_*`) — a cache-friendly feed-forward network with
   SGD back-propagation. Flat `Vec<f32>` weight buffers, no per-inference heap
   allocation, autovectorized inner products.
2. **Behavior trees** (`bt_*`) — the classic NPC decision structure, authored as a
   tiny text DSL and ticked against a numeric blackboard.
3. **Miniature dialog LLM** (`dialog_*`) — a trainable neural language model that
   learns a character's voice from a [`.lingdialog`](LING_DIALOG_STANDARD.md)
   corpus and samples fresh lines.

Everything is pure Rust with a seeded RNG, so AI is **reproducible** frame to
frame — essential for replays, lockstep multiplayer, and debugging. Models
serialize to compact binary (`nn_save` / `dialog_save`) for the asset pipeline.

Try it: `ling examples/ai/game_ai.ling`

---

## 1. Neural network

A handle-based API. `nn_new` returns an integer handle; pass it back to every
other call.

```ling
bind net = nn_new(2, 7)        // 2 inputs, RNG seed 7  -> handle
nn_dense(net, 8, "tanh")       // append hidden layer (relu|tanh|sigmoid|linear)
nn_dense(net, 1, "sigmoid")    // append output layer

// one SGD step (inputs, targets, learning-rate) -> loss
bind loss = nn_train(net, [0, 1], [1], 0.5)

bind y = nn_forward(net, [0, 1])   // inference -> list of outputs
nn_save(net, "brain.lnn")          // persist
bind net2 = nn_load("brain.lnn")   // restore -> handle
```

| Builtin | Signature | Returns |
|---------|-----------|---------|
| `nn_new`     | `(inputs[, seed])`               | handle |
| `nn_dense`   | `(handle, units[, activation])`  | — |
| `nn_forward` | `(handle, [inputs])`             | `[outputs]` |
| `nn_train`   | `(handle, [inputs], [targets][, lr])` | loss |
| `nn_save`    | `(handle, path)`                 | bool |
| `nn_load`    | `(path)`                         | handle (`-1` on error) |

Inputs of the wrong width are zero-padded / truncated rather than crashing the
interpreter.

## 2. Behavior trees

Author the tree as a string and tick it. Conditions (`?key OP value`) read the
blackboard; actions (`!name`) are returned as the chosen action for the frame.

```ling
bind brain = bt_build("selector { sequence { ?enemy > 0 ?ammo >= 1 !attack } !patrol }")

bt_set(brain, "enemy", 1)      // set a perception fact
bt_set(brain, "ammo", 3)
bind action = bt_tick(brain)   // -> "attack"   (run once per frame)
bind st = bt_status(brain)     // 0 fail / 1 success / 2 running
```

**DSL**

- Composites: `selector { … }` (first child that succeeds), `sequence { … }`
  (all must succeed), `parallel { … }` (tick all, succeed if all do).
- Decorator: `not <child>` (inverts success/failure).
- Leaves: `?key OP value` conditions (`> < >= <= == !=`) and `!action` actions.
- `#` begins a comment.

| Builtin | Signature | Returns |
|---------|-----------|---------|
| `bt_build`  | `(dsl_string)`        | handle (`-1` on parse error) |
| `bt_set`    | `(handle, key, value)`| — |
| `bt_tick`   | `(handle)`            | chosen action name (`""` if none) |
| `bt_status` | `(handle)`            | `0`/`1`/`2` |

## 3. Miniature dialog LLM

A neural probabilistic language model: a trainable embedding table feeds a context
window into the MLP, predicting the next token with softmax + cross-entropy.
Tiny enough that every NPC carries its own. See the
[Ling Dialog Standard](LING_DIALOG_STANDARD.md) for the corpus format.

```ling
bind npc   = dialog_new(3, 32, 64, 1)                 // ctx, embed, hidden, seed
bind lines = dialog_load(npc, "npc/bram.lingdialog")  // or: dialog_learn(npc, "a line")
bind loss  = dialog_train(npc, 80, 0.1)               // epochs, lr  -> loss
print(dialog_say(npc, "welcome", 12, 0.7))            // prompt, max-tokens, temperature
dialog_save(npc, "npc/bram.llm")
bind npc2 = dialog_load_model("npc/bram.llm")
```

`temperature` ≤ 0 is greedy (deterministic); higher is more varied (top-k 40,
nucleus 0.9 sampling under the hood).

| Builtin | Signature | Returns |
|---------|-----------|---------|
| `dialog_new`        | `([ctx, embed, hidden, seed])`        | handle |
| `dialog_learn`      | `(handle, text)`                      | — |
| `dialog_load`       | `(handle, path)`                      | lines added (`-1` on error) |
| `dialog_train`      | `(handle[, epochs, lr])`              | loss |
| `dialog_say`        | `(handle, prompt[, max_tokens, temp])`| reply text |
| `dialog_save`       | `(handle, path)`                      | bool |
| `dialog_load_model` | `(path)`                              | handle (`-1` on error) |

---

## Multilingual names

Every builtin is callable by its native name in all five Ling languages. Source
written in one language can be translated to another with
`lingfu normalize <lang>`.

| English | 中文 (zh) | 日本語 (ja) | 한국어 (ko) | ไทย (th) |
|---------|-----------|-------------|-------------|----------|
| `nn_new`            | `建神经网`     | `ニューラル作成`   | `신경망생성`       | `สร้างโครงข่าย` |
| `nn_dense`          | `密集层`       | `密層追加`       | `밀집층`         | `ชั้นหนาแน่น` |
| `nn_forward`        | `神经前向`     | `順伝播`        | `순전파`         | `ส่งต่อโครงข่าย` |
| `nn_train`          | `训练网`       | `ニューラル学習`   | `신경망학습`       | `ฝึกโครงข่าย` |
| `nn_save`           | `保存网`       | `網保存`        | `신경망저장`       | `บันทึกโครงข่าย` |
| `nn_load`           | `载入网`       | `網読込`        | `신경망불러오기`     | `โหลดโครงข่าย` |
| `bt_build`          | `建行为树`     | `行動木構築`     | `행동트리구성`      | `สร้างต้นไม้พฤติกรรม` |
| `bt_set`            | `设事实`       | `事実設定`      | `사실설정`        | `ตั้งข้อเท็จจริง` |
| `bt_tick`           | `行为树滴答`   | `行動木更新`     | `행동트리틱`       | `เดินต้นไม้พฤติกรรม` |
| `bt_status`         | `行为树状态`   | `行動木状態`     | `행동트리상태`      | `สถานะต้นไม้พฤติกรรม` |
| `dialog_new`        | `建对话模型`   | `対話モデル作成`   | `대화모델생성`      | `สร้างโมเดลสนทนา` |
| `dialog_learn`      | `对话学习`     | `対話学習`      | `대화학습`        | `เรียนรู้สนทนา` |
| `dialog_load`       | `对话载入`     | `対話読込`      | `대화불러오기`      | `โหลดชุดสนทนา` |
| `dialog_train`      | `对话训练`     | `対話訓練`      | `대화훈련`        | `ฝึกสนทนา` |
| `dialog_say`        | `对话生成`     | `対話生成`      | `대화생성`        | `พูดสนทนา` |
| `dialog_save`       | `对话存模`     | `対話モデル保存`   | `대화모델저장`      | `บันทึกโมเดลสนทนา` |
| `dialog_load_model` | `对话载模`     | `対話モデル読込`   | `대화모델불러오기`    | `โหลดโมเดลสนทนา` |

### Example, in Chinese

```ling
绑定 脑 = 建行为树("selector { sequence { ?敌人 > 0 !attack } !patrol }")
设事实(脑, "敌人", 1)
打印(行为树滴答(脑))      // -> "attack"
```

---

## Performance & design notes

- **No allocations on the hot path.** `nn_forward` allocates only its output
  vector; the inner products are tight `f32` loops the compiler autovectorizes at
  `opt-level = 3`.
- **Deterministic.** All randomness flows through a seeded SplitMix64 RNG, so
  training and sampling are reproducible given the same seed.
- **Self-contained.** Pure Rust, no BLAS/CUDA/ONNX dependency — it builds and runs
  anywhere Ling does, and every model is a few kilobytes.
- **Fail-soft.** Bad handles, wrong-width inputs, and malformed DSL/corpora return
  safe defaults instead of panicking the interpreter.

### Roadmap (2030+)

- GRU/attention layers for longer dialog context.
- Convolutional layers for vision-based agents.
- Utility-AI / GOAP planner alongside behavior trees.
- Quantized (int8) inference for thousands of concurrent agents.
