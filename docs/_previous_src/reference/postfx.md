# Toon Post-FX & Volumetric Light

Ling's renderer finishes every frame with a **screen-space post chain** — the
"Wind Waker pipeline" — applied inside `present()` in this order:

```
1. SSAO        contact shadows from the z-buffer          set_ssao
2. Outlines    depth-discontinuity ink lines              set_outline
3. Tone ramp   luminance → brightness curve (cel bands)   tone_* family
4. Bloom       soft glow bleeding from bright pixels      set_bloom
5. FXAA        edge anti-aliasing over the final image    set_fxaa
```

Everything is CPU-rasterised and parallelised; with all passes on, the whole
chain costs well under a millisecond at 960×540 (bench: +0.1 ms for
tone + SSAO + bloom + FXAA together) — the two costs that scale with what you
draw are `depth_blur` (~+0.4 ms) and the volumetric light volumes (pure
fill-rate, proportional to their on-screen size).

Vector **line and text ink is exempt** from tone mapping: those pixels carry an
internal *unlit* tag, so UI text and wireframes never cel-band-snap or flicker
against the lit triangles around them.

---

## The tone ramp

All lighting response passes through one **tone ramp**: a gradient of
`(luminance in → brightness out)` stops. Hard-snapping between stops gives
crisp cel bands; smoothing gives painterly gradients.

```ling
tone_stop(t, value)        # add a stop: input luminance t → brightness value
tone_smooth(on)            # 0 = hard cel snap (default) · 1 = smooth lerp
tone_bezier(y1, y2)        # Bézier remap of input luminance (S-curve contrast)
tone_bezier_off()          # remove the remap
tone_ramp_reset()          # restore the default 3-band cel ramp
tone_ramp_clear()          # remove all stops, then build your own
tone_soft(soft, sheen)     # band-edge softness + highlight sheen (see below)
```

The default ramp is the classic 3-band cel: shadow `0.08`, mid `0.50`,
lit `1.00`.

### `tone_soft(soft, sheen)` — soft bands and clean highlights

Two fixes for the classic cel-shading artefacts, both on by default:

- **`soft`** `[0..1]` — each band boundary gets a smoothstep transition zone
  covering this fraction of the band gap. `0` = razor-crisp cel edges;
  `0.32` (default) = Wind Waker-style soft shadow terminators. Band interiors
  stay flat, so the look still reads as toon.
- **`sheen`** `[0..1]` — pixels brighter than ~0.72 luminance keep their
  smooth gradient instead of being quantised. Specular and fresnel-rim
  highlights render as a clean **sheen** rather than scratchy posterised
  patches. Default `0.65`.

Dark pixels are also protected: near-black noise is no longer amplified into
bright speckles inside shadows.

### Recommended looks

```ling
# crisp anime cel (band edges softened, highlights smooth)
tone_smooth(0);  tone_soft(0.32, 0.65)

# Wind Waker / painterly: smooth S-curve tone, banding comes from the
# per-vertex cel lighting instead of the pixel ramp — best with additive
# gradients (light pools, beams) which stay perfectly smooth
tone_smooth(1);  tone_bezier(0.12, 0.88);  tone_soft(0.32, 0.65)
```

---

## Ambient occlusion — `set_ssao(strength, radius_px, zrange)`

Soft contact shading in corners, creases and under objects, computed from the
z-buffer on a half-resolution grid (8 taps), box-smoothed, and multiplied into
the frame with bilinear upsampling — wide and smooth, never grainy.

```ling
set_depth_test(1)          # SSAO reads the z-buffer
set_ssao(0.4, 7, 14)       # strength [0..1] · tap radius px · depth window
set_ssao(0)                # off (default)
```

`zrange` is in camera-space depth units: a neighbour counts as an occluder
when it is closer to the camera by more than a small bias and less than
`zrange`.

## Bloom — `set_bloom(strength, threshold)`

Bright pixels (rim sheen, emissive cores, additive FX) bleed a soft
quarter-resolution glow over the frame — the "HDR material" feel for toon and
vector art.

