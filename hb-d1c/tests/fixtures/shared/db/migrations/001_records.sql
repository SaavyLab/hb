CREATE TABLE records (
    id INTEGER PRIMARY KEY,
    broker_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    payload BLOB NOT NULL,
    note TEXT
);
