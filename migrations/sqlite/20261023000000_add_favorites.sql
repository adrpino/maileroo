-- Add favorites column to received_emails and sent_emails

ALTER TABLE received_emails ADD COLUMN is_favorite BOOLEAN NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_received_emails_is_favorite ON received_emails(is_favorite);

ALTER TABLE sent_emails ADD COLUMN is_favorite BOOLEAN NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_sent_emails_is_favorite ON sent_emails(is_favorite);
