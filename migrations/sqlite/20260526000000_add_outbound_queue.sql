-- Create outbound queue table for dynamic retries (SQLite dialect)
CREATE TABLE IF NOT EXISTS outbound_queue (
    id TEXT PRIMARY KEY NOT NULL,
    from_envelope TEXT NOT NULL,
    to_recipient TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 10,
    last_error TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    next_retry_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_outbound_queue_retry ON outbound_queue (status, next_retry_at);
