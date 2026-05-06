use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ling")]
#[command(about = "The Omniglot Systems Language", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Compile {
        #[arg(short, long)]
        input: String,
        #[arg(short, long)]
        output: Option<String>,
    },
    Repl,
}
