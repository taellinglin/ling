# Dialog

Ling has two dialog tools. **Dialog boxes** show what a character says: an
adventure-game text box that types out letter by letter, pages, and highlights
names, places and items in colour. **Dialog models** decide what a character
says: a tiny language model, trained in a second from that character's own
lines, that every NPC can carry. Its training data uses a plain text format,
the **Ling Dialog Standard** (`.lingdialog`).

Both work without a window except `dialog_draw`, so you can test a whole
conversation headless.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `dialog_show(text, cps)` | `对话显示` | `会話表示` | `대화표시` | `แสดงบทสนทนา` | Open a dialog box with marked-up text. |
| `dialog_step(dt)` | `对话步进` | `会話更新` | `대화스텝` | `ก้าวบทสนทนา` | Advance the typewriter and auto-advance timer. |
| `dialog_advance()` | `对话推进` | `会話送り` | `대화진행` | `เลื่อนบทสนทนา` | The player pressed confirm. |
| `dialog_active()` | `对话激活` | `会話中` | `대화중` | `บทสนทนาทำงาน` | Whether a box is open. |
| `dialog_typing()` | `对话打字` | `会話タイプ中` | `대화타이핑` | `กำลังพิมพ์บทสนทนา` | Whether the page is still typing. |
| `dialog_close()` | `对话关闭` | `会話閉じる` | `대화닫기` | `ปิดบทสนทนา` | Close the box at once. |
| `dialog_color(role, r, g, b)` | `对话颜色` | `会話色` | `대화색` | `สีบทสนทนา` | Colour of a text role. |
| `dialog_draw(x, y, w, h, font)` | `对话绘制` | `会話描画` | `대화그리기` | `วาดบทสนทนา` | Draw the box and its revealed text. |
| `dialog_new(ctx, embed, hidden, seed)` | `建对话模型` | `対話モデル作成` | `대화모델생성` | `สร้างโมเดลสนทนา` | Create an empty dialog model. |
| `dialog_learn(m, text)` | `对话学习` | `対話学習` | `대화학습` | `เรียนรู้สนทนา` | Add one line to the model's corpus. |
| `dialog_load(m, path)` | `对话载入` | `対話読込` | `대화불러오기` | `โหลดชุดสนทนา` | Add a `.lingdialog` file to the corpus. |
| `dialog_train(m, epochs, lr)` | `对话训练` | `対話訓練` | `대화훈련` | `ฝึกสนทนา` | Train on the corpus. Returns the loss. |
| `dialog_say(m, prompt, max, temp)` | `对话生成` | `対話生成` | `대화생성` | `พูดสนทนา` | Generate a reply. |
| `dialog_save(m, path)` | `对话存模` | `対話モデル保存` | `대화모델저장` | `บันทึกโมเดลสนทนา` | Save the trained model. |
| `dialog_load_model(path)` | `对话载模` | `対話モデル読込` | `대화모델불러오기` | `โหลดโมเดลสนทนา` | Load a saved model. |

## Dialog boxes

The box state lives in the runtime, separate from drawing: `dialog_show` opens
it, `dialog_step` moves time forward, `dialog_advance` reacts to the player, and
`dialog_draw` paints it. While `dialog_active()` is true, pause your game world.

### Markup

| Markup | Meaning |
|---|---|
| `{n}...{/}` | A character name (colour role 1). |
| `{p}...{/}` | A place (role 2). |
| `{i}...{/}` | An item (role 3). |
| a newline character | A new line in the box. |
| `\|\|` | A page break. |

A tag is exactly three characters, `{`, a letter and `}`, and `{/}` returns to
plain text (role 0). Anything else is shown as written.

A page that has finished typing advances by itself after 1.4 seconds, and the last
page then closes the box.

### `dialog_show(text, cps)`

Opens a box with `text`, typed at `cps` characters per second (default 32, at
least 1). It replaces any box already open.

### `dialog_step(dt)`

Advances the typewriter and the auto-advance timer by `dt` seconds (default
`0.016`). Call it every frame while the box is active.

