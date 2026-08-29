use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSource {
    pub id: String,
    pub path: PathBuf,
    pub sql: String,
    pub checksum: String,
}

pub struct Schema {
    connection: Connection,
    migrations: Vec<MigrationSource>,
}

impl Schema {
    pub fn replay(migrations_dir: impl AsRef<Path>) -> Result<Self> {
        let migrations = load_migrations(migrations_dir)?;
        let mut connection =
            Connection::open_in_memory().context("open analysis SQLite database")?;
        replay_sources(&mut connection, &migrations)?;
        Ok(Self {
            connection,
            migrations,
        })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn migrations(&self) -> &[MigrationSource] {
        &self.migrations
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

pub fn load_migrations(migrations_dir: impl AsRef<Path>) -> Result<Vec<MigrationSource>> {
    let migrations_dir = migrations_dir.as_ref();
    collect_sql_files(migrations_dir)?
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(migrations_dir).with_context(|| {
                format!(
                    "migration {} is outside configured directory {}",
                    path.display(),
                    migrations_dir.display()
                )
            })?;
            let id = relative
                .to_str()
                .with_context(|| format!("migration path {} is not UTF-8", relative.display()))?
                .replace('\\', "/");
            let sql = fs::read_to_string(&path)
                .with_context(|| format!("read migration {}", path.display()))?;
            let checksum = format!("sha256:{:x}", Sha256::digest(sql.as_bytes()));
            Ok(MigrationSource {
                id,
                path,
                sql,
                checksum,
            })
        })
        .collect()
}

/// Replays every migration in one transaction. An empty directory is supported and
/// produces an empty schema. Any failure rolls the complete replay back.
pub fn replay_migrations(
    connection: &mut Connection,
    migrations_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let migrations = load_migrations(migrations_dir)?;
    replay_sources(connection, &migrations)?;
    Ok(migrations
        .into_iter()
        .map(|migration| migration.path)
        .collect())
}

fn replay_sources(connection: &mut Connection, migrations: &[MigrationSource]) -> Result<()> {
    let transaction = connection
        .transaction()
        .context("begin migration replay transaction")?;
    for migration in migrations {
        transaction
            .execute_batch(&migration.sql)
            .with_context(|| format!("apply migration {}", migration.path.display()))?;
    }
    transaction
        .commit()
        .context("commit migration replay transaction")?;
    Ok(())
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
    fn loads_stable_ids_content_and_checksums() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(
            root.path().join("nested/001.sql"),
            "CREATE TABLE item(id INTEGER);",
        )
        .unwrap();
        let migrations = load_migrations(root.path()).unwrap();
        assert_eq!(migrations[0].id, "nested/001.sql");
        assert_eq!(migrations[0].sql, "CREATE TABLE item(id INTEGER);");
        assert_eq!(
            migrations[0].checksum,
            "sha256:393eb8bd82575d861b2e18f22efdc6b3d9134f8646690d1ad71629cacf9e5ebb"
        );
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
        assert!(schema.migrations().is_empty());
    }
}
