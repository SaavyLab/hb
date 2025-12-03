use crate::commands::Command;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "d1c",
    version,
    about = "Type-safe SQL generator for Cloudflare D1 + Rust"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn parse() -> Cli {
        Parser::parse()
    }
}
