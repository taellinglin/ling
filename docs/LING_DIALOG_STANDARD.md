# The Ling Dialog Standard (`.lingdialog`)

A tiny, human-writable corpus format for training the in-engine dialog model
([`ling_ai::dialog_lm`]). It is line-oriented and diff-friendly: writers author
it by hand, or generate it from a spreadsheet, then `ling` loads it directly with
`dialog_load`.

> **Why a standard?** Every NPC can ship its own kilobyte-scale personality
> model, trained from a corpus of *its own lines*. A shared text format means
> dialog data is reviewable in PRs, localizable, and tool-generatable — no opaque
> binary blobs in the asset pipeline.

## File shape

```lingdialog
# Ling Dialog Standard v1            <- '#' starts a comment, to end of line
@meta title: Tavern Keeper          <- dataset metadata (free-form key: value)
@meta speaker: Bram
@meta lang: en

= welcome                           <- '=' opens a new conversation ('welcome' label is optional)
bram: welcome traveler              <- "speaker: text" is one turn
hero: any news from the road
bram: bandits on the north pass
      nothing my stew cannot cure   <- an indented line with no ':' continues the turn

= rumors                            <- next conversation
bram: keep your blade sharp
```

## Grammar

| Construct | Meaning |
|-----------|---------|
| `# …`               | Comment to end of line. |
| `@meta key: value`  | Dataset metadata. Repeatable. Read with `Dataset::meta`. |
| `= [label]`         | Start a new conversation. The label is optional. |
| `speaker: text`     | One turn. Speaker is the text before the **first** colon. |
| *line without `:`*  | Continuation — appended (space-joined) to the previous turn. |
| *blank line*        | Ignored. |

Parsing **never fails** — malformed lines are skipped so a stray character can't
break a training run. Turns appearing before any `=` open an implicit
conversation.

## How it becomes training data

`dialog_load` calls [`Dataset::training_lines`]: each conversation becomes one
training string with its turns joined by the reserved `<eos>` token, so the model
learns to continue dialog **across** turns, not just within a single line:

```
welcome traveler <eos> any news from the road <eos> bandits on the north pass nothing my stew cannot cure
```

The four reserved tokens are `<pad>`, `<bos>`, `<eos>`, `<unk>`. Writing `<eos>`
literally in a corpus is honored as a turn boundary.

## Using it from Ling

```ling
bind npc   = dialog_new(3, 32, 64, 1)                 // ctx, embed, hidden, seed
bind lines = dialog_load(npc, "npc/bram.lingdialog")  // returns # lines loaded
bind loss  = dialog_train(npc, 80, 0.1)               // epochs, learning rate
print(dialog_say(npc, "welcome traveler", 12, 0.7))   // prompt, max tokens, temperature
dialog_save(npc, "npc/bram.llm")                      // persist the trained model
```

The same calls are available in all five Ling languages — see
[GAME_AI_2030.md](GAME_AI_2030.md) for the full builtin table.

## Authoring tips

- **Consistency beats volume.** A small, on-voice corpus produces a more
  recognisable character than a large, generic one — the model is tiny and learns
  *cadence and vocabulary*, not facts.
- **Repeat signature phrases.** Lines you want the NPC to favour ("keep your
  blade sharp") should appear a few times across conversations.
- **One file per voice.** Keep each character's corpus separate so each gets its
  own model and personality.
- **Lowercasing & punctuation** are handled by the tokenizer; write naturally.

## Round-tripping

`Dataset::to_lingdialog` re-emits the canonical form, so tools can read, transform,
and rewrite corpora losslessly (verified by the crate's round-trip test).