```ling
set_bloom(0.5, 0.74)       # glow strength · luminance threshold [0..1]
set_bloom(0)               # off (default)
```

## Anti-aliasing — `set_fxaa(on)`

FXAA-lite: pixels on a high-contrast luma edge blend toward their neighbour
average — polygon stair-steps and ink-line jaggies soften, flat fills are
untouched. Runs last so it also smooths outlines and bloom edges.

```ling
set_fxaa(1)
```

> `set_antialias(on)` is a different, older toggle: it smooths **wireframe
> strokes** (Xiaolin-Wu coverage) at draw time. `set_fxaa` smooths the final
> composited image.

## Tilt-shift depth blur — `depth_blur(focus, range, radius, oil)`

Depth-of-field over the framebuffer using the z-buffer: sharp at camera-space
depth `focus`, blurring up to `radius` px as depth departs by `range`.
Background (no geometry) blurs fully. The blur is computed at half resolution
(visually identical, ~4× cheaper).

`oil` `[0..1]` gives the blurred zone an **oil-slick treatment**: the red and
blue blur planes are sampled with opposite horizontal offsets (iridescent
chroma fringe) plus a gentle hue swirl — water, heat haze, dreamy far fields.

```ling
set_depth_test(1)
# ... draw the world ...
flush_3d()
depth_blur(30, 150, 3, 0.4)   # focus · range · radius px · oil
# ... draw the HUD ...
present()
```

Call it **after** the world is flushed (z-buffer populated) and **before**
drawing UI.

---

## Volumetric lights — `light_pool` and `light_beam`

Underwater-style volumetric lighting as pure vector gradients: attach them to
your point/spot lights and the colour visibly spreads through space and
splashes on the floor. Both are additive Gouraud fans whose outer vertices are
black — additive black adds nothing, so the edges are perfectly smooth with no
polygon rim, no texture.

```ling
light_pool(x, y, z, radius, r, g, b, intensity)
#   the coloured splash a light throws on the floor at height y
#   centre = colour × intensity → mid ring → transparent edge

light_beam(x, y, z, floor_y, radius, r, g, b, intensity)
#   god-ray double cone from the light position down to floor_y,
#   spreading to radius — outer shell fades to transparent, a
#   half-radius core keeps the shaft body lit from every angle
```

Both are distance-fog aware (`set_fog`): far volumes fade out instead of
adding fog-coloured light. Cost is fill-rate — a screen-tall beam is a big
additive triangle fan, so size and count are your budget dials.

```ling
# an underwater lamp: cyan shaft + pool + real light, under a soft blue fog
add_light(-16, -20, 26,  0.25, 0.9, 1.0,  1.6, 90)
light_beam(-16, -20, 26,  3.0, 13,  64, 230, 255,  0.55)
light_pool(-16,  3.0, 26,  19,      64, 230, 255,  0.7)
```

Pair with `tone_smooth(1)` (hard band snapping turns smooth additive
gradients into flat bands) and a touch of `set_bloom` for the glow.

---

## Quick start — the full Wind Waker chain

```ling
bind start = do {
    open_fullscreen("windwaker")
    set_depth_test(1)
    tone_smooth(1);  tone_bezier(0.12, 0.88);  tone_soft(0.32, 0.65)
    set_ssao(0.4, 7, 14)
    set_bloom(0.5, 0.74)
    set_fxaa(1)

    while window_is_open() {
        fill(5, 8, 20)
        clear_lights()
        add_light(0, -30, 14,  1.0, 0.8, 0.35,  1.2, 80)

        # ... world ...

        light_beam(0, -30, 14,  3.0, 11,  255, 204, 90, 0.45)
        light_pool(0,  3.0, 14,  16,      255, 204, 90, 0.6)

        flush_3d()
        depth_blur(30, 150, 3, 0.4)
        present()
    }
}
```

Multilingual aliases for every function on this page are listed in
[Builtin Aliases by Language](../multilingual/builtins.md) and the
[ling-graphics glossary](../glossary/crate-graphics.md).
