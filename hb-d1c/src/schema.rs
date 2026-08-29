use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::Connection;
use walkdir::WalkDir;

pub struct Schema {
    connection: Connection,
}

impl Schema {
    pub fn replay(migrations_dir: impl AsRef<Path>) -> Result<Self> {
        let mut connection =
            Connection::open_in_memory().context("open analysis SQLite database")?;
        replay_migrations(&mut connection, migrations_dir)?;
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn dump(&self) -> Result<String> {
        let mut statement = self.connection.prepare(
            "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let sql = rows.collect::<rusqlite::Result<Vec<_>>>()?.join(";\n\n");
        if sql.is_empty() {
            Ok(sql)
        } else {
            Ok(format!("{sql};\n"))
        }
    }
}

/// Replays every migration in one transaction. An empty directory is supported and
/// produces an empty schema. Any failure rolls the complete replay back.
pub fn replay_migrations(
    connection: &mut Connection,
    migrations_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let files = collect_sql_files(migrations_dir)?;
    let transaction = connection
        .transaction()
        .context("begin migration replay transaction")?;
    for path in &files {
        let sql = fs::read_to_string(path)
            .with_context(|| format!("read migration {}", path.display()))?;
        transaction
            .execute_batch(&sql)
            .with_context(|| format!("apply migration {}", path.display()))?;
    }
    transaction
        .commit()
        .context("commit migration replay transaction")?;
    Ok(files)
}

pub fn collect_sql_files(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        anyhow::bail!("SQL directory not found: {}", dir.display());
    }
    let mut files = WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .filter(|path| path.file_name().is_some_and(|name| name != "schema.sql"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn discovers_sorted_paths_and_excludes_schema_dump() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("b.sql"), "CREATE TABLE b(id INTEGER);").unwrap();
        fs::write(root.path().join("a.sql"), "CREATE TABLE a(id INTEGER);").unwrap();
        fs::write(root.path().join("schema.sql"), "broken").unwrap();
        let files = collect_sql_files(root.path()).unwrap();
        assert_eq!(files[0].file_name().unwrap(), "a.sql");
        assert_eq!(files[1].file_name().unwrap(), "b.sql");
    }

    #[test]
    fn migration_failure_rolls_back_every_file() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("1.sql"), "CREATE TABLE first(id INTEGER);").unwrap();
        fs::write(root.path().join("2.sql"), "CREATE TABL broken;").unwrap();
        let mut connection = Connection::open_in_memory().unwrap();
        let error = replay_migrations(&mut connection, root.path()).unwrap_err();
        assert!(error.to_string().contains("2.sql"));
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn empty_directory_is_an_empty_schema() {
        let root = tempdir().unwrap();
        let schema = Schema::replay(root.path()).unwrap();
        assert_eq!(schema.dump().unwrap(), "");
    }
}
