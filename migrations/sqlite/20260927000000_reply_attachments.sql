-- Add reply ownership and stored-message linkage for reply attachments.

ALTER TABLE attachments ADD COLUMN reply_id UUID REFERENCES email_replies(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_attachments_reply_id ON attachments(reply_id);

ALTER TABLE email_replies ADD COLUMN body_key UUID;
