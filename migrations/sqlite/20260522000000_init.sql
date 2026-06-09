-- SQLite Initial Schema Setup

-- 1. Create tables
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    email TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    is_admin BOOLEAN DEFAULT FALSE NOT NULL,
    bypass_alias_limit BOOLEAN DEFAULT FALSE NOT NULL,
    disable_autoclean BOOLEAN DEFAULT FALSE NOT NULL,
    can_send_firsthand BOOLEAN DEFAULT FALSE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_login_at TIMESTAMPTZ,
    last_login_ip TEXT
);

CREATE TABLE IF NOT EXISTS domains (
    id UUID PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    active BOOLEAN DEFAULT TRUE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS aliases (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    domain_id UUID REFERENCES domains(id) ON DELETE CASCADE NOT NULL,
    subdomain TEXT NOT NULL, 
    destination_email TEXT NOT NULL,
    auto_forward BOOLEAN DEFAULT TRUE NOT NULL,
    active BOOLEAN DEFAULT TRUE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    UNIQUE (subdomain, domain_id)
);

CREATE TABLE IF NOT EXISTS received_emails (
    id UUID PRIMARY KEY,
    alias_id UUID REFERENCES aliases(id) ON DELETE CASCADE,
    sender_email TEXT NOT NULL,
    subject TEXT,
    body_key UUID NOT NULL,
    status TEXT DEFAULT 'pending',
    viewed BOOLEAN DEFAULT FALSE NOT NULL,
    forwarded BOOLEAN DEFAULT FALSE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    received_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    last_activity_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    message_id TEXT,
    thread_id UUID REFERENCES received_emails(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS email_replies (
    id UUID PRIMARY KEY,
    email_id UUID REFERENCES received_emails(id) ON DELETE CASCADE NOT NULL,
    body_text TEXT NOT NULL,
    sent_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    message_id TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    data BYTEA NOT NULL,
    expiry_date TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS sent_emails (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    from_alias_id UUID REFERENCES aliases(id) ON DELETE CASCADE NOT NULL,
    to_address TEXT NOT NULL,
    cc_addresses TEXT, 
    bcc_addresses TEXT,
    subject TEXT NOT NULL,
    body_key UUID NOT NULL,
    status TEXT DEFAULT 'draft' NOT NULL,
    error_message TEXT,
    message_id TEXT UNIQUE,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL,
    sent_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    key_hash TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS reply_mappings (
    id UUID PRIMARY KEY,
    alias_id UUID REFERENCES aliases(id) ON DELETE CASCADE NOT NULL,
    original_sender TEXT NOT NULL,
    anonymous_token TEXT UNIQUE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- 2. Create indices
CREATE INDEX IF NOT EXISTS idx_sent_emails_user_id ON sent_emails(user_id);
CREATE INDEX IF NOT EXISTS idx_sent_emails_status ON sent_emails(status);
CREATE INDEX IF NOT EXISTS idx_received_emails_alias_id ON received_emails(alias_id);
CREATE INDEX IF NOT EXISTS idx_received_emails_received_at ON received_emails(received_at DESC);
CREATE INDEX IF NOT EXISTS idx_aliases_user_id ON aliases(user_id);
CREATE INDEX IF NOT EXISTS idx_received_emails_message_id ON received_emails(message_id);
CREATE INDEX IF NOT EXISTS idx_received_emails_thread_id ON received_emails(thread_id);
CREATE INDEX IF NOT EXISTS idx_email_replies_message_id ON email_replies(message_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_reply_mappings_alias_sender ON reply_mappings(alias_id, original_sender);
