use anyhow::Result;
use rusqlite::Connection;

pub fn run(conn: &Connection) -> Result<()> {
    println!("Dumping schema...");
    let rows = dump_schema(conn)?;

    for row in rows {
        println!("-- Table: {}", row.name);
        println!("{};\n", row.sql);
    }

    println!("Schema dumped successfully");

    Ok(())
}

#[derive(Debug, Clone)]
pub struct SchemaRow {
    pub name: String,
    pub sql: String,
}

pub fn dump_schema(conn: &Connection) -> Result<Vec<SchemaRow>> {
    let mut stmt =
        conn.prepare("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")?;

    let schema = stmt.query_map([], |row| {
        Ok(SchemaRow {
            name: row.get(0)?,
            sql: row.get(1)?,
        })
    })?;

    let mut schema_rows = Vec::new();
    for row in schema {
        let unwrapped_row = row.unwrap();
        schema_rows.push(unwrapped_row);
    }
    Ok(schema_rows)
}
