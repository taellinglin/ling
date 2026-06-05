# Builtins — UI Toolkit

A vector, theme-coloured immediate-mode widget library (HUD, meters, interface controls, game
UI, plus a few faux-3D widgets). All widgets are drawn each frame; interactive ones read the
mouse and return their state. Set the palette once with `ui_theme`; every widget takes an
optional trailing `r,g,b` to override its main colour.

```ling
ui_theme(0,210,255,  60,255,180,  40,60,95,  255,90,90,  190,235,255,  10,16,24)
```

## HUD / sci-fi overlays
`ui_radar(cx,cy,r,sweep)` · `ui_compass(x,y,w,h,heading)` · `ui_reticle(cx,cy,r,spread)` ·
`ui_target(x,y,w,h,lock)` · `ui_panel(x,y,w,h,bevel)` · `ui_scanlines(x,y,w,h,density)`

## Meters & gauges
`ui_bar(x,y,w,h,value,max)` · `ui_segbar(…,segs)` · `ui_gauge(cx,cy,r,value,max)` ·
`ui_ring(cx,cy,r,value,max)` · `ui_vu(x,y,w,h,levels_list)` · `ui_spark(x,y,w,h,values_list)` ·
`ui_battery(x,y,w,h,value,max)`

## Interface controls (interactive — return state)
`ui_button(x,y,w,h)` → 1 when clicked · `ui_toggle(x,y,w,h,state)` → new state ·
`ui_slider(x,y,w,value,min,max)` → new value · `ui_checkbox(x,y,size,checked)` → new checked ·
`ui_tabs(x,y,w,h,count,active)` → active index · `ui_progress(x,y,w,h,frac)` ·
`ui_tooltip(x,y,w,h)` · `ui_stepper(x,y,w,h,value,step)` → new value

## Game UI
`ui_healthbar(x,y,w,h,hp,max,pulse)` · `ui_cooldown(cx,cy,r,frac)` ·
`ui_counter(x,y,dw,dh,value,digits)` (7-segment) · `ui_minimap(x,y,w,h)` ·
`ui_dpad(cx,cy,r)` → direction · `ui_slotgrid(x,y,cols,rows,cell,sel)` ·
`ui_vignette(intensity, r,g,b)`

## Faux-3D in 2D space
`ui_gauge3d(cx,cy,r,value,spin)` · `ui_panel3d(x,y,w,h,depth)` · `ui_radar3d(cx,cy,r,tilt,sweep)`

## Interface sounds
`ui_sound("click"|"hover"|"confirm"|"error"|"toggle"|"tick")` · `audio_blip(freq,dur,wave,amp)`

See `examples/ui/hud_showcase.ling`.
