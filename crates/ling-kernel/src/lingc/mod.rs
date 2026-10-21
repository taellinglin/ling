//! `ling run` inside LingOS: the Ling front end (lexer → parser → `use`
//! flattening → MIR lowering, generated no_std copies of the compiler's own
//! sources) plus a MIR interpreter over the kernel's app runtime. The same
//! language, builtins, graphics (`open_window`/`px_*`), input, files and DAW
//! calls that AOT-bundled apps get — on the Pi and on x86.

pub mod ast;
pub mod entry;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod modules;
pub mod parser;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Example programs from the compiler's own `examples/`, baked into every
/// image so `ling run` has something to run before the SD card / network can
/// supply programs; each user's home gets a copy (`Scripts`, `Graphics`).
pub const EXAMPLES: &[(&str, &str)] = &[
    ("Scripts/fib.ling", include_str!("../../../../examples/basics/fib.ling")),
    ("Graphics/spiral.ling", include_str!("../../../../examples/lingos/spiral.ling")),
];

/// `ling demo`: a built-in program exercising the whole pipeline (functions,
/// recursion, if/else, for/while, lists, indexing, strings, globals). Like
/// every Ling program it has top-level `fn`/`bind` items and a `main` block.
pub const DEMO: &str = "fn sq(n) { n * n }\n\
fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\
bind x = 9\n\
bind main = do {\n\
    print(\"ling running in LingOS\")\n\
    print(\"sq(\", x, \") = \", sq(x), \"   6 * 7 = \", 6 * 7)\n\
    bind xs = [3, 1, 4, 1, 5, 9]\n\
    print(\"list \", xs, \"  len \", len(xs), \"  xs[2] = \", xs[2])\n\
    bind s = \"Ling\" + \" \" + \"OS\"\n\
    print(s, \" has \", str_len(s), \" chars, upper \", str_upper(s))\n\
    for i in 0..6 { print(\"fib(\", i * 3, \") = \", fib(i * 3)) }\n\
    bind total = 0\n\
    bind i = 0\n\
    while i < 10 { bind total = total + i  bind i = i + 1 }\n\
    print(\"sum 0..9 = \", total)\n\
    if x > 4 { print(\"x is big\") } else { print(\"x is small\") }\n\
}\n";

/// Compile `source` (whose `use`s resolve against `dir`) to MIR.
pub fn compile(source: &str, dir: &str) -> Result<ir::MirProgram, String> {
    let ast = parser::parse(source)?;
    let flat = modules::flatten(&ast, dir)?;
    let flat = lower::dedup_items(flat);
    Ok(lower::lower_program_with(&flat, true))
}

/// The program `path` names: the file itself, a folder's `main.ling`, or
/// `<path>.ling`.
pub fn locate(path: &str) -> Option<String> {
    let p = path.trim_start_matches("./").trim_end_matches('/');
    if p.is_empty() {
        return None;
    }
    for c in [String::from(p), format!("{p}/main.ling"), format!("{p}.ling")] {
        if modules::source_exists(&c) {
            return Some(c);
        }
    }
    None
}

/// Run a program's MIR to completion with its own globals, state store and
/// window (the caller's are put aside and restored), with `args` as its
/// command line. Returns the runtime error, if one stopped it.
pub fn run_program(prog: &ir::MirProgram, args: &[&str]) -> Option<String> {
    let owned: Vec<String> = args.iter().map(|a| String::from(*a)).collect();
    let mut err = None;
    crate::lingapp::with_own_window(|| {
        crate::lingrt::run_isolated_with(|| {
            crate::lingrt::set_args(&owned);
            let mut it = interp::Interp::new(prog);
            it.run();
            err = it.error.take();
            0
        });
    });
    err
}

/// `ling run <path> [args]`: read, compile and run a program (see [`locate`]).
pub fn run_file(path: &str, args: &[&str]) -> Result<(), String> {
    let Some(found) = locate(path) else {
        return Err(format!("ling run: cannot find '{path}'"));
    };
    let Some(src) = modules::read_source(&found) else {
        return Err(format!("ling run: cannot read '{found}'"));
    };
    let prog = compile(&src, modules::parent(&found)).map_err(|e| format!("{found}: {e}"))?;
    let mut argv: Vec<&str> = Vec::with_capacity(args.len() + 1);
    argv.push(&found);
    argv.extend_from_slice(args);
    match run_program(&prog, &argv) {
        None => Ok(()),
        Some(e) => Err(format!("ling run: {e}")),
    }
}

/// `ling eval <source>`: run a snippet (imports resolve against the root).
pub fn run_source(src: &str) -> Result<(), String> {
    let prog = compile(src, "")?;
    match run_program(&prog, &["eval"]) {
        None => Ok(()),
        Some(e) => Err(format!("ling: {e}")),
    }
}
