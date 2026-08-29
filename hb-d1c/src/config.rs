use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    D1,
    Rusqlite,
}

impl std::fmt::Display for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::D1 => "d1",
            Self::Rusqlite => "rusqlite",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub target: Target,
    pub migrations_dir: String,
    pub queries_dir: String,
    pub out_dir: String,
    pub module_name: String,
    pub emit_schema: bool,
    #[serde(default)]
    pub emit_migrations: bool,
    pub instrument_by_default: bool,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let config: Self = toml::from_str(&source)
            .with_context(|| format!("parse configuration {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "unsupported d1c configuration version {}; expected {}",
                self.version,
                CONFIG_VERSION
            );
        }
        if self.module_name.is_empty() {
            bail!("module_name must not be empty");
        }
        syn::parse_str::<syn::Ident>(self.module_name.trim_end_matches(".rs"))
            .context("module_name must be a Rust identifier")?;
        if self.emit_migrations && self.module_name.trim_end_matches(".rs") == "migrations" {
            bail!("module_name `migrations` conflicts with the generated migration manifest");
        }
        if self.target == Target::Rusqlite && self.instrument_by_default {
            bail!(
                "target rusqlite is incompatible with instrument_by_default; instrumentation is D1-only"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            version: CONFIG_VERSION,
            target: Target::Rusqlite,
            migrations_dir: "migrations".into(),
            queries_dir: "queries".into(),
            out_dir: "generated".into(),
            module_name: "queries".into(),
            emit_schema: false,
            emit_migrations: false,
            instrument_by_default: false,
        }
    }

    #[test]
    fn rejects_unknown_versions() {
        let mut config = config();
        config.version = 2;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("version 2"));
    }

    #[test]
    fn existing_version_one_config_defaults_migration_manifest_off() {
        let source = r#"
version = 1
target = "rusqlite"
migrations_dir = "migrations"
queries_dir = "queries"
out_dir = "generated"
module_name = "queries"
emit_schema = false
instrument_by_default = false
"#;
        let config: Config = toml::from_str(source).unwrap();
        assert!(!config.emit_migrations);
    }

    #[test]
    fn rejects_manifest_and_query_module_name_collision() {
        let mut config = config();
        config.emit_migrations = true;
        config.module_name = "migrations.rs".into();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("conflicts"));
    }

    #[test]
    fn rejects_target_incompatible_instrumentation() {
        let mut config = config();
        config.instrument_by_default = true;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("D1-only"));
    }
}
