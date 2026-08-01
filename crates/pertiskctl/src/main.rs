//! `pertiskctl` — node management CLI (Phase 3). Stub for M0.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pertiskctl", about = "Pertisk KOS management CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print CLI and planned API version.
    Version,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!("pertiskctl {}", env!("CARGO_PKG_VERSION"));
            println!("api: planned v1alpha1 (Phase 3)");
        }
    }
}
