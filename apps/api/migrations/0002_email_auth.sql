-- Normalize before enforcing uniqueness. Fail rather than silently merge duplicate accounts.
UPDATE users SET email = lower(btrim(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_email_unique ON users (email) WHERE email IS NOT NULL;

CREATE TABLE auth_email_challenges (
    id varchar(12) PRIMARY KEY,
    email text NOT NULL,
    code_hash bytea NOT NULL,
    request_ip_hash bytea NOT NULL,
    attempts_remaining smallint NOT NULL CHECK (attempts_remaining >= 0),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX auth_email_challenges_email_created_idx ON auth_email_challenges (email, created_at DESC);
CREATE INDEX auth_email_challenges_ip_created_idx ON auth_email_challenges (request_ip_hash, created_at DESC);
CREATE INDEX auth_email_challenges_created_idx ON auth_email_challenges (created_at);

CREATE TABLE account_sessions (
    token_hash bytea PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX account_sessions_user_idx ON account_sessions (user_id);
CREATE INDEX account_sessions_expiry_idx ON account_sessions (expires_at);
