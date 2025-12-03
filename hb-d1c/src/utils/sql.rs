use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;

/// This is in-memory, this does not apply migrations to your D1 database.
/// This is called a single time at entry of the CLI so we don't accidentally
/// re-apply migrations for anything that needs the in-memory database.
pub fn replay_migrations(conn: &Connection, migrations_dir: &str) -> Result<()> {
    let entries = collect_sql_files(PathBuf::from(migrations_dir))?;

    for entry in entries {
        let sql = fs::read_to_string(&entry)
            .with_context(|| format!("read migration {}", entry.display()))?;
        conn.execute_batch(&sql)
            .with_context(|| format!("apply migration {}", entry.display()))?;
    }

    Ok(())
}

/// Used to collect the migrations as well as the queries
pub fn collect_sql_files(dir: PathBuf) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        anyhow::bail!("Directory not found: {}", dir.display());
    }

    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().map(|ext| ext == "sql").unwrap_or(false))
        .filter(|path| path.file_name().map(|n| n != "schema.sql").unwrap_or(true))
        .collect();

    files.sort();
    Ok(files)
}
