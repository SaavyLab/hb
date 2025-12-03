use anyhow::{Context, Error, Result};
use console::{style, Term};
use rusqlite::Connection;
use std::{
    env,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use clap::Args;
use toml::Table;

use crate::{commands::dump_schema::dump_schema, D1CConfig};

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Dry run - don't write to files
    #[arg(long, default_value = "false")]
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
struct FoundDb {
    name: String,
    binding: String,
    migrations_dir: String,
}

impl std::fmt::Display for FoundDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (binding: {}, dir: {})",
            self.name, self.binding, self.migrations_dir
        )
    }
}

pub fn run(conn: &Connection, args: &InitArgs) -> Result<(), Error> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} {}",
        style("d1c").bold().cyan(),
        style("init").dim(),
    ))?;

    // 1. Detect Context (Wrangler & Cargo)
    let wrangler_info = find_file_upwards("wrangler.toml")?;
    let cargo_info = find_file_upwards("Cargo.toml")?;

    // 2. Analyze Monorepo/Workspace status
    if let Some((cargo_path, cargo_content)) = &cargo_info {
        let cargo_toml = cargo_content
            .parse::<Table>()
            .context("Failed to parse Cargo.toml")?;

        if cargo_toml.get("workspace").is_some() && cargo_toml.get("package").is_none() {
            term.write_line(&format!(
                "{} Detected Cargo Workspace root at {:?}",
                style("⚠️").yellow(),
                cargo_path.parent().unwrap(),
            ))?;
            term.write_line(
                "   d1c works best when run inside the specific Worker crate directory.",
            )?;

            let proceed = inquire::Confirm::new("Are you sure you want to initialize here?")
                .with_default(false)
                .prompt()?;

            if !proceed {
                term.write_line(&format!(
                    "{}",
                    style("Aborting. Please cd into your worker crate.").red()
                ))?;
                return Ok(());
            }
        }
    }

    let mut migrations_dir: Option<String> = None;
    let mut use_detected_migrations_dir = false;

    // 3. Process Wrangler Configuration
    if let Some((_, content)) = wrangler_info {
        term.write_line(&format!("{} Found wrangler.toml", style("✨").green()))?;

        let found_dbs = parse_d1_databases(&content)?;

        match found_dbs.len() {
            0 => {
                term.write_line(&format!(
                    "{} Found wrangler.toml but no 'd1_databases' configured",
                    style("⚠️").yellow()
                ))?;
            }
            1 => {
                let db = &found_dbs[0];
                term.write_line(&format!(
                    "   Detected D1 database: {}",
                    style(&db.name).bold()
                ))?;

                use_detected_migrations_dir =
                    inquire::Confirm::new(&format!("Use '{}' for migrations?", db.migrations_dir))
                        .with_default(true)
                        .prompt()?;

                if use_detected_migrations_dir {
                    migrations_dir = Some(db.migrations_dir.clone());
                }
            }
            _ => {
                term.write_line("   Found multiple D1 databases.")?;
                let selection =
                    inquire::Select::new("Which database do you want to use?", found_dbs)
                        .prompt()?;

                migrations_dir = Some(selection.migrations_dir);
                use_detected_migrations_dir = true;
            }
        }
    } else {
        term.write_line(&format!(
            "{} No wrangler.toml found - using manual configuration",
            style("💡").yellow()
        ))?;
    }

    // 4. Interactive Configuration
    if migrations_dir.is_none() {
        // If we didn't find config, show the help text for how to add it
        if !use_detected_migrations_dir {
            term.write_line("   Add a D1 database to wrangler.toml:")?;
            term.write_line(&format!("{}", style("   [[d1_databases]]").dim()))?;
            term.write_line(&format!("{}", style("   binding = \"DB\"").dim()))?;
            term.write_line(&format!("{}", style("   database_name = \"my-db\"").dim()))?;
            term.write_line(&format!(
                "{}",
                style("   migrations_dir = \"db/migrations\"").dim()
            ))?;
            term.write_line("")?;
        }

        let manual_migrations_dir = inquire::Text::new("Where do your migrations live?")
            .with_default("migrations")
            .prompt()?;
        migrations_dir = Some(manual_migrations_dir);
    }

    // Determine smart defaults for queries based on migration location
    let queries_default = if use_detected_migrations_dir {
        let mig_dir = migrations_dir.as_ref().unwrap();
        if mig_dir.contains("migrations") {
            mig_dir.replace("migrations", "queries")
        } else {
            format!("{mig_dir}/../queries")
        }
    } else {
        "db/queries".to_string()
    };

    let queries_dir = inquire::Text::new("Where do you want the queries to live?")
        .with_default(&queries_default)
        .prompt()?;

    let out_dir = inquire::Text::new("Where do you want to write the generated code?")
        .with_default("src/d1c")
        .prompt()?;

    let module_name = inquire::Text::new("What is the name of the module for the generated code?")
        .with_default("queries")
        .prompt()?;

    let emit_schema = inquire::Confirm::new("Do you want to emit the schema.sql file?")
        .with_help_message(
            "Not required by d1c, but useful for viewing your schema. \
         D1 doesn't provide an easy way to inspect schema otherwise.",
        )
        .with_default(true)
        .prompt()?;

    let instrument_by_default = inquire::Confirm::new(
        "Do you want to auto-instrument queries with tracing?",
    )
    .with_help_message(
        "Adds #[tracing::instrument] to generated functions. Requires `tracing` crate dependency. \
         Works with Cloudflare Workers Observability when traces/logs are enabled.",
    )
    .with_default(false)
    .prompt()?;

    let config = D1CConfig {
        migrations_dir: migrations_dir.unwrap(),
        queries_dir,
        out_dir,
        module_name,
        emit_schema,
        instrument_by_default,
    };

    let config_string = toml::to_string_pretty(&config)?;

    // 5. Execution
    if !args.dry_run {
        let mut config_file = File::create("d1c.toml")?;
        config_file.write_all(config_string.as_bytes())?;

        fs::create_dir_all(&config.queries_dir)?;
        fs::create_dir_all(&config.out_dir)?;

        if config.emit_schema {
            let schema_rows = dump_schema(conn)?;
            let schema_string = schema_rows
                .iter()
                .map(|row| row.sql.clone())
                .collect::<Vec<String>>()
                .join("\n\n");
            let mut schema_file = File::create(Path::new(&config.queries_dir).join("schema.sql"))?;
            schema_file.write_all(schema_string.as_bytes())?;
        }

        // Create a sample query file if the directory is empty and user confirms
        let example_path = Path::new(&config.queries_dir).join("example.sql");
        if !example_path.exists() {
            // Check if dir is empty (except potentially schema.sql)
            let is_empty = fs::read_dir(&config.queries_dir)?
                .filter_map(|e| e.ok())
                .all(|e| e.file_name() == "schema.sql");

            if is_empty {
                let create_example =
                    inquire::Confirm::new("Do you want to create an example query file?")
                        .with_default(true)
                        .prompt()?;

                if create_example {
                    let mut example_file = File::create(&example_path)?;
                    example_file.write_all(
                        b"-- name: ListExample :many\nSELECT 1 as id, 'hello' as message;",
                    )?;
                }
            }
        }

        term.write_line("")?;
        term.write_line(&format!(
            "{} Created {}/",
            style("✅").green(),
            config.queries_dir
        ))?;
        term.write_line("")?;
        term.write_line(&format!(
            "{} Created {}/",
            style("✅").green(),
            config.out_dir
        ))?;
        term.write_line("")?;
        if config.emit_schema {
            term.write_line(&format!(
                "{} Created {}/schema.sql",
                style("✅").green(),
                config.queries_dir
            ))?;
            term.write_line("")?;
        }
        term.write_line(&format!("{} Created d1c.toml", style("✅").green()))?;
        term.write_line("")?;
        term.write_line(&format!("{}", style("Next steps:").bold()))?;
        term.write_line(&format!(
            "  - Create your first query in {}/example.sql",
            &config.queries_dir
        ))?;
        term.write_line(&format!(
            "  - Run {} to generate typed Rust bindings",
            style("d1c generate").yellow()
        ))?;
        term.write_line("  - Import them in your Worker:")?;
        term.write_line("")?;
        term.write_line(&format!(
            "{}",
            style("      mod d1c; // in lib.rs or main.rs").dim()
        ))?;
        term.write_line(&format!(
            "{}",
            style(format!("      use crate::d1c::{}::*;", &config.module_name)).dim()
        ))?;
        term.write_line(&format!(
            "{}",
            style(format!(
                "      // or specific modules: use crate::d1c::{}::example::*;",
                &config.module_name
            ))
            .dim()
        ))?;
        term.write_line("")?;
        term.write_line(&format!("{} Happy querying!", style("🚀").green()))?;
    } else {
        term.write_line(&format!(
            "{}",
            style("================================================").dim()
        ))?;
        term.write_line(&format!(
            "{} DRY RUN: Would have created d1c.toml with the following contents:",
            style("⚠️").yellow()
        ))?;
        term.write_line(&format!(
            "{}",
            style("================================================").dim()
        ))?;
        term.write_line(&config_string)?;
    }

    Ok(())
}

