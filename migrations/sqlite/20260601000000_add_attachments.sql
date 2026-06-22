-- Add attachments table and has_attachments column to received_emails
CREATE TABLE IF NOT EXISTS attachments (
    id           UUID PRIMARY KEY,
    email_id     UUID NOT NULL REFERENCES received_emails(id) ON DELETE CASCADE,
    filename     TEXT,
    content_type TEXT,
    size_bytes   BIGINT NOT NULL,
    part_index   INTEGER NOT NULL,
    is_inline    BOOLEAN DEFAULT FALSE NOT NULL,
    content_id   TEXT,
    created_at   TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attachments_email_id ON attachments(email_id);

-- SQLite ALTER TABLE ADD COLUMN does not support IF NOT EXISTS; this migration runs once.
ALTER TABLE received_emails ADD COLUMN has_attachments BOOLEAN DEFAULT FALSE NOT NULL;
