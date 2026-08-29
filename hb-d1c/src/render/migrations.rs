use std::path::Path;

use anyhow::Result;
use quote::quote;

use crate::schema::MigrationSource;

use super::format_tokens;

pub fn render_manifest(migrations: &[MigrationSource], project_dir: &Path) -> Result<String> {
    let entries = migrations.iter().map(|migration| {
        let id = &migration.id;
        let sql = &migration.sql;
        let checksum = &migration.checksum;
        quote! {
            Migration {
                id: #id,
                sql: #sql,
                checksum: #checksum,
            }
        }
    });

    format_tokens(
        quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct Migration {
                pub id: &'static str,
                pub sql: &'static str,
                pub checksum: &'static str,
            }

            pub const MIGRATIONS: &[Migration] = &[
                #(#entries,)*
            ];
        },
        project_dir,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::schema::load_migrations;

    use super::*;

    #[test]
    fn renders_sorted_embedded_migrations_with_checksums() {
        let root = tempdir().unwrap();
        let migrations_dir = root.path().join("db/migrations");
        fs::create_dir_all(&migrations_dir).unwrap();
        fs::write(
            migrations_dir.join("002_second.sql"),
            "ALTER TABLE item ADD COLUMN name TEXT;",
        )
        .unwrap();
        fs::write(
            migrations_dir.join("001_first.sql"),
            "CREATE TABLE item(id INTEGER);",
        )
        .unwrap();
        let migrations = load_migrations(&migrations_dir).unwrap();
        let source = render_manifest(&migrations, root.path()).unwrap();

        assert!(source.find("001_first.sql").unwrap() < source.find("002_second.sql").unwrap());
        assert!(source.contains("sql: \"CREATE TABLE item(id INTEGER);\""));
        assert!(!source.contains("include_str!"));
        assert!(source.contains("sha256:"));
        syn::parse_file(&source).unwrap();
    }
}
