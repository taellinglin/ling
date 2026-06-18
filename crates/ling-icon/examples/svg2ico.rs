//! svg2ico — convert an icon source to a multi-resolution `.ico`.
//!
//!   cargo run -p ling-icon --example svg2ico -- <in.svg|png|ico> <out.ico>
//!
//! Used to pre-render the Ling toolchain's default icon, and handy on its own.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: svg2ico <in.svg|png|ico> <out.ico>");
        std::process::exit(2);
    }
    match ling_icon::write_ico(
        Path::new(&args[1]),
        Path::new(&args[2]),
        ling_icon::DEFAULT_SIZES,
    ) {
        Ok(()) => println!(
            "wrote {} ({} sizes)",
            args[2],
            ling_icon::DEFAULT_SIZES.len()
        ),
        Err(e) => {
            eprintln!("svg2ico: {e}");
            std::process::exit(1);
        },
    }
}
