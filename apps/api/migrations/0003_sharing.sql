CREATE TABLE assets (
    id varchar(12) PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id),
    name text NOT NULL,
    content_type text NOT NULL,
    byte_size bigint NOT NULL CHECK (byte_size >= 0),
    state text NOT NULL CHECK (state IN ('pending', 'ready', 'deleting')),
    multipart_upload_id text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX assets_owner_idx ON assets(user_id, created_at DESC);
CREATE INDEX assets_cleanup_idx ON assets(state, created_at);

CREATE TABLE shares (
    id varchar(12) PRIMARY KEY,
    asset_id varchar(12) NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    password_hash text,
    expires_at timestamptz,
    shared_at timestamptz NOT NULL DEFAULT now(),
    active boolean NOT NULL DEFAULT true
);
CREATE UNIQUE INDEX shares_one_active_asset ON shares(asset_id) WHERE active;
CREATE INDEX shares_asset_idx ON shares(asset_id);

-- Grants never replace the share check: revocation, expiry, and owner suspension
-- are checked again on EVERY metadata/media request, including existing grants.
CREATE TABLE share_viewer_grants (
    token_hash bytea PRIMARY KEY,
    share_id varchar(12) NOT NULL REFERENCES shares(id) ON DELETE CASCADE
);
CREATE TABLE share_unlock_attempts (
    share_id varchar(12) NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    ip_hash bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX share_unlock_attempts_ip_idx ON share_unlock_attempts(ip_hash, created_at);
CREATE INDEX share_unlock_attempts_share_idx ON share_unlock_attempts(share_id, created_at);
