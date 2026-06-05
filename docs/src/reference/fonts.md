# Builtins — Vector Fonts

Ling renders real TrueType/OpenType fonts as **resolution-independent vectors**. The first time
a glyph is used its outline is extracted and cached as `cache/fonts/<font>/<codepoint>.ling`
(curves preserved), then rendered crisp — stroked or filled, with anti-aliasing — at any size
and in 2D or 3D/4D.

| Builtin | Meaning |
|---|---|
| `font_load(path [, weight])` → handle | Load a TTF/OTF; optional variable-font `weight` (e.g. `600` for bold). Returns `-1` on failure. |
| `font_text(handle, x, y, px, "text")` | Anti-aliased **stroked** outline in the current `set_color`/`set_blend`. |
| `font_text_fill(handle, x, y, px, "text")` | Anti-aliased **filled** glyphs (solid letterforms). |
| `font_text_3d(handle, cx,cy,cz, ux,uy,uz, vx,vy,vz, size, "text")` | Stroked text on a 3-D plane (basis `u`,`v`), depth-sorted — rotates with the camera and projects through the 4-D pipeline. |
| `font_width(handle, px, "text")` | Pixel width of a string. |

Any script is covered: Latin, 中文, 日本語, 한국어, ไทย all render from the right font
(e.g. Noto Sans SC covers Latin + CJK + Thai).

```ling
bind f = font_load("assets/fonts/Exo2.ttf", 600.0)
set_color(0, 230, 255)
font_text_fill(f, 40.0, 30.0, 36.0, "LING 灵")
font_text_3d(f, 0.0,0.0,0.0,  1.0,0.0,0.0,  0.0,-1.0,0.0, 0.6, "3D TEXT")
```

Related: `project_3d(x,y,z)` → `[sx,sy,depth]` and `draw_poly([x0,y0,…])` (filled 2-D polygon,
blend-aware) let you place filled vector shapes onto projected 3-D points.