### `dialog_advance()`

Call it when the player presses the confirm key. If the page is still typing, the
rest of the page appears at once; otherwise the box moves to the next page, or
closes after the last one. Drive it from a key press (`key_pressed`), not a held
key, so one press doesn't skip several pages.

### `dialog_active()`, `dialog_typing()` and `dialog_close()`

`dialog_active()` is `true` while a box is open. `dialog_typing()` is `true` while
the current page is still appearing. `dialog_close()` closes the box immediately.

```ling
bind start = do {
  dialog_show("Hello {n}Ling{/}.||Second page.", 100)
  print("active: " + str(dialog_active()))
  print("typing: " + str(dialog_typing()))
  dialog_step(0.5)
  print("typing after 0.5 s: " + str(dialog_typing()))
  dialog_advance()
  print("page 2 typing: " + str(dialog_typing()))
  dialog_advance()
  dialog_advance()
  print("active at the end: " + str(dialog_active()))
}
```

```output
active: true
typing: true
typing after 0.5 s: false
page 2 typing: true
active at the end: false
```

At 100 characters per second the first page is fully typed after 0.5 s. The first
`dialog_advance` turns to page 2, the second reveals all of page 2, and the third
closes the box.

### `dialog_color(role, r, g, b)`

Sets the colour of a role: `0` plain text, `1` name, `2` place, `3` item (values
above 3 count as 3). `r`, `g` and `b` are 0 to 255 (default 255).

### `dialog_draw(x, y, w, h, font)`

Draws the bevelled box with its top-left corner at `(x, y)` (default 40, 0), size
`w` by `h` (default 720 by 150), and the text revealed so far. `font` is a handle
from `font_load`; leave it out or pass `-1` to use the built-in text drawing.

```ling-gui
bind start = do {
  open_window(800, 600)
  bind ui = font_load("assets/fonts/Exo2.ttf", 600.0)
  bind line = "Hey {n}Link{/}! Take the {i}Master Sword{/} to {p}Hyrule Field{/}.||Beware the night."
  dialog_color(1, 255, 210, 80)
  while window_is_open() {
    if key_pressed("space") {
      if dialog_active() { dialog_advance() } else { dialog_show(line, 34.0) }
    }
    fill(10, 10, 20)
    if dialog_active() {
      dialog_step(0.016)
      dialog_draw(50.0, 420.0, 700.0, 150.0, ui)
    } else {
      // normal game update here
    }
    present()
  }
}
```

## Dialog models

A dialog model is a miniature next-word language model, a few kilobytes in size,
built into Ling (`ling_ai::dialog_lm`). You give it a character's lines, train it,
and ask it to continue a prompt. It learns cadence and vocabulary, not facts, so
it suits ambient chatter, barks and flavour text. Training a small corpus takes
well under a second. With temperature `0` the output is deterministic.

```ling
bind start = do {
  bind npc = dialog_new(3, 32, 64, 1)
  dialog_learn(npc, "welcome traveler")
  dialog_learn(npc, "keep your blade sharp")
  dialog_learn(npc, "welcome back traveler")
  bind loss = dialog_train(npc, 60, 0.1)
  print(loss < 2.0)
  print(dialog_say(npc, "welcome", 6, 0.0))
}
```

```output
true
back traveler
```

### `dialog_new(ctx, embed, hidden, seed)`

Creates an empty model and returns its handle. `ctx` (default 3) is how many
previous words it looks at, `embed` (default 32) and `hidden` (default 64) are the
network sizes, and `seed` (default 1) makes training repeatable.

### `dialog_learn(m, text)` and `dialog_load(m, path)`

`dialog_learn` adds one line to the corpus. `dialog_load` adds a whole
`.lingdialog` file and returns the number of training lines it added (one per
conversation), or `-1` if the file can't be read.

### `dialog_train(m, epochs, lr)`

Trains on everything in the corpus for `epochs` passes (default 20) at learning
rate `lr` (default 0.1). Returns the final loss: lower means the model fits its
lines better.

