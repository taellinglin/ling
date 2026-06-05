# Ling UI Fonts

Per-language professional fonts downloaded from [Google Fonts](https://fonts.google.com),
all under the **SIL Open Font License 1.1** (OFL). Each is a variable TTF; the Ling
glyph rasterizer (fontdue) renders the default master.

| Language | Script        | File               | Family         |
|----------|---------------|--------------------|----------------|
| `en`     | Latin         | `Orbitron.ttf`     | Orbitron       |
| `zh`     | CJK (Simpl.)  | `NotoSansSC.ttf`   | Noto Sans SC   |
| `ja`     | Japanese      | `NotoSansJP.ttf`   | Noto Sans JP   |
| `ko`     | Korean        | `NotoSansKR.ttf`   | Noto Sans KR   |
| `th`     | Thai          | `NotoSansThai.ttf` | Noto Sans Thai |

Orbitron gives the holographic UI its geometric sci-fi headings; the Noto Sans
family provides complete coverage for every language the Ling lexicon ships.

## Loading from a `.ling` script

```ling
bind fnt = font_load("assets/fonts/Orbitron.ttf")   # → handle
font_text(fnt, 40.0, 22.0, 28.0, "CRYPTO DONUT")    # handle, x, y, px, string
bind w = font_width(fnt, 28.0, "CRYPTO DONUT")       # measure pixel width
```

The text is rasterized once per (glyph, size) and alpha-blended into the
framebuffer using the current `set_color` pen — so it glows under additive blend
just like the vector `ui_text`.
