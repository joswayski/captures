CREATE TABLE IF NOT EXISTS feedback (
    id BIGSERIAL PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    message TEXT NOT NULL,
    contact TEXT,
    category TEXT NOT NULL DEFAULT 'bug',
    app_version TEXT,
    os TEXT,
    os_version TEXT,
    arch TEXT,
    source TEXT NOT NULL DEFAULT 'desktop'
);

CREATE INDEX IF NOT EXISTS feedback_created_at_idx ON feedback (created_at DESC);
CREATE INDEX IF NOT EXISTS feedback_category_idx ON feedback (category);
