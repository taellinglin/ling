# oTODO

## Ling: make `ling-lang examples/hello.ling` print output
- [ ] Implement minimal lexer for subset used by examples/hello.ling (string literals, identifiers, keywords: bind/do/print/start)
- [ ] Implement minimal parser for subset: `bind <name> = do { <stmts> }` with nested `bind <name> = <expr>` and `print(<expr>)`
- [ ] Implement small interpreter that evaluates this subset and prints to stdout
- [ ] Wire CLI `ling run <file>` (or equivalent) to the interpreter
- [ ] Implement `src/bin/lingc.rs` driver to call interpreter for `run`
- [ ] Add integration test that runs `examples/hello.ling` and asserts stdout contains `Hello, World!`


