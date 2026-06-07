# Asset Conversion — `ling convert`

Transcode an external asset into a **self-contained, importable `.ling` file** that
reconstructs it with the engine's own builtins — preserving geometry, names, and
structure. Bulk numeric data is embedded **losslessly**: deflate-compressed + base64
behind the `blob_f32` / `blob_i32` builtins by default, or plain `.ling` arrays with
`--no-compression`.

```sh
ling convert model.glb                 # → model.ling   (compressed, lossless)
ling convert sfx.wav -o sfx.ling       # explicit output
ling convert logo.svg --no-compression # plain arrays (human-readable / diff-able)
lingfu convert song.mid                # lingfu forwards to ling
```

| Extension | Becomes | Preserves |
|---|---|---|
| `.gltf` / `.glb` | per-mesh `pos`/`nrm`/`uv` + indices, node names/transforms, `draw_<name>()` | geometry, mesh & node names, hierarchy, materials index |
| `.wav` `.ogg` `.flac` `.mp3` | `<name>_pcm` (mono f32) + `_rate` / `_dur` | full-precision PCM at source rate |
| `.mid` / `.midi` | flat `[time,dur,midi,vel,channel]` note list | every note event + timing |
| `.svg` | flattened polylines + `draw_<name>(ox,oy,scale)` | paths (M/L/H/V/C/Q/Z), `line`/`rect`/`polyline`/`polygon` |
| `.blend` | routed through Blender's glTF exporter (needs Blender on `PATH` or `$BLENDER`) | whatever Blender's glTF export carries |

**Flags:** `-o <out.ling>` (default `<asset>.ling`) · `--no-compression`.

## Using a converted file

```ling
ใช้ "model.ling"            # use / import the generated module
bind start = do {
    open_window(1280, 720)
    while window_is_open() {
        set_color(180, 220, 255)
        draw_<name>(0.0, 0.0, 0.0, 1.0)   # the generated draw fn
        flush_3d()
        present()
    }
}
```

The blob builtins decode the embedded data on import:

- `blob_f32("…")` → list of numbers (little-endian f32, deflate+base64).
- `blob_i32("…")` → list of numbers (little-endian i32) — mesh/polyline indices.

Both are lossless; `--no-compression` simply emits the same numbers as a literal
`[ … ]` list instead of a blob, which is larger but human-readable.
