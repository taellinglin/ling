# oTODO

## Ling: make `ling run examples/hello.ling` print output
- [ ] Implement minimal lexer for subset used by examples/hello.ling (string literals, identifiers, keywords: bind/do/print/start)
- [ ] Implement minimal parser for subset: `bind <name> = do { <stmts> }` with nested `bind <name> = <expr>` and `print(<expr>)`
- [ ] Implement small interpreter that evaluates this subset and prints to stdout
- [x] Wire CLI `ling run <file>` to the interpreter (enabled in this build)
- [x] Add integration test that runs `examples/hello.ling` and asserts stdout contains `Hello, World!`

## Examples: gfx compatibility (ling-graphics)
- [x] Make `examples/gfx.ling` parse and run by removing unsupported `&&` conditions

## Requested: rotating cube “window/viewport” instance API
- [ ] Add Rust-to-language integration so `.ling` can trigger a render loop
- [ ] Add a small “software viewport” presenter (RGBA framebuffer -> terminal output)
- [ ] Add sample rotating cube program (or Rust demo) using `ling-graphics`:
  - cube mesh: `ling_graphics::geometry::cube(half)`
  - scene: `ling_graphics::scene::Scene`
  - camera: `ling_graphics::camera::Camera3D`
  - renderer: `ling_graphics::renderer::SoftwareRenderer`
  - update: rotate transform over frames

