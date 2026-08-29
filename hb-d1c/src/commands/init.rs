use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use inquire::{Confirm, Select, Text};
use serde::Deserialize;

use crate::config::{Config, Target, CONFIG_VERSION};

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Preview configuration without writing files
    #[arg(long)]
    pub dry_run: bool,
    /// Select a renderer without an interactive target prompt
    #[arg(long, value_enum)]
    pub target: Option<InitTarget>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum InitTarget {
    D1,
    Rusqlite,
}

impl std::fmt::Display for InitTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::D1 => "d1 (Cloudflare Workers)",
            Self::Rusqlite => "rusqlite (synchronous SQLite)",
        })
    }
}

impl From<InitTarget> for Target {
    fn from(value: InitTarget) -> Self {
        match value {
            InitTarget::D1 => Self::D1,
            InitTarget::Rusqlite => Self::Rusqlite,
        }
    }
}

pub fn run(args: &InitArgs) -> Result<()> {
    let selected = match args.target {
        Some(target) => target,
        None => Select::new(
            "Renderer target",
            vec![InitTarget::D1, InitTarget::Rusqlite],
        )
        .prompt()?,
    };
    let target = Target::from(selected);

    let detected = if target == Target::D1 {
        detect_d1_migrations()?
    } else {
        None
    };
    let migrations_default = detected.as_deref().unwrap_or("db/migrations");
    let migrations_dir = Text::new("Migration SQL directory")
        .with_default(migrations_default)
        .prompt()?;
    let queries_dir = Text::new("Annotated query SQL directory")
        .with_default("db/queries")
        .prompt()?;
    let out_dir = Text::new("Generated Rust output directory")
        .with_default("src/generated")
        .prompt()?;
    let module_name = Text::new("Generated root module name")
        .with_default("queries")
        .prompt()?;
    let emit_schema = Confirm::new("Emit queries/schema.sql for inspection?")
        .with_default(true)
        .prompt()?;
    let instrument_by_default = if target == Target::D1 {
        Confirm::new("Add tracing instrumentation to D1 functions?")
            .with_default(false)
            .prompt()?
    } else {
        false
    };

    let config = Config {
        version: CONFIG_VERSION,
        target,
        migrations_dir,
        queries_dir,
        out_dir,
        module_name,
        emit_schema,
        instrument_by_default,
    };
    config.validate()?;
    let rendered = toml::to_string_pretty(&config)?;
    if args.dry_run {
        print!("{rendered}");
        return Ok(());
    }

    fs::write("d1c.toml", rendered).context("write d1c.toml")?;
    fs::create_dir_all(&config.migrations_dir)?;
    fs::create_dir_all(&config.queries_dir)?;
    fs::create_dir_all(&config.out_dir)?;
    println!("created d1c.toml for target {}", config.target);
    println!("next: add migrations and queries, then run `d1c generate`");
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Wrangler {
    #[serde(default)]
    d1_databases: Vec<D1Database>,
}

#[derive(Debug, Deserialize)]
struct D1Database {
    migrations_dir: Option<String>,
}

fn detect_d1_migrations() -> Result<Option<String>> {
    let Some(path) = find_upwards("wrangler.toml")? else {
        return Ok(None);
    };
    let source = fs::read_to_string(&path)?;
    let wrangler: Wrangler =
        toml::from_str(&source).with_context(|| format!("parse {}", path.display()))?;
    let directories = wrangler
        .d1_databases
        .into_iter()
        .filter_map(|database| database.migrations_dir)
        .collect::<Vec<_>>();
    Ok(if directories.len() == 1 {
        directories.into_iter().next()
    } else {
        None
    })
}

fn find_upwards(name: &str) -> Result<Option<PathBuf>> {
    let mut current = env::current_dir()?;
    for _ in 0..=5 {
        let candidate = current.join(name);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        if !current.pop() {
            break;
        }
    }
    Ok(None)
}
