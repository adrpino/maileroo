-- Create outbound queue table for dynamic retries (Postgres dialect)
CREATE TABLE IF NOT EXISTS outbound_queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    from_envelope VARCHAR(255) NOT NULL,
    to_recipient VARCHAR(255) NOT NULL,
    attempts INTEGER DEFAULT 0 NOT NULL,
    max_attempts INTEGER DEFAULT 10 NOT NULL,
    last_error TEXT,
    status VARCHAR(50) DEFAULT 'pending' NOT NULL,
    next_retry_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_outbound_queue_retry ON outbound_queue (status, next_retry_at);
