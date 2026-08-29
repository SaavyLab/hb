use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::{config::Config, generator, schema::Schema};

mod init;
mod watch;

pub use init::InitArgs;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize d1c in the current project
    Init(InitArgs),
    /// Generate Rust bindings from migrations and annotated SQL
    #[command(alias = "gen")]
    Generate,
    /// Check committed generated output without rewriting it
    Check,
    /// Watch migrations and queries and regenerate after changes
    Watch,
    /// Print the schema produced by configured migrations
    DumpSchema,
}

pub fn run(command: &Command, config_path: &Path) -> Result<()> {
    if let Command::Init(args) = command {
        return init::run(args);
    }
    let config = Config::load(config_path)
        .with_context(|| format!("load {} (run `d1c init` first)", config_path.display()))?;
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    match command {
        Command::Generate => {
            let report = generator::generate(&config, base)?;
            println!(
                "generated {} file(s), {} unchanged, {} removed",
                report.written.len(),
                report.unchanged.len(),
                report.removed.len()
            );
            Ok(())
        }
        Command::Check => {
            let report = generator::check(&config, base)?;
            println!(
                "generated output is current ({} files)",
                report.checked.len()
            );
            Ok(())
        }
        Command::Watch => watch::run(&config, base),
        Command::DumpSchema => {
            let schema = Schema::replay(base.join(&config.migrations_dir))?;
            print!("{}", schema.dump()?);
            Ok(())
        }
        Command::Init(_) => unreachable!("handled before configuration loading"),
    }
}

pub fn absolute_config_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
