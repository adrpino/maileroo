-- Add attachments table and has_attachments column to received_emails
CREATE TABLE IF NOT EXISTS attachments (
    id           UUID PRIMARY KEY,
    email_id     UUID NOT NULL REFERENCES received_emails(id) ON DELETE CASCADE,
    filename     TEXT,
    content_type TEXT,
    size_bytes   BIGINT NOT NULL,          -- BIGINT, not INTEGER (32-bit caps ~2.1GB); decoded size
    part_index   INTEGER NOT NULL,         -- index into mail_parser's attachments() iterator
    is_inline    BOOLEAN DEFAULT FALSE NOT NULL,  -- cid: embedded image, not a real attachment
    content_id   TEXT,                     -- the Content-ID (without <>), for cid: rewriting
    created_at   TIMESTAMPTZ DEFAULT NOW() NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attachments_email_id ON attachments(email_id);

ALTER TABLE received_emails ADD COLUMN IF NOT EXISTS has_attachments BOOLEAN DEFAULT FALSE NOT NULL;
