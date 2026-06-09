-- Migration: Add DKIM fields to domains table
ALTER TABLE domains 
ADD COLUMN dkim_private_key TEXT;

ALTER TABLE domains 
ADD COLUMN dkim_public_key TEXT;

ALTER TABLE domains 
ADD COLUMN dkim_selector VARCHAR(255) NOT NULL DEFAULT 'maileroo';

ALTER TABLE domains 
ADD COLUMN pending_dkim_private_key TEXT;

ALTER TABLE domains 
ADD COLUMN pending_dkim_public_key TEXT;

ALTER TABLE domains 
ADD COLUMN pending_dkim_selector VARCHAR(255);
