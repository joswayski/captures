-- Retain keys on cancelled/trashed rows: a retry must never create a new asset.
ALTER TABLE assets ADD COLUMN create_request_id uuid;
CREATE UNIQUE INDEX assets_owner_create_request ON assets(user_id, create_request_id);
