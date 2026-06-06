# Builtins — Dialog

Cinematic, Ocarina/Majora-style dialog boxes (from `ling-game`): text typed out
letter-by-letter, paged, auto-advancing, with **colour-coded highlighting** and a
font of your choice. Pure-logic in `ling-game`; the runtime renders it.

**Markup:** `{n}…{/}` name/character · `{p}…{/}` place · `{i}…{/}` item · `\n` newline ·
`||` page break.

| Builtin | Meaning |
|---|---|
| `dialog_show("text" [, cps])` | Start a dialog (chars-per-second typing speed, default 32). |
| `dialog_step(dt)` | Advance the typewriter + auto-advance timers (call each frame). |
| `dialog_advance()` | On a key press: reveal the rest of the page, else go to the next page / close (drive it from SPACE *release*). |
| `dialog_active()` → bool | A box is open — gate your world update on this to "suspend" physics/input. |
| `dialog_typing()` → bool | Still revealing characters. |
| `dialog_close()` | Force-close the box. |
| `dialog_color(role, r,g,b)` | Recolour a role — `0` text · `1` name · `2` place · `3` item. |
| `dialog_draw(x, y, w, h [, font])` | Draw the beveled box + revealed text (pass a `font_load` handle, or `-1` for the stroke font). |

```ling
mic_open()
bind ui = font_load("assets/fonts/Exo2.ttf", 600.0)
bind line = "Hey {n}Link{/}! Take the {i}Master Sword{/} to {p}Hyrule{/} field.||Beware the night."
while window_is_open() {
    if key_pressed("space") { if dialog_active() { dialog_advance() } else { dialog_show(line, 34.0) } }
    if dialog_active() {
        dialog_step(0.016)               # world suspended while talking
        dialog_draw(50.0, 420.0, 800.0, 150.0, ui)
    } else {
        # … normal game update …
    }
    present()
}
```
See `examples/ui/dialog_demo.ling`.
