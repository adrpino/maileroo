-- Add favorites column to received_emails and sent_emails

ALTER TABLE received_emails
    ADD COLUMN IF NOT EXISTS is_favorite BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_received_emails_is_favorite ON received_emails(is_favorite);

ALTER TABLE sent_emails
    ADD COLUMN IF NOT EXISTS is_favorite BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_sent_emails_is_favorite ON sent_emails(is_favorite);
