//! `pertisk-sign` — generate trust keys and sign OS upgrade bundles.

use std::fs;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use pertisk_update::{
    build_manifest, generate_keypair, load_signing_key, sign_manifest, verify_bundle,
};

#[derive(Parser)]
#[command(name = "pertisk-sign", about = "Sign Pertisk OS upgrade bundles")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate Ed25519 trust keypair (hex files).
    Keygen {
        #[arg(long)]
        secret: PathBuf,
        #[arg(long)]
        public: PathBuf,
    },
    /// Hash artifacts, write manifest.json + manifest.sig into a bundle dir.
    Sign {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        secret: PathBuf,
        #[arg(long, value_delimiter = ',')]
        artifacts: Vec<String>,
    },
    /// Verify bundle against a public trust key.
    Verify {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        public: PathBuf,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("pertisk-sign: {err:#}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Keygen { secret, public } => {
            generate_keypair(&secret, &public)?;
            println!("wrote {} and {}", secret.display(), public.display());
        }
        Commands::Sign {
            bundle,
            version,
            secret,
            artifacts,
        } => {
            let names: Vec<&str> = if artifacts.is_empty() {
                vec!["kernel", "initramfs"]
            } else {
                artifacts.iter().map(String::as_str).collect()
            };
            let manifest = build_manifest(&bundle, &version, &names)?;
            let json = manifest.to_canonical_json()?;
            fs::write(bundle.join("manifest.json"), &json)?;
            let sig = sign_manifest(&load_signing_key(&secret)?, &json);
            fs::write(bundle.join("manifest.sig"), sig)?;
            println!(
                "signed {} artifacts for version {version}",
                names.len()
            );
        }
        Commands::Verify { bundle, public } => {
            let verified = verify_bundle(&bundle, &public)?;
            println!(
                "ok version={} artifacts={}",
                verified.manifest.version,
                verified.manifest.artifacts.len()
            );
        }
    }
    Ok(())
}
