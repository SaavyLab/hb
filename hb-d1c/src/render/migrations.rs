use std::path::Path;

use anyhow::{Context, Result};
use quote::quote;

use crate::schema::MigrationSource;

use super::format_tokens;

pub fn render_manifest(
    migrations: &[MigrationSource],
    output_path: &Path,
    project_dir: &Path,
) -> Result<String> {
    let output_dir = output_path
        .parent()
        .context("generated migration manifest has no parent directory")?;
    let entries = migrations
        .iter()
        .map(|migration| {
            let include_path =
                pathdiff::diff_paths(&migration.path, output_dir).with_context(|| {
                    format!(
                        "cannot express migration {} relative to generated manifest {}",
                        migration.path.display(),
                        output_path.display()
                    )
                })?;
            let include_path = include_path
                .to_str()
                .with_context(|| {
                    format!(
                        "migration include path {} is not UTF-8",
                        include_path.display()
                    )
                })?
                .replace('\\', "/");
            let id = &migration.id;
            let checksum = &migration.checksum;
            Ok(quote! {
                Migration {
                    id: #id,
                    sql: include_str!(#include_path),
                    checksum: #checksum,
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;

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
        let output = root.path().join("src/generated/migrations.rs");
        let source = render_manifest(&migrations, &output, root.path()).unwrap();

        assert!(source.find("001_first.sql").unwrap() < source.find("002_second.sql").unwrap());
        assert!(source.contains("include_str!(\"../../db/migrations/001_first.sql\")"));
        assert!(source.contains("sha256:"));
        syn::parse_file(&source).unwrap();
    }
}
