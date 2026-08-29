-- name: InsertRecord :exec
INSERT INTO records (broker_id, ordinal, payload, note)
VALUES (:broker_id, :ordinal, :payload, :note);

-- name: UpdateRecord :exec
UPDATE records
SET note = :note
WHERE broker_id = :broker_id;

-- name: GetRecord :one
SELECT id, broker_id, ordinal, payload, note
FROM records
WHERE broker_id = :broker_id;

-- name: ListRecords :many
SELECT id, broker_id, ordinal, payload, note
FROM records
ORDER BY ordinal;

-- name: CountRecords :scalar
-- columns: count i64
SELECT count(*) AS count
FROM records;

-- name: FindRepeated :one
-- params: ordinal crate::SessionOrdinal
SELECT id, broker_id, ordinal, payload, note
FROM records
WHERE ordinal = :ordinal OR ordinal = :ordinal;

-- name: GetCustomOrdinal :scalar
-- params: broker_id String
-- columns: ordinal crate::SessionOrdinal
SELECT ordinal
FROM records
WHERE broker_id = :broker_id;

-- name: ConstantValue :scalar
-- columns: value i64
SELECT 42 AS value;