const MAX_DEPTH: usize = 5;

/// Searches upwards for a file, returning (PathBuf, String content) if found
fn find_file_upwards(filename: &str) -> Result<Option<(PathBuf, String)>> {
    let mut depth = 0;
    let mut current_dir = env::current_dir()?;

    while depth < MAX_DEPTH {
        let candidate = current_dir.join(filename);
        if candidate.is_file() {
            let content = fs::read_to_string(&candidate)?;
            return Ok(Some((candidate, content)));
        }

        if !current_dir.pop() {
            break;
        }
        depth += 1;
    }

    Ok(None)
}

fn parse_d1_databases(wrangler_toml: &str) -> Result<Vec<FoundDb>> {
    let parsed = wrangler_toml
        .parse::<Table>()
        .context("Failed to parse wrangler.toml")?;
    let mut results = Vec::new();

    if let Some(toml::Value::Array(dbs)) = parsed.get("d1_databases") {
        for db in dbs {
            if let Some(table) = db.as_table() {
                let migrations_dir = table.get("migrations_dir").and_then(|v| v.as_str());
                let binding = table.get("binding").and_then(|v| v.as_str());
                let name = table.get("database_name").and_then(|v| v.as_str());

                // We strictly require migrations_dir to consider it a candidate for d1c
                if let (Some(dir), Some(bind)) = (migrations_dir, binding) {
                    results.push(FoundDb {
                        name: name.unwrap_or("unnamed-db").to_string(),
                        binding: bind.to_string(),
                        migrations_dir: dir.to_string(),
                    });
                }
            }
        }
    }

    Ok(results)
}