### `dialog_say(m, prompt, max, temp)`

Continues `prompt` for up to `max` words (default 24) and returns only the new
words. It stops early at the end of a turn. `temp` (default 0.8) is the
temperature: `0` always picks the likeliest word, higher values are more varied.

The tokenizer splits text at spaces and punctuation and lower-cases it. For
languages written without spaces, put a space between words in your corpus and
prompts; the reply comes back with spaces between words.

### `dialog_save(m, path)` and `dialog_load_model(path)`

`dialog_save` writes the trained model to a file (default `"model.llm"`) and
returns `true` on success. `dialog_load_model` reads one back and returns a new
handle, or `-1` on failure. Train once while building your game, then ship and
load the small model file.

## The Ling Dialog Standard

`.lingdialog` is a line-based, human-writable corpus format. Writers edit it by
hand or generate it from a spreadsheet; it diffs cleanly in reviews and can be
localized like any text file.

```text
# Ling Dialog Standard v1
@meta title: Tavern Keeper
@meta speaker: Bram

= welcome
bram: welcome traveler
hero: any news from the road
bram: bandits on the north pass
      nothing my stew cannot cure

= rumors
bram: keep your blade sharp
```

| Line | Meaning |
|---|---|
| `# ...` | Comment to the end of the line. |
| `@meta key: value` | Metadata about the dataset. Repeatable. |
| `= label` | Start a new conversation. The label is optional. |
| `speaker: text` | One turn. The speaker is the text before the **first** colon. |
| a line without `:` | Continues the previous turn (joined with a space). |
| a blank line | Ignored. |

Parsing never fails: a malformed line is skipped, so one stray character can't
break a training run. Turns before the first `=` form an implicit conversation.

Each conversation becomes one training line, with its turns joined by the
reserved `<eos>` token, so the model learns to continue **across** turns. Writing
`<eos>` yourself also marks a turn boundary. The reserved tokens are `<pad>`,
`<bos>`, `<eos>` and `<unk>`.

```ling
bind start = do {
  write_file("bram.lingdialog", "# Ling Dialog Standard v1\n@meta speaker: Bram\n\n= welcome\nbram: welcome traveler\nhero: any news from the road\nbram: bandits on the north pass\n\n= rumors\nbram: keep your blade sharp\n")
  bind npc = dialog_new(3, 32, 64, 1)
  print(dialog_load(npc, "bram.lingdialog"))
  dialog_train(npc, 80, 0.1)
  print(dialog_say(npc, "bandits on the", 8, 0.0))
  print(dialog_save(npc, "bram.llm"))
  bind again = dialog_load_model("bram.llm")
  print(dialog_say(again, "keep your", 8, 0.0))
}
```

```output
2
north pass
true
blade sharp
```

### Writing a good corpus

- **Consistency beats volume.** A small corpus in one clear voice gives a more
  recognisable character than a large generic one.
- **Repeat signature phrases.** Lines you want the character to favour should
  appear a few times across conversations.
- **One file per voice.** Keep each character in its own file, so each gets its
  own model and personality.
- **Write naturally.** Case and punctuation are handled by the tokenizer.

## Putting them together

The model writes the line; the box shows it:

```ling-gui
bind start = do {
  open_window(800, 600)
  bind bram = dialog_load_model("npc/bram.llm")
  while window_is_open() {
    if key_pressed("e") && !dialog_active() {
      dialog_show("{n}Bram{/}: " + dialog_say(bram, "welcome", 12, 0.7))
    }
    if key_pressed("space") && dialog_active() { dialog_advance() }
    fill(10, 10, 20)
    dialog_step(0.016)
    if dialog_active() { dialog_draw(50, 420, 700, 150) }
    present()
  }
}
```

## See also

- [Fonts and UI](../reference/fonts-ui.md): `font_load` for the box font.
- [Input and Microphone](../reference/input.md): `key_pressed` for advancing.
- [AI and Language Models](../reference/ai.md): behaviour trees, larger language models and speech.
