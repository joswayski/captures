export const GIB = 1024 * 1024 * 1024;
export const SINGLE_UPLOAD_MAX_BYTES = 100 * 1024 * 1024;
export const MULTIPART_PART_BYTES = 16 * 1024 * 1024;
export const UPLOAD_TTL_MS = 24 * 60 * 60 * 1_000;
export const PRESIGN_TTL_SECONDS = 15 * 60;
export const DOWNLOAD_TTL_SECONDS = 60 * 60;

export type AssetKind = "screenshot" | "video" | "gif";
export type AssetAccess = "private" | "shared";
export type ClientKind = "web" | "desktop";

export interface SharingConfig {
  enabled: boolean;
  databaseUrl?: string;
  migrationDatabaseUrl?: string;
  publicOrigin: string;
  storage: {
    backend: string;
    endpoint?: string;
    region: string;
    bucket: string;
    accessKeyId?: string;
    secretAccessKey?: string;
  };
  auth: {
    codeHmacKey?: Buffer;
    allowedEmails: Set<string>;
    allowedCidrs: string[];
    publicSignup: boolean;
    googleClientId?: string;
    googleClientSecret?: string;
  };
  mail: {
    host?: string;
    port: number;
    secure: boolean;
    user?: string;
    password?: string;
    from: string;
    configurationSet: string;
    tenant: string;
    snsTopicArn?: string;
  };
}

export interface SessionUser {
  id: string;
  email: string;
  quotaBytes: number;
  usedBytes: number;
  reservedBytes: number;
}

export interface AssetRecord {
  id: string;
  ownerId: string;
  shareId: string;
  title: string | null;
  kind: AssetKind;
  status: "pending" | "ready" | "failed" | "deleting";
  access: AssetAccess;
  storageBackend: string;
  storageBucket: string;
  originalKey: string;
  previewKey: string | null;
  originalMimeType: string;
  previewMimeType: string | null;
  originalBytes: number;
  previewBytes: number;
  reservedBytes: number;
  originalSha256: string;
  previewSha256: string | null;
  width: number | null;
  height: number | null;
  durationMs: number | null;
  multipartUploadId: string | null;
  multipartPartSize: number | null;
  uploadExpiresAt: Date;
  shareExpiresAt: Date | null;
  sharePasswordHash: string | null;
  shareAccessVersion: number;
  createdAt: Date;
  updatedAt: Date;
}

export interface UploadObjectInput {
  bytes: number;
  mimeType: string;
  sha256: string;
}

export interface CreateAssetInput {
  title?: string;
  kind: AssetKind;
  original: UploadObjectInput;
  preview?: UploadObjectInput;
  width?: number;
  height?: number;
  durationMs?: number;
}

export interface MultipartPart {
  partNumber: number;
  etag: string;
}

export interface UploadTarget {
  key: string;
  mimeType: string;
  bytes: number;
  sha256: string;
}

export interface PresignedSingleUpload {
  type: "single";
  url: string;
  headers: Record<string, string>;
}

export interface PresignedMultipartUpload {
  type: "multipart";
  uploadId: string;
  partSize: number;
  parts: Array<{ partNumber: number; url: string }>;
}

export type PresignedUpload = PresignedSingleUpload | PresignedMultipartUpload;

export interface Mailer {
  sendLoginCode(email: string, code: string): Promise<string>;
}

export interface StoredObjectMetadata {
  bytes: number;
  mimeType?: string;
  sha256?: string;
  firstBytes: Uint8Array;
}

export interface ObjectStorage {
  createSingleUpload(target: UploadTarget): Promise<PresignedSingleUpload>;
  createMultipartUpload(
    target: UploadTarget,
  ): Promise<PresignedMultipartUpload>;
  refreshMultipartParts(
    target: UploadTarget,
    uploadId: string,
    partNumbers: number[],
  ): Promise<Array<{ partNumber: number; url: string }>>;
  completeMultipartUpload(
    key: string,
    uploadId: string,
    parts: MultipartPart[],
  ): Promise<void>;
  abortMultipartUpload(key: string, uploadId: string): Promise<void>;
  inspectObject(key: string): Promise<StoredObjectMetadata>;
  createDownloadUrl(key: string, responseContentType: string): Promise<string>;
  deleteObjects(keys: string[]): Promise<void>;
}
