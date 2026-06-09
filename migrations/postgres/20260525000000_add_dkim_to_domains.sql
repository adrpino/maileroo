-- Migration: Add DKIM fields to domains table
ALTER TABLE domains 
ADD COLUMN dkim_private_key TEXT,
ADD COLUMN dkim_public_key TEXT,
ADD COLUMN dkim_selector VARCHAR(255) DEFAULT 'maileroo' NOT NULL,
ADD COLUMN pending_dkim_private_key TEXT,
ADD COLUMN pending_dkim_public_key TEXT,
ADD COLUMN pending_dkim_selector VARCHAR(255);
