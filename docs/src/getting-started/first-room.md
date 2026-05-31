# Your First 3D Room

A minimal windowed scene with a floor grid and a spinning chakra mandala.

```ling
令 PI = 3.14159265

令 启 = 执 {
    open_fullscreen("My First Room")
    set_ambient(0.15)

    令 帧 = 0

    循 window_is_open() {
        fill(0, 0, 8)           # deep-blue background

        令 ry = 帧 * 0.0006
        set_camera(cos(ry), sin(ry), 1.0, 0.0)
        set_camera_pos(sin(ry) * 6.0, 1.0, cos(ry) * 6.0)
        set_zdist(2.0)

        # Floor grid
        vtex_grid(0, -3, 0,  1,0,0,  0,0,1,  8, 8,  1.0, 1.0,  帧*0.0001, 1.57)

        # Spinning mandala
        vtex_chakra(0, 0, 0,  1,0,0,  0,1,0,  2.5, 8,  帧*0.0005, 3.50)
        vtex_lotus( 0, 0, 0,  1,0,0,  0,1,0,  0.6, 2.2, 8,  帧*0.0004, 2.10)

        display()
        令 帧 = 帧 + 1
    }
}
```

Save as `room.ling` and run:

```sh
ling room.ling
```

Press **Escape** or close the window to exit.
