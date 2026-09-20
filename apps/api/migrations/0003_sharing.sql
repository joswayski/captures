CREATE TABLE uploads (
    id uuid PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id),
    content_type text NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp')),
    byte_size bigint NOT NULL CHECK (byte_size > 0 AND byte_size <= 20971520),
    state text NOT NULL CHECK (state IN ('pending', 'ready', 'deleting')),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX uploads_owner_idx ON uploads(user_id, created_at DESC);
CREATE INDEX uploads_cleanup_idx ON uploads(state, created_at);

CREATE TABLE shares (
    id uuid PRIMARY KEY,
    upload_id uuid NOT NULL REFERENCES uploads(id) ON DELETE CASCADE,
    visibility text NOT NULL CHECK (visibility IN ('private', 'unlisted', 'public')),
    password_hash text,
    expires_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX shares_upload_idx ON shares(upload_id);

-- Grants never replace the share check: revocation, expiry, and owner suspension
-- are checked again on EVERY metadata/media request, including existing grants.
CREATE TABLE share_viewer_grants (
    token_hash bytea PRIMARY KEY,
    share_id uuid NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL
);
CREATE INDEX share_viewer_grants_expiry_idx ON share_viewer_grants(expires_at);
CREATE TABLE share_unlock_attempts (
    share_id uuid NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    ip_hash bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX share_unlock_attempts_ip_idx ON share_unlock_attempts(ip_hash, created_at);
CREATE INDEX share_unlock_attempts_share_idx ON share_unlock_attempts(share_id, created_at);
