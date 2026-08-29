use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use hb_d1c::commands::{self, Command};

#[derive(Debug, Parser)]
#[command(
    name = "d1c",
    version,
    about = "Strict SQL-to-Rust generator for Cloudflare D1 and rusqlite"
)]
struct Cli {
    /// Configuration file
    #[arg(long, default_value = "d1c.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = commands::absolute_config_path(cli.config)?;
    commands::run(&cli.command, &config)
}
