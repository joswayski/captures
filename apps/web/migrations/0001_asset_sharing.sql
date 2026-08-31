CREATE TABLE IF NOT EXISTS users (
  id varchar(21) PRIMARY KEY,
  email text NOT NULL UNIQUE,
  google_subject text UNIQUE,
  quota_bytes bigint NOT NULL DEFAULT 1073741824 CHECK (quota_bytes > 0),
  used_bytes bigint NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
  reserved_bytes bigint NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
  suspended_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK (email = lower(email)),
  CHECK (used_bytes + reserved_bytes <= quota_bytes)
);

CREATE TABLE IF NOT EXISTS sessions (
  id varchar(21) PRIMARY KEY,
  user_id varchar(21) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind text NOT NULL CHECK (kind IN ('web', 'desktop')),
  access_token_hash bytea NOT NULL UNIQUE,
  refresh_token_hash bytea UNIQUE,
  access_expires_at timestamptz NOT NULL,
  refresh_expires_at timestamptz,
  last_used_at timestamptz NOT NULL DEFAULT now(),
  revoked_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (
    (kind = 'web' AND refresh_token_hash IS NULL AND refresh_expires_at IS NULL)
    OR
    (kind = 'desktop' AND refresh_token_hash IS NOT NULL AND refresh_expires_at IS NOT NULL)
  )
);

CREATE INDEX IF NOT EXISTS sessions_user_active_idx
  ON sessions (user_id, access_expires_at)
  WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS login_codes (
  id varchar(21) PRIMARY KEY,
  email text NOT NULL,
  client_kind text NOT NULL CHECK (client_kind IN ('web', 'desktop')),
  code_hmac bytea NOT NULL,
  request_ip_hmac bytea NOT NULL,
  attempts smallint NOT NULL DEFAULT 0 CHECK (attempts >= 0 AND attempts <= 6),
  ses_message_id text,
  expires_at timestamptz NOT NULL,
  consumed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (email = lower(email))
);

CREATE INDEX IF NOT EXISTS login_codes_email_created_idx ON login_codes (email, created_at DESC);
CREATE INDEX IF NOT EXISTS login_codes_ip_created_idx ON login_codes (request_ip_hmac, created_at DESC);

CREATE TABLE IF NOT EXISTS assets (
  id varchar(21) PRIMARY KEY,
  owner_id varchar(21) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  share_id varchar(21) NOT NULL UNIQUE,
  title text,
  kind text NOT NULL CHECK (kind IN ('screenshot', 'video', 'gif')),
  status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'ready', 'failed', 'deleting')),
  access text NOT NULL DEFAULT 'private' CHECK (access IN ('private', 'shared')),
  storage_backend text NOT NULL,
  storage_bucket text NOT NULL,
  original_key text NOT NULL UNIQUE,
  preview_key text,
  original_mime_type text NOT NULL,
  preview_mime_type text,
  original_bytes bigint NOT NULL CHECK (original_bytes > 0 AND original_bytes <= 1073741824),
  preview_bytes bigint NOT NULL DEFAULT 0 CHECK (preview_bytes >= 0),
  reserved_bytes bigint NOT NULL CHECK (reserved_bytes > 0),
  original_sha256 text NOT NULL,
  preview_sha256 text,
  width integer CHECK (width IS NULL OR width > 0),
  height integer CHECK (height IS NULL OR height > 0),
  duration_ms bigint CHECK (duration_ms IS NULL OR duration_ms >= 0),
  multipart_upload_id text,
  multipart_part_size integer,
  upload_expires_at timestamptz NOT NULL,
  share_expires_at timestamptz,
  share_password_hash text,
  share_access_version integer NOT NULL DEFAULT 1 CHECK (share_access_version > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  deleted_at timestamptz,
  CHECK (reserved_bytes = original_bytes + preview_bytes),
  CHECK ((preview_key IS NULL) = (preview_bytes = 0)),
  CHECK ((preview_sha256 IS NULL) = (preview_bytes = 0)),
  CHECK ((preview_mime_type IS NULL) = (preview_bytes = 0)),
  CHECK ((multipart_upload_id IS NULL) = (multipart_part_size IS NULL))
);

CREATE INDEX IF NOT EXISTS assets_owner_created_idx
  ON assets (owner_id, created_at DESC, id DESC)
  WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS assets_pending_expiry_idx
  ON assets (upload_expires_at)
  WHERE status = 'pending' AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS email_suppressions (
  email text PRIMARY KEY,
  reason text NOT NULL CHECK (reason IN ('hard_bounce', 'complaint')),
  ses_event_id text NOT NULL UNIQUE,
  ses_message_id text,
  occurred_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK (email = lower(email))
);
