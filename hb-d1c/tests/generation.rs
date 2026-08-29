use std::{fs, path::Path, process::Command};

use hb_d1c::{check, Config};
use rusqlite::types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};
use tempfile::tempdir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionOrdinal(pub i64);

impl ToSql for SessionOrdinal {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0))
    }
}

impl FromSql for SessionOrdinal {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        i64::column_result(value).map(Self)
    }
}

#[path = "fixtures/rusqlite_consumer/src/generated/migrations.rs"]
mod generated_migrations;

#[path = "fixtures/rusqlite_consumer/src/generated/queries/records.rs"]
mod generated_records;

#[test]
fn committed_goldens_are_current_valid_rust_and_target_isolated() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for fixture in ["rusqlite_consumer", "d1"] {
        let root = manifest.join("tests/fixtures").join(fixture);
        let config = Config::load(root.join("d1c.toml")).unwrap();
        check(&config, &root).unwrap();
        let source = fs::read_to_string(if fixture == "d1" {
            root.join("generated/queries/records.rs")
        } else {
            root.join("src/generated/queries/records.rs")
        })
        .unwrap();
        syn::parse_file(&source).unwrap();
        let manifest_source = fs::read_to_string(if fixture == "d1" {
            root.join("generated/migrations.rs")
        } else {
            root.join("src/generated/migrations.rs")
        })
        .unwrap();
        syn::parse_file(&manifest_source).unwrap();
        assert!(manifest_source.contains("CREATE TABLE records"));
        assert!(!manifest_source.contains("include_str!"));
        if fixture == "d1" {
            assert!(source.contains("worker::D1Database"));
            assert!(source.contains("pub async fn"));
            assert!(!source.contains("rusqlite"));
        } else {
            assert!(source.contains("rusqlite::named_params!"));
            assert!(!source.contains("worker::"));
            assert!(!source.contains("D1Database"));
            assert!(!source.contains("async fn"));
            assert!(!source.contains("hb_d1c"));
        }
    }
}

#[test]
fn generated_rusqlite_code_executes_all_cardinalities_and_transactions() {
    use generated_records::*;

    let mut connection = Connection::open_in_memory().unwrap();
    for migration in generated_migrations::MIGRATIONS {
        assert!(migration.checksum.starts_with("sha256:"));
        connection.execute_batch(migration.sql).unwrap();
    }
    assert_eq!(generated_migrations::MIGRATIONS[0].id, "001_records.sql");
    assert_eq!(
        insert_record(
            &connection,
            &InsertRecordParams {
                broker_id: "second",
                ordinal: 2,
                payload: &[2, 2],
                note: None,
            },
        )
        .unwrap(),
        1
    );
    assert_eq!(
        insert_record(
            &connection,
            &InsertRecordParams {
                broker_id: "first",
                ordinal: 1,
                payload: &[1, 1],
                note: Some("before"),
            },
        )
        .unwrap(),
        1
    );
    assert_eq!(
        update_record(
            &connection,
            &UpdateRecordParams {
                note: Some("after"),
                broker_id: "first",
            },
        )
        .unwrap(),
        1
    );
    assert!(get_record(
        &connection,
        &GetRecordParams {
            broker_id: "missing"
        }
    )
    .unwrap()
    .is_none());
    let rows = list_records(&connection).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].payload, vec![1, 1]);
    assert_eq!(rows[0].note.as_deref(), Some("after"));
    assert_eq!(rows[1].note, None);
    assert_eq!(count_records(&connection).unwrap(), Some(2));
    assert_eq!(constant_value(&connection).unwrap(), Some(42));
    assert_eq!(
        find_repeated(
            &connection,
            &FindRepeatedParams {
                ordinal: SessionOrdinal(2)
            }
        )
        .unwrap()
        .unwrap()
        .broker_id,
        "second"
    );
    assert_eq!(
        get_custom_ordinal(&connection, &GetCustomOrdinalParams { broker_id: "first" }).unwrap(),
        Some(SessionOrdinal(1))
    );

    let transaction = connection.transaction().unwrap();
    insert_record(
        &transaction,
        &InsertRecordParams {
            broker_id: "rollback",
            ordinal: 3,
            payload: &[3],
            note: None,
        },
    )
    .unwrap();
    transaction.rollback().unwrap();
    assert!(get_record(
        &connection,
        &GetRecordParams {
            broker_id: "rollback"
        }
    )
    .unwrap()
    .is_none());
}

#[test]
fn cli_generate_and_check_report_drift_and_config_errors() {
    let root = tempdir().unwrap();
    fs::create_dir_all(root.path().join("db/migrations")).unwrap();
    fs::create_dir_all(root.path().join("db/queries")).unwrap();
    fs::write(
        root.path().join("db/migrations/001.sql"),
        "CREATE TABLE item(id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    )
    .unwrap();
    fs::write(
        root.path().join("db/queries/items.sql"),
        "-- name: Items :many\nSELECT id, name FROM item;",
    )
    .unwrap();
    let config_path = root.path().join("d1c.toml");
    fs::write(
        &config_path,
        "version = 1\ntarget = \"rusqlite\"\nmigrations_dir = \"db/migrations\"\nqueries_dir = \"db/queries\"\nout_dir = \"generated\"\nmodule_name = \"queries\"\nemit_schema = false\ninstrument_by_default = false\n",
    )
    .unwrap();

    let binary = env!("CARGO_BIN_EXE_d1c");
    let run = |command: &str| {
        Command::new(binary)
            .arg("--config")
            .arg(&config_path)
            .arg(command)
            .output()
            .unwrap()
    };
    assert!(run("generate").status.success());
    assert!(run("check").status.success());
    let original = fs::read(root.path().join("generated/queries/items.rs")).unwrap();
    fs::write(
        root.path().join("db/queries/items.sql"),
        "-- name: Items :many\nSELECT missing FROM absent;",
    )
    .unwrap();
    assert!(!run("generate").status.success());
    assert_eq!(
        fs::read(root.path().join("generated/queries/items.rs")).unwrap(),
        original
    );
    fs::write(
        root.path().join("db/queries/items.sql"),
        "-- name: Items :many\nSELECT id, name FROM item WHERE id > 0;",
    )
    .unwrap();
    assert!(!run("check").status.success());
    assert!(run("generate").status.success());
    assert!(run("check").status.success());

    fs::write(&config_path, "target = \"rusqlite\"\n").unwrap();
    let missing_version = run("check");
    assert!(!missing_version.status.success());
    assert!(String::from_utf8_lossy(&missing_version.stderr).contains("missing field `version`"));
    fs::write(&config_path, "version = 1\n").unwrap();
    let missing_target = run("check");
    assert!(!missing_target.status.success());
    assert!(String::from_utf8_lossy(&missing_target.stderr).contains("missing field `target`"));
    fs::write(&config_path, "version = 1\ntarget = \"other\"\n").unwrap();
    let unknown_target = run("check");
    assert!(!unknown_target.status.success());
    assert!(String::from_utf8_lossy(&unknown_target.stderr).contains("unknown variant `other`"));
}

#[test]
fn independent_fixture_manifest_has_only_rusqlite_runtime_dependency() {
    let manifest = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rusqlite_consumer/Cargo.toml.fixture"),
    )
    .unwrap();
    assert!(manifest.contains("rusqlite"));
    assert!(!manifest.contains("hb-d1c"));
    assert!(!manifest.contains("worker"));
    assert!(!manifest.contains("tokio"));
}
