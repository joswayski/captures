-- Public IDs are separate from internal join keys. Existing object names remain
-- untouched: storage_key records their legacy assets/<old-id> location.
ALTER TABLE users ADD COLUMN external_id varchar(12);
CREATE UNIQUE INDEX users_external_id_key ON users(external_id);

ALTER TABLE assets RENAME COLUMN id TO external_id;
ALTER TABLE assets ADD COLUMN id bigserial;
ALTER TABLE assets ADD COLUMN storage_key text;
ALTER TABLE assets ADD COLUMN deleted_at timestamptz;
UPDATE assets SET storage_key = 'assets/' || external_id;
ALTER TABLE assets ALTER COLUMN storage_key SET NOT NULL;

-- Preserve prior deletion records. Completed objects are now soft deleted;
-- incomplete uploads are canceled and may still need their multipart abort.
ALTER TABLE assets DROP CONSTRAINT assets_state_check;
UPDATE assets SET deleted_at=now(),
    state=CASE WHEN multipart_upload_id IS NULL THEN 'ready' ELSE 'cancelled' END
WHERE state='deleting';
ALTER TABLE assets ADD CONSTRAINT assets_state_check
    CHECK (state IN ('pending', 'ready', 'cancelled'));

ALTER TABLE shares ADD COLUMN asset_internal_id bigint;
UPDATE shares s SET asset_internal_id = a.id FROM assets a
WHERE s.asset_id = a.external_id;
ALTER TABLE shares DROP CONSTRAINT shares_asset_id_fkey;
DROP INDEX shares_one_active_asset;
DROP INDEX shares_asset_idx;
ALTER TABLE assets DROP CONSTRAINT assets_pkey;
ALTER TABLE assets ADD PRIMARY KEY (id);
ALTER TABLE assets ADD CONSTRAINT assets_external_id_key UNIQUE (external_id);
ALTER TABLE shares DROP COLUMN asset_id;
ALTER TABLE shares RENAME COLUMN asset_internal_id TO asset_id;
ALTER TABLE shares ALTER COLUMN asset_id SET NOT NULL;
ALTER TABLE shares ADD CONSTRAINT shares_asset_id_fkey
    FOREIGN KEY (asset_id) REFERENCES assets(id) ON DELETE CASCADE;
CREATE UNIQUE INDEX shares_one_active_asset ON shares(asset_id) WHERE active;
CREATE INDEX shares_asset_idx ON shares(asset_id);

UPDATE shares SET active=false WHERE asset_id IN
    (SELECT id FROM assets WHERE deleted_at IS NOT NULL);
DELETE FROM share_viewer_grants WHERE share_id IN
    (SELECT s.id FROM shares s JOIN assets a ON a.id=s.asset_id WHERE a.deleted_at IS NOT NULL);

ALTER TABLE share_unlock_attempts ADD COLUMN source_ip inet;
ALTER TABLE share_unlock_attempts ADD COLUMN user_agent varchar(512);
ALTER TABLE share_unlock_attempts ADD COLUMN id bigserial;
ALTER TABLE share_unlock_attempts ADD PRIMARY KEY (id);
ALTER TABLE share_unlock_attempts ADD COLUMN outcome text NOT NULL DEFAULT 'unknown'
    CHECK (outcome IN ('unknown', 'pending', 'denied', 'granted'));
ALTER TABLE share_unlock_attempts ALTER COLUMN outcome SET DEFAULT 'pending';
CREATE INDEX share_unlock_attempts_created_idx ON share_unlock_attempts(created_at);
