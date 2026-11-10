# Post-FX and Lighting

This page covers how Ling lights a 3D scene and how it finishes each frame. Lights and shading decide the colour of every triangle as it is drawn. Then a screen-space **post chain** runs over the finished image: ambient occlusion, ink outlines, a tone ramp, bloom and anti-aliasing. On top of that you can add depth of field, a screen warp and soft volumetric light shafts.

Everything here draws into the open window, so the examples are `ling-gui` programs.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `set_ambient(level)` | `环境光` | `環境光設定` | `환경광설정` | `ตั้งแสงรอบข้าง` | Ambient light level |
| `add_light(x, y, z, r, g, b, intensity, radius)` | `加灯` | `ライト追加` | `조명추가` | `เพิ่มแสง` | Add a point light |
| `clear_lights()` | `清灯` | `ライト消去` | `조명초기화` | `ล้างแสง` | Remove all lights |
| `set_fog(r, g, b, start, end)` | `雾` | `霧設定` | `안개설정` | `ตั้งหมอก` | Distance fog |
| `set_shade_mode(m)` | `设置着色` | `シェード設定` | `셰이드모드` | `ตั้งการแรเงา` | Flat, cel or holographic shading |
| `set_cel_bands(n)` | `设置色阶` | `セル段数` | `셀밴드` | `ตั้งระดับสี` | Number of brightness bands |
| `set_shadow_color(r, g, b)` | `设置阴影色` | `影の色` | `그림자색` | `ตั้งสีเงา` | Tint of unlit areas |
| `set_rim(strength, r, g, b)` | `设置边缘光` | `リム設定` | `림라이트` | `ตั้งขอบเรือง` | Rim glow on silhouettes |
| `set_flat_shade(on)` | `平面着色` | `フラット着色` | `평면음영` | `ตั้งแฟลตเชด` | Skip lighting entirely |
| `set_material(key, value)` | `设置材质` | `マテリアル設定` | `재질설정` | `ตั้งวัสดุ` | Set a material property |
| `reset_material()` | `重置材质` | `マテリアルリセット` | `재질초기화` | `รีเซ็ตวัสดุ` | Back to ordinary lighting |
| `set_ssao(strength, radius_px, zrange)` | `环境光遮蔽` | `アンビエントオクルージョン` | `앰비언트오클루전` | `ตั้งเงาสัมผัส` | Ambient occlusion |
| `toon_outlines(thickness, color, threshold)` | `卡通轮廓` | `トゥーンアウトライン` | `툰아웃라인` | `ตั้งเส้นขอบการ์ตูน` | Ink outlines |
| `tone_stop(t, value)` | `色调停止` | `トーンストップ` | `톤스톱` | `ตั้งจุดโทน` | Add a tone-ramp stop |
| `tone_smooth(on)` | `色调平滑` | `トーンスムーズ` | `톤스무스` | `ตั้งโทนนุ่ม` | Hard bands or smooth ramp |
| `tone_bezier(y1, y2)` | `色调贝塞尔` | `トーンベジェ` | `톤베지어` | `ตั้งโทนเบซิเยร์` | Contrast curve |
| `tone_bezier_off()` | `关闭色调贝塞尔` | `トーンベジェオフ` | `톤베지어끄기` | `ปิดโทนเบซิเยร์` | Remove the contrast curve |
| `tone_ramp_reset()` | `重置色调渐变` | `トーンランプリセット` | `톤램프리셋` | `รีเซ็ตการไล่โทน` | Default three-band ramp |
| `tone_ramp_clear()` | `清除色调渐变` | `トーンランプクリア` | `톤램프클리어` | `ล้างการไล่โทน` | Remove all stops |
| `tone_soft(soft, sheen)` | `色调柔边` | `トーンソフト` | `톤소프트` | `โทนขอบนุ่ม` | Soft band edges, smooth highlights |
| `set_bloom(strength, threshold)` | `泛光` | `ブルーム` | `블룸` | `ตั้งบลูม` | Glow around bright pixels |
| `set_fxaa(on)` | `屏幕抗锯齿` | `画面アンチエイリアス` | `화면안티앨리어싱` | `ลบรอยหยัก` | Screen anti-aliasing |
| `flush_post()` | `后期冲刷` | — | `포스트플러시` | — | Run the post chain now, before the HUD |
| `depth_blur(focus, range, radius, oil)` | `景深` | `景深` | — | `เบลอความลึก` | Depth of field |
| `screen_distort(amount, t, step)` | `屏幕扭曲` | `画面歪み` | `화면왜곡` | `บิดจอ` | Viscous screen warp |
| `light_pool(x, y, z, radius, r, g, b, intensity)` | `光池` | `ライトプール` | `빛웅덩이` | `แอ่งแสง` | Pool of light on the floor |
| `light_beam(x, y, z, floor_y, radius, r, g, b, intensity)` | `光柱` | `ライトビーム` | `빛기둥` | `ลำแสงไฟ` | Volumetric light shaft |

