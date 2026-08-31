import initialAssetSharingMigration from "../../../migrations/0001_asset_sharing.sql?raw";

export const SHARING_MIGRATIONS = [
  { name: "asset_sharing", sql: initialAssetSharingMigration },
] as const;
