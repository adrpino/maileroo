-- SQLite cannot DROP a column-level FK via ALTER TABLE. Recreate the table without the FK,
-- copy rows, swap. (Standard SQLite table-rebuild; keep the index.)
PRAGMA foreign_keys=OFF;
CREATE TABLE attachments_new (
    id                  UUID PRIMARY KEY,
    email_id            UUID NOT NULL,
    filename            TEXT,
    content_type        TEXT,
    size_bytes          BIGINT NOT NULL,
    part_index          INTEGER NOT NULL,
    is_inline           BOOLEAN DEFAULT FALSE NOT NULL,
    content_id          TEXT,
    created_at          TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL
);
-- List columns explicitly so future column additions on `attachments` don't silently
-- reorder the SELECT/INSERT and corrupt the copy.
INSERT INTO attachments_new (id, email_id, filename, content_type, size_bytes, part_index,
                             is_inline, content_id, created_at)
SELECT id, email_id, filename, content_type, size_bytes, part_index,
       is_inline, content_id, created_at
FROM attachments;
DROP TABLE attachments;
ALTER TABLE attachments_new RENAME TO attachments;
CREATE INDEX IF NOT EXISTS idx_attachments_email_id ON attachments(email_id);
PRAGMA foreign_keys=ON;

ALTER TABLE sent_emails ADD COLUMN has_attachments BOOLEAN DEFAULT FALSE NOT NULL;
