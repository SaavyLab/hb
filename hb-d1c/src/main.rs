use std::fs;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::{cli::Cli, commands::Command, utils::sql::replay_migrations};

mod cli;
mod commands;
mod config;
mod migrations;
pub mod utils;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let conn = Connection::open_in_memory().context("open in-memory sqlite connection")?;
    if let Command::Init(args) = &cli.command {
        return commands::run_init(&conn, args);
    }

    let config = D1CConfig::load().context("failed to load d1c.toml (run `d1c init` first)")?;
    replay_migrations(&conn, &config.migrations_dir)?;

    commands::run(&conn, &cli, &config)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct D1CConfig {
    pub migrations_dir: String,
    pub queries_dir: String,
    pub out_dir: String,
    pub module_name: String,
    pub emit_schema: bool,
    #[serde(default)] // To support existing configs without this field
    pub instrument_by_default: bool,
}

impl D1CConfig {
    pub fn load() -> Result<Self> {
        let config = toml::from_str(&fs::read_to_string("d1c.toml")?)?;
        Ok(config)
    }
}
