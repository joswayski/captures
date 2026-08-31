# Asset sharing architecture

Captures sharing is an optional capability of the existing website process. It
adds authentication, a private account library, and random shared links without
changing the local-first capture workflow.

## Product boundary

- A capture is local until its owner chooses **Share**.
- Every remote asset belongs to an account.
- Remote assets have only `private` and `shared` access states.
- A private asset is visible only in its owner's library.
- A shared asset is visible to anyone who has its random NanoID link, until the
  owner makes it private, rotates the link, deletes it, or its optional expiry
  passes.
- There is no global gallery or public enumeration API.
- Deleting a remote asset never deletes its local history entry or file.
- Password-protected links are a future feature. The schema already has the
  password hash and access-version fields needed to add it safely.

All public identifiers are 21-character NanoIDs. The asset ID is stable and is
used only by authenticated owner routes. `share_id` is independent so a leaked
link can be invalidated without replacing the asset.

## Why five tables

The shared PostgreSQL cluster hosts a separate `captures` logical database. The
application and startup migrations use one Captures credential; PostgreSQL is
not split into one role per table or one migrator per application.

| Table | What it owns | Why it is separate |
| --- | --- | --- |
| `users` | Email identity, optional Google subject, quota, suspension | One durable account and one atomic quota counter per person |
| `sessions` | Hashed web/access/refresh tokens and expiry/revocation | Sign-out and device revocation must not alter the user row |
| `login_codes` | Keyed hashes of short-lived OTPs, attempts, IP hash, SES message ID | Login throttling and one-time consumption have a different lifecycle from sessions |
| `assets` | Ownership, object keys, upload state, media metadata, access, link expiry/password fields | One row represents one user-visible capture; there is no separate asset-object table because an original plus optional preview is enough today |
| `email_suppressions` | Hard-bounce and complaint addresses mirrored from SES events | Stops Captures from repeatedly sending to an address even if SES suppression configuration changes |

Startup migrations are repeatable DDL under a PostgreSQL advisory lock. There
is intentionally no sixth migration-ledger table; future changes must use
idempotent `IF NOT EXISTS` or catalog-checked migration steps.

An `asset_objects` table would only earn its keep if Captures supported an
unbounded set of renditions per asset. Today the original and optional preview
have fixed roles, so their keys, sizes, MIME types, and checksums live directly
on `assets`. This keeps quota reservation and completion in one transaction.

The database stores the storage backend, bucket, and object key—not a provider
URL. The current process uses one configured S3-compatible backend, while those
fields leave enough information for a future router or migration to keep older
objects on R2 or move them to a provider such as Tigris without creating a
permanent public-URL dependency.

## Upload lifecycle

1. The authenticated client sends media metadata and SHA-256 values to
   `POST /api/assets`.
2. PostgreSQL atomically reserves the original plus preview bytes against the
   user's 1 GiB quota and creates a `pending` asset.
3. The API returns 15-minute presigned uploads. Files up to 100 MiB use one PUT;
   larger files use 16 MiB multipart parts. Single PUTs require
   `If-None-Match: *`, so an unexpired URL cannot overwrite an object after
   completion.
4. The client uploads bytes directly to the private object store. The API never
   buffers the capture.
5. The client submits completed part ETags to `POST /api/assets/:id/complete`.
6. The API completes multipart uploads when needed, then checks stored size,
   type, checksum metadata, and media magic bytes before changing the asset to
   `ready` and moving reserved bytes to used bytes.
7. The owner explicitly changes access to `shared`; completion alone never
   publishes an asset.

Pending uploads last 24 hours. A client can request fresh URLs for specific
multipart parts, so a long-running upload can continue after an individual URL
expires. The desktop client does not persist completed part ETags across an app
restart yet. An hourly cleanup runs inside every app replica but uses a
PostgreSQL advisory lock, so only one replica aborts expired multipart uploads,
deletes stale objects, and releases reservations.

## Authentication and email

Web sessions use an HttpOnly, Secure, SameSite=Lax cookie. Desktop access tokens
last 15 minutes; 30-day refresh tokens are stored in the operating system
credential store and never in Captures settings or history files. All persisted
session tokens are SHA-256 hashes.

Email login codes are stored as HMACs using `AUTH_CODE_HMAC_KEY`; this is a
long-lived application secret, not a password pepper and not a scheduled-rotation
credential. Private beta login requires both an allowed email and an allowed
client IP/CIDR. Public signup later adds Turnstile while retaining database-backed
email/IP rate limits.

Captures sends through its own SES tenant, verified identity, configuration set,
and SMTP IAM user. Other projects do not share its SMTP credentials or sending
identity, and SES can enforce reputation at the tenant boundary. The AWS account
is still the outer Trust & Safety boundary; hard isolation from account-wide
enforcement would require a separate AWS account. SES publishes bounces and
complaints to a Captures SNS topic. The
API verifies SNS signatures, confirms only that configured topic, and mirrors
hard bounces and complaints into `email_suppressions`; an SQS dead-letter queue
retains events SNS cannot deliver.

## Migration and rollout

The process runs checked-in, repeatable SQL migrations before it
starts accepting sharing requests. It connects through `DATABASE_MIGRATION_URL`
and serializes migration runners with a PostgreSQL advisory lock. Runtime
queries use the pooled `DATABASE_URL`. This deliberately keeps the small current
deployment simple while allowing migrations to move to a release job later if
startup contention or privileges become a real operational problem.

`SHARING_ENABLED` is the rollout switch. When false, the sharing module does not
connect to PostgreSQL, object storage, or SES and existing website behavior is
unchanged.