The interpreter accepts every name in the table. `lingfu normalize` translates only some of them (the lights and `set_ambient`), so the localized examples on this site keep the English names for the rest.

## Lights

Lights follow the same per-frame routine as drawing: clear them, add them, draw.

### `set_ambient(level)`

The light that reaches every surface regardless of lights, from 0 to 1 (default 0.15).

### `add_light(x, y, z, r, g, b, intensity, radius)`

Adds a coloured point light at a world position. The colour can be 0 to 1 or 0 to 255: if any channel is above 1.5, all three are read as 0 to 255. `intensity` is a brightness multiplier (default 1). `radius` is the distance at which the light fades to nothing; 0 means no falloff (the default).

The order is position, **colour**, intensity, radius. Lights add up until you call `clear_lights()`, so clear and re-add them every frame.

### `set_fog(r, g, b, start, end)`

Distance fog: geometry fades toward the colour (0 to 255) between camera depths `start` and `end`. `end` of 0 or less turns fog off. The volumetric lights below fade with the fog too.

```ling-gui
bind start = do {
    open_window(720, 420, "Lights")
    set_depth_test(1)
    set_ambient(0.2)
    set_fog(8, 10, 24, 10, 40)
    while window_is_open() {
        fill(8, 10, 24)
        clear_lights()
        add_light(0, -5, 4, 1.0, 0.85, 0.6, 1.2, 20)
        set_color(240, 150, 110)
        cube(0, 0, 8, 1, 1, 1, 0.3, time_now(), 0, 0)
        present()
    }
}
```

## Shading

For each vertex the shader mixes your pen colour with the ambient term (tinted by the shadow colour where a surface faces away from every light), each light's contribution, a rim glow toward the silhouette, and in the default mode a faint iridescent sheen. The result is quantised into a few brightness bands.

The defaults are deliberately stylised: a cool indigo shadow tint, a cyan rim and the holographic sheen. For colours that match your pen, use `set_shade_mode(1)`, `set_rim(0, 0, 0, 0)` and a dark neutral `set_shadow_color`.

| Builtin | Parameters |
|---|---|
| `set_shade_mode(m)` | `0` flat, one band per triangle; `1` smooth cel, no sheen, rim reduced; `2` holographic (default). |
| `set_cel_bands(n)` | Number of bands, at least 2 (default 4). |
| `set_shadow_color(r, g, b)` | Tint for unlit areas, 0 to 255 (default 26, 33, 77). |
| `set_rim(strength, r, g, b)` | Rim strength (default 0.6; 0 turns it off) and colour 0 to 255 (default 115, 217, 255). |
| `set_flat_shade(on)` | Non-zero skips all lighting and draws the raw pen colour. Meant for performance tests. |

### `set_material(key, value)` and `reset_material()`

`set_material` turns on a physically based toon material for later `draw_triangle_3d` and polygon draws. `key` is a string and `value` a number; colour keys take a packed `0xRRGGBB` number. Keys: `albedo`, `roughness`, `metallic`, `emission`, `emission_strength`, `specular`, `specular_tint`, `subsurface`, `subsurface_color`, `clearcoat`, `clearcoat_roughness`, `transmission`, `ior`, `iridescence`, `sheen`, `anisotropy`, `anisotropy_angle`, `toon_bands`, `shadow_softness`, `outline_px`, `outline_color`, `highlight_color`. Unknown keys are ignored. `reset_material()` switches back to ordinary lighting. The primitive solids (`cube`, `sphere`, ...) do not read the material.

