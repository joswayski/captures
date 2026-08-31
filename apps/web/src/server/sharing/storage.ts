import {
  AbortMultipartUploadCommand,
  CompleteMultipartUploadCommand,
  CreateMultipartUploadCommand,
  DeleteObjectsCommand,
  GetObjectCommand,
  HeadObjectCommand,
  PutObjectCommand,
  S3Client,
  UploadPartCommand,
} from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";
import {
  DOWNLOAD_TTL_SECONDS,
  MULTIPART_PART_BYTES,
  PRESIGN_TTL_SECONDS,
  type MultipartPart,
  type ObjectStorage,
  type PresignedMultipartUpload,
  type PresignedSingleUpload,
  type StoredObjectMetadata,
  type UploadTarget,
} from "./types.ts";

export class S3CompatibleStorage implements ObjectStorage {
  readonly #bucket: string;
  readonly #client: S3Client;

  constructor(options: {
    endpoint: string;
    region: string;
    bucket: string;
    accessKeyId: string;
    secretAccessKey: string;
  }) {
    this.#bucket = options.bucket;
    this.#client = new S3Client({
      endpoint: options.endpoint,
      region: options.region,
      forcePathStyle: true,
      credentials: {
        accessKeyId: options.accessKeyId,
        secretAccessKey: options.secretAccessKey,
      },
    });
  }

  async createSingleUpload(target: UploadTarget): Promise<PresignedSingleUpload> {
    const headers = uploadHeaders(target);
    const command = new PutObjectCommand({
      Bucket: this.#bucket,
      Key: target.key,
      ContentLength: target.bytes,
      ContentType: target.mimeType,
      IfNoneMatch: "*",
      Metadata: { sha256: target.sha256 },
    });
    return {
      type: "single",
      url: await getSignedUrl(this.#client, command, { expiresIn: PRESIGN_TTL_SECONDS }),
      headers,
    };
  }

  async createMultipartUpload(
    target: UploadTarget,
  ): Promise<PresignedMultipartUpload> {
    const created = await this.#client.send(
      new CreateMultipartUploadCommand({
        Bucket: this.#bucket,
        Key: target.key,
        ContentType: target.mimeType,
        Metadata: { sha256: target.sha256 },
      }),
    );
    if (!created.UploadId) throw new Error("object storage did not return an upload ID");
    const partCount = Math.ceil(target.bytes / MULTIPART_PART_BYTES);
    return {
      type: "multipart",
      uploadId: created.UploadId,
      partSize: MULTIPART_PART_BYTES,
      parts: await this.refreshMultipartParts(
        target,
        created.UploadId,
        Array.from({ length: partCount }, (_, index) => index + 1),
      ),
    };
  }

  async refreshMultipartParts(
    target: UploadTarget,
    uploadId: string,
    partNumbers: number[],
  ): Promise<Array<{ partNumber: number; url: string }>> {
    const partCount = Math.ceil(target.bytes / MULTIPART_PART_BYTES);
    if (
      partNumbers.length === 0 ||
      partNumbers.some(
        (partNumber) =>
          !Number.isInteger(partNumber) || partNumber < 1 || partNumber > partCount,
      )
    ) {
      throw new Error("invalid multipart part number");
    }
    return Promise.all(
      partNumbers.map(async (partNumber) => ({
        partNumber,
        url: await getSignedUrl(
          this.#client,
          new UploadPartCommand({
            Bucket: this.#bucket,
            Key: target.key,
            UploadId: uploadId,
            PartNumber: partNumber,
            ContentLength: partLength(target.bytes, partNumber, MULTIPART_PART_BYTES),
          }),
          { expiresIn: PRESIGN_TTL_SECONDS },
        ),
      })),
    );
  }

  async completeMultipartUpload(
    key: string,
    uploadId: string,
    parts: MultipartPart[],
  ): Promise<void> {
    await this.#client.send(
      new CompleteMultipartUploadCommand({
        Bucket: this.#bucket,
        Key: key,
        UploadId: uploadId,
        MultipartUpload: {
          Parts: parts.map(({ partNumber, etag }) => ({
            PartNumber: partNumber,
            ETag: etag,
          })),
        },
      }),
    );
  }

  async abortMultipartUpload(key: string, uploadId: string): Promise<void> {
    await this.#client.send(
      new AbortMultipartUploadCommand({
        Bucket: this.#bucket,
        Key: key,
        UploadId: uploadId,
      }),
    );
  }

  async inspectObject(key: string): Promise<StoredObjectMetadata> {
    const [head, prefix] = await Promise.all([
      this.#client.send(new HeadObjectCommand({ Bucket: this.#bucket, Key: key })),
      this.#client.send(
        new GetObjectCommand({ Bucket: this.#bucket, Key: key, Range: "bytes=0-4095" }),
      ),
    ]);
    if (!prefix.Body) throw new Error("object storage returned an empty object body");
    return {
      bytes: Number(head.ContentLength ?? -1),
      mimeType: head.ContentType,
      sha256: head.Metadata?.sha256,
      firstBytes: await prefix.Body.transformToByteArray(),
    };
  }

  async createDownloadUrl(key: string, responseContentType: string): Promise<string> {
    return getSignedUrl(
      this.#client,
      new GetObjectCommand({
        Bucket: this.#bucket,
        Key: key,
        ResponseContentType: responseContentType,
        ResponseContentDisposition: "inline",
      }),
      { expiresIn: DOWNLOAD_TTL_SECONDS },
    );
  }

  async deleteObjects(keys: string[]): Promise<void> {
    if (keys.length === 0) return;
    const response = await this.#client.send(
      new DeleteObjectsCommand({
        Bucket: this.#bucket,
        Delete: { Quiet: true, Objects: keys.map((Key) => ({ Key })) },
      }),
    );
    if (response.Errors?.length) {
      throw new Error(`failed to delete ${response.Errors.length} stored object(s)`);
    }
  }
}

function uploadHeaders(target: UploadTarget): Record<string, string> {
  return {
    "content-type": target.mimeType,
    "if-none-match": "*",
    "x-amz-meta-sha256": target.sha256,
  };
}

function partLength(total: number, partNumber: number, partSize: number): number {
  return Math.min(partSize, total - (partNumber - 1) * partSize);
}
