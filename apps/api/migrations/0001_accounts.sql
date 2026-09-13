-- Use the default public schema in the dedicated captures database, like Caper.
-- Provider-independent account foundation; no sign-in or provisioning is active.
CREATE TABLE users (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email text,
    email_verified boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz,
    deleted_at timestamptz,
    CHECK (deleted_at IS NULL OR email IS NULL)
);
