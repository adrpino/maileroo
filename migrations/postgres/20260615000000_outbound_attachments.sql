-- Drop the received-only FK so an attachment row can belong to either a received
-- or a sent email. Referential cleanup is handled explicitly on delete (see §7).
ALTER TABLE attachments DROP CONSTRAINT IF EXISTS attachments_email_id_fkey;

ALTER TABLE sent_emails ADD COLUMN IF NOT EXISTS has_attachments BOOLEAN DEFAULT FALSE NOT NULL;