## The post chain

The post chain runs inside `present()` in this order:

1. **Ambient occlusion** (`set_ssao`): soft contact shadows from the depth buffer.
2. **Outlines** (`toon_outlines`): ink lines where depth jumps.
3. **Tone ramp** (`tone_*`): maps brightness to cel bands.
4. **Bloom** (`set_bloom`): glow bleeding from bright pixels.
5. **FXAA** (`set_fxaa`): edge anti-aliasing over the final image.

Ambient occlusion, outlines, bloom and FXAA are off until you turn them on. The tone ramp always runs, starting from the default three-band ramp. Occlusion and outlines read the depth buffer, so call `set_depth_test(1)` first.

Vector lines and text are exempt from the tone ramp, so interface text and wireframes never snap into bands.

### `set_ssao(strength, radius_px, zrange)`

Soft darkening in corners, creases and under objects, computed at half resolution and smoothed. `strength` 0 to 1 (0 is off), `radius_px` the sampling radius in pixels (default 6), `zrange` the depth window in which a neighbour counts as an occluder (default 12).

### `toon_outlines(thickness, color, threshold)`

Ink lines on depth discontinuities. `thickness` is the half-width in pixels (0 is off, 1.5 is a good anime default), `color` a packed `0xRRGGBB` number (default 0, black), `threshold` the depth jump that counts as an edge (default 0.05).

### The tone ramp

The tone ramp is a list of stops, each mapping an input brightness `t` to an output brightness `value` (both 0 to 1). The default ramp has three bands: 0.08 below 0.25, 0.50 below 0.60, and 1.00 above.

- `tone_stop(t, value)` adds a stop. Stops are kept sorted by `t`.
- `tone_ramp_clear()` removes all stops so you can build your own. `tone_ramp_reset()` restores the default.
- `tone_smooth(on)`: `0` snaps hard between stops (default), `1` blends smoothly.
- `tone_bezier(y1, y2)` remaps the input brightness through a cubic Bézier before the lookup. About `0.333, 0.667` is neutral (the defaults), `0, 0` is shadow-heavy, `1, 1` highlight-heavy and `0.1, 0.9` an S-curve. `tone_bezier_off()` removes it.
- `tone_soft(soft, sheen)`: `soft` (0 to 1, default 0.32) is the fraction of each band gap that blends across the boundary, from razor-sharp at 0 to soft shadow edges around 0.3. `sheen` (0 to 1, default 0.65) lets bright pixels keep their gradient, so highlights look like a clean sheen instead of scratchy bands.

Two common looks:

| Look | Calls |
|---|---|
| Crisp anime cel | `tone_smooth(0)`, `tone_soft(0.32, 0.65)` |
| Painterly, smooth | `tone_smooth(1)`, `tone_bezier(0.12, 0.88)`, `tone_soft(0.32, 0.65)` |

### `set_bloom(strength, threshold)`

Bright pixels bleed a soft glow, computed at quarter resolution. `strength` defaults to 0.45 (0 is off); `threshold` is the luminance cutoff from 0 to 0.99 (default 0.74).

### `set_fxaa(on)`

A light anti-aliasing pass over the whole image (default argument 1). It softens polygon stair-steps and outline jaggies and leaves flat fills alone. It is different from `set_antialias`, which smooths wireframe strokes as they are drawn.

### `flush_post()`

Flushes the 3D queue like `flush_3d()`, then runs the post chain immediately. `present()` skips the chain for that frame, so a HUD drawn after `flush_post()` stays crisp, with no bloom or blur. `post_now()` is another English name for it.

```ling-gui
bind start = do {
    open_window(960, 540, "Toon")
    set_depth_test(1)
    set_shade_mode(1)
    set_rim(0, 0, 0, 0)
    set_shadow_color(30, 30, 50)
    toon_outlines(1.5, 0, 0.05)
    set_ssao(0.4, 7, 14)
    tone_smooth(1)
    tone_bezier(0.12, 0.88)
    tone_soft(0.32, 0.65)
    set_bloom(0.5, 0.74)
    set_fxaa(1)
    while window_is_open() {
        fill(20, 24, 40)
        clear_lights()
        add_light(-2, -6, -2, 1.0, 0.95, 0.85, 0.8, 0)
        set_color(220, 180, 120)
        sphere(0, 0, 8, 1.5, 1.5, 1.5, 0, time_now(), 0, 0)
        flush_post()
        set_color(255, 255, 255)
        ui_text(20, 20, 16, "HUD")
        present()
    }
}
```

