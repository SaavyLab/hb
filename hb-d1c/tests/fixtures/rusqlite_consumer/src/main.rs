use rusqlite::types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};

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

#[path = "generated/queries/records.rs"]
mod records;

use records::*;

fn main() -> rusqlite::Result<()> {
    let mut connection = Connection::open_in_memory()?;
    connection.execute_batch(include_str!(
        "../../shared/db/migrations/001_records.sql"
    ))?;

    assert_eq!(
        insert_record(
            &connection,
            &InsertRecordParams {
                broker_id: "broker-b",
                ordinal: 20,
                payload: &[2, 0],
                note: None,
            },
        )?,
        1
    );
    assert_eq!(
        insert_record(
            &connection,
            &InsertRecordParams {
                broker_id: "broker-a",
                ordinal: 10,
                payload: &[1, 0],
                note: Some("initial"),
            },
        )?,
        1
    );
    assert_eq!(
        update_record(
            &connection,
            &UpdateRecordParams {
                note: Some("updated"),
                broker_id: "broker-a",
            },
        )?,
        1
    );

    assert!(get_record(
        &connection,
        &GetRecordParams {
            broker_id: "missing"
        }
    )?
    .is_none());
    let record = get_record(
        &connection,
        &GetRecordParams {
            broker_id: "broker-a",
        },
    )?
    .expect("inserted row");
    assert_eq!(record.payload, vec![1, 0]);
    assert_eq!(record.note.as_deref(), Some("updated"));

    let records = list_records(&connection)?;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].broker_id, "broker-a");
    assert_eq!(records[1].note, None);
    assert_eq!(count_records(&connection)?, Some(2));
    assert_eq!(constant_value(&connection)?, Some(42));
    assert_eq!(
        find_repeated(
            &connection,
            &FindRepeatedParams {
                ordinal: SessionOrdinal(20),
            },
        )?
        .expect("repeated named binding")
        .broker_id,
        "broker-b"
    );
    assert_eq!(
        get_custom_ordinal(
            &connection,
            &GetCustomOrdinalParams {
                broker_id: "broker-a",
            },
        )?,
        Some(SessionOrdinal(10))
    );

    let transaction = connection.transaction()?;
    insert_record(
        &transaction,
        &InsertRecordParams {
            broker_id: "rolled-back",
            ordinal: 30,
            payload: &[3],
            note: None,
        },
    )?;
    transaction.rollback()?;
    assert!(get_record(
        &connection,
        &GetRecordParams {
            broker_id: "rolled-back",
        },
    )?
    .is_none());

    Ok(())
}
