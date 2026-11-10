# Asset Conversion

`ling convert` turns an external asset (a 3D model, a sound, a MIDI song or an SVG
drawing) into a self-contained `.ling` file. You `use` that file in your program,
and it rebuilds the asset with Ling's own builtins. No loader or file at run time
is needed.

```sh
ling convert model.glb                  # writes model.ling (compressed, lossless)
ling convert sfx.wav -o sfx.ling        # choose the output path
ling convert logo.svg --no-compression  # plain arrays you can read and diff
lingfu convert song.mid                 # lingfu forwards to ling convert
```

| Option | Meaning |
|---|---|
| `-o <out.ling>` (or `--out`) | Output path. Default: the input name with `.ling`. |
| `--no-compression` | Write numbers as plain lists instead of compressed blobs. |

## What each format becomes

| Input | Generated bindings | Profile |
|---|---|---|
| `.gltf`, `.glb` | Per mesh: `<name>_<mesh>_pos`, `_nrm`, `_uv`, `_idx`, and a wireframe `draw_<name>_<mesh>(ox, oy, oz, scale)`. `draw_<name>(ox, oy, oz, scale)` draws the whole model. Node names and transforms are kept as comments. | `game` |
| `.wav`, `.ogg`, `.flac`, `.mp3` | `<name>_pcm` (mono samples), `<name>_rate`, `<name>_dur` | `audio` |
| `.mid`, `.midi` | `<name>_notes`, a flat list of `[time, dur, midi, vel, channel]` per note, and `<name>_dur` | `audio` |
| `.svg` | `<name>_xy` (points), `<name>_lens` (points per polyline), and `draw_<name>(ox, oy, scale)` | any |
| `.blend` | Exported to glTF by Blender, then converted like a `.glb`. Needs Blender on your `PATH` or in `$BLENDER`. | `game` |

`<name>` is the input file name, cleaned up to be a valid identifier. For SVG,
`<path>` data (`M`, `L`, `H`, `V`, `C`, `Q`, `Z`) and `<line>`, `<rect>`,
`<polyline>` and `<polygon>` elements are turned into polylines. Any other extension
fails with `unsupported extension`.

## Compressed or plain

By default, long lists of numbers are stored losslessly as zlib-compressed, base64
text behind two builtins:

- `blob_f32("...")` decodes to a list of numbers (little-endian 32-bit floats).
- `blob_i32("...")` decodes to a list of whole numbers (32-bit integers), used for
  indices and lengths.

`--no-compression` writes the same numbers as a literal `[ ... ]` list. The file is
larger but easy to read. Lists of eight numbers or fewer are always written plainly.

## Example: an SVG

Given a small `logo.svg` with one `<rect>` and one `<path>`:

```text
$ ling convert logo.svg
[convert] logo.svg → logo.ling  (1 KB, deflate+base64 lossless)
```

The generated `logo.ling` starts like this:

```ling-module
# 2 polylines, 7 points
bind logo_xy = blob_f32("eNpdibENAAAIgzjFV/RmDzTRwaYDAwCRLF2Huvf/A/L0CM0=")
bind logo_lens = [2, 5]
```

followed by a `draw_logo(ox, oy, scale)` function. Use it from your program:

```ling-gui
use "logo.ling"

bind start = do {
    open_window(400, 400, "Logo")
    while window_is_open() {
        fill(0, 0, 0)
        set_color(255, 255, 255)
        draw_logo(100, 100, 2.0)
        present()
    }
}
```

## Notes

- Audio is stored as a mono downmix at the source sample rate. The generated file
  holds the samples; playing or analysing them is up to your program.
- The generated code uses Thai keywords for its functions (`ฟังก์ชัน`). That is
  normal: every language works in every file.
- A model's draw function draws a wireframe with `draw_line_3d`. For solid, textured
  meshes, see [3D Shapes and Meshes](../reference/shapes.md).