## Screen effects

### `depth_blur(focus, range, radius, oil)`

Depth of field over the frame, using the depth buffer. The image is sharp at camera depth `focus` (default 30) and reaches full blur `radius` pixels (default 3) as depth moves `range` away (default 60). Background with no geometry blurs fully. `oil` (0 to 1, default 0) adds an iridescent oil-slick fringe and hue swirl to the blurred zone, for water or heat haze. `dof` and `depth_of_field` are other English names.

Call it **after** `flush_3d()` (so the depth buffer is filled) and **before** you draw the interface. It needs `set_depth_test(1)`.

### `screen_distort(amount, t, step)`

A viscous full-screen warp that wraps at the edges, for goo, heat haze and dream effects. `amount` is the largest displacement in pixels (default 8), `t` a time value so the warp moves (default 0), and `step` 1 for full resolution or 2 and up for a cheaper block warp (default 1). Like `depth_blur`, call it after the 3D flush and before the interface. It is not available in the browser build.

```ling-gui
bind start = do {
    open_window(960, 540, "Haze")
    set_depth_test(1)
    while window_is_open() {
        fill(10, 20, 40)
        set_color(120, 200, 255)
        cube(0, 0, 8, 1, 1, 1, 0, time_now(), 0, 0)
        cube(3, 0, 20, 1, 1, 1, 0, time_now(), 0, 0)
        flush_3d()
        depth_blur(8, 6, 4, 0.3)
        screen_distort(4, time_now(), 2)
        present()
    }
}
```

## Volumetric light

`light_pool` and `light_beam` draw soft, additive gradients that look like light spreading through water or dusty air. Their outer edges fade to black, so with additive blending they have no visible rim. Both fade with distance fog, and their cost is fill rate: the bigger on screen, the more expensive.

### `light_pool(x, y, z, radius, r, g, b, intensity)`

The coloured splash a lamp throws on the floor at height `y`. Defaults: position 0, `radius` 20, colour 255 (0 to 255 per channel), `intensity` 1 (about 0.2 to 1.5 looks good).

### `light_beam(x, y, z, floor_y, radius, r, g, b, intensity)`

A god-ray: a double cone from the light position down to the floor plane at `floor_y`, widening to `radius` (default 14). Pair it with a `light_pool` at its base.

Draw both with `set_blend(1)` (additive) and return to `set_blend(0)` afterwards. They look best with `tone_smooth(1)`, because hard bands turn smooth gradients into flat steps.

```ling-gui
bind start = do {
    open_window(720, 420, "Volumetric")
    set_depth_test(1)
    set_ambient(0.2)
    set_shade_mode(1)
    tone_smooth(1)
    set_bloom(0.5, 0.74)
    while window_is_open() {
        fill(6, 8, 18)
        set_camera(1, 0, cos(0.25), sin(0.25))
        set_camera_pos(0, -3, 0)
        clear_lights()
        add_light(0, -5, 8, 1.0, 0.9, 0.7, 0.8, 0)
        set_color(220, 180, 120)
        cylinder(0, 1.3, 9, 0.8, 0.7, 0.8, 0, 0, 0, 0)
        set_blend(1)
        light_beam(0, -6, 9, 2, 3.0, 255, 220, 160, 0.8)
        light_pool(0, 2, 9, 5.0, 255, 220, 160, 0.9)
        set_blend(0)
        flush_3d()
        present()
    }
}
```

## Older names

`shadow_smooth(softness)` is an older shortcut that sets `tone_smooth` and `tone_bezier` together; use those instead. `toon_highlight(...)` is accepted but does nothing; use `tone_stop` and `tone_soft`.

See also: [Window and Camera](window.md), [3D Shapes and Meshes](shapes.md), [Animation](animation.md).
