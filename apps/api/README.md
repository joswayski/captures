# Captures accounts and asset sharing API

Rust/Axum owns PostgreSQL accounts, SES email OTP, opaque sessions, account-owned
assets, and access-controlled sharing from private R2 storage. Accounts and
sharing default to disabled. Native account UI, billing, public profiles, and
organizations are not implemented.

## Endpoints

| Route | Contract |
| --- | --- |
| `GET /health` | Public liveness; not a database readiness probe |
| `POST /api/auth/email/request` | `{email}` → 202 `{challengeId}`; throttled requests also return synthetic IDs without sending mail |
| `POST /api/auth/email/verify` | `{challengeId,code,transport:"cookie"\|"bearer"}` → `{user:{id,email},token?}` |
| `GET /api/account/me` | `{user:{id,email}}`; 401 signed out, 503 when disabled |
| `POST /api/auth/logout` | Revoke the current session, clear cookie; 204 |
| `GET /api/assets` | `{assets:[{id,name,contentType,byteSize,createdAt,share}]}`; completed owner assets |
| `POST /api/assets` | `{name,contentType,byteSize}` → 201 `{id,partSize,partCount}` |
| `POST /api/assets/<id>/parts` | `{partNumber}` → direct R2 `{url,headers}` |
| `POST /api/assets/<id>/complete` | `{parts:[{partNumber,etag}]}` → completed asset |
| `GET /api/media/assets/<id>` | Worker-only owner authorization → `{key,name,contentType,byteSize}`; no file bytes |
| `DELETE /api/assets/<id>` | Abort pending or deny/delete completed asset with durable retry; 204 |
| `PUT /api/assets/<id>/share` | `{enabled,password?,expiresAt?}` → `{share}`; owner-only |
| `GET /api/shares/<id>` | Metadata; protected links return `passwordRequired:true,mediaUrl:null` until authorized |
| `GET /api/media/shares/<id>` | Worker-only share authorization → `{key,name,contentType,byteSize}`; no file bytes |
| `POST /api/shares/<id>/unlock` | `{password}` → 204 and browser-session per-share viewer cookie |

Account and media responses are `no-store`. Browser writes require the exact
`AUTH_ALLOWED_ORIGIN`; native JSON/bearer clients omit browser Origin/cookies.
User IDs serialize as decimal strings. Errors are JSON `{error,attemptsRemaining?}`.
Asset, share, and auth-challenge IDs are 12-character NanoIDs. Each ID is reserved
in PostgreSQL with bounded collision retries; logs record only the ID kind, never
the colliding value. Codes use six A–Z/0–9 characters, expire in ten minutes,
allow three guesses, and
are HMAC-bound to their challenge/email. Requests allow 3/email/15m, 5/email/day,
10/IP/hour, 500 globally/hour, serialized in PostgreSQL across replicas. New
challenges supersede older ones. Code-delivery failures consume the challenge.
Sessions use hashed random 32-byte tokens with 30-day expiry and logout revocation.
Raw tokens, codes, IPs, emails and provider credentials must not be logged.

## Runtime configuration

Set variables in the process environment; the Rust binary does not load `.env`
files itself. Never put secrets in the image or desktop bundle.

| Variable | Required/default |
| --- | --- |
| `DATABASE_URL` | Captures runtime role, pooled PlanetScale port **6432**, database **`captures`** |
| `MIGRATION_DATABASE_URL` | Captures migration role, direct port **5432**, same host and database; required at startup |
| `CAPTURES_API_BIND` | `127.0.0.1:3001`; image uses `0.0.0.0:3001` |
| `RUST_LOG` | Optional filter; do not enable request/body or SQL parameter logging in production |
| `AUTH_ENABLED` | `false`; explicit `true` requires all auth settings below |
| `AUTH_SECRET` | Stable, app/environment-specific random secret of at least 32 bytes; rotating invalidates challenges, not sessions |
| `AUTH_ALLOWED_ORIGIN` | Exact browser origin, e.g. `https://captur.es`, no trailing slash |
| `AWS_REGION` | SES region; AWS default credential/workload-identity chain |
| `SES_FROM_ADDRESS`, `SES_CONFIGURATION_SET` | Verified SES sender and transactional configuration set |
| `AUTH_TRUST_CF_CONNECTING_IP` | `false`; enable only behind ingress restricted to trusted Cloudflare proxy paths; otherwise socket peer IP is used |
| `AUTH_INSECURE_LOOPBACK_COOKIE` | `false`; local HTTP development only, requires loopback API bind and permits loopback HTTP origin |
| `SHARING_ENABLED` | `false`; `true` also requires auth and all R2 settings |
| `R2_ACCOUNT_ID` | Cloudflare account ID; derives the HTTPS R2 endpoint, region `auto` |
| `R2_BUCKET` | `staging-captures` or `production-captures`; private, no r2.dev/custom public domain |
| `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` | Environment/bucket-scoped Object Read & Write credentials, separate from SES AWS identity |
| `MEDIA_WORKER_SECRET` | Required when sharing enabled; independent random secret of at least 32 bytes shared only with the environment's media Worker |

Enabled-but-incomplete configuration fails startup. Health is liveness, not an SES
delivery test: activation still requires a real delivery/upload/view/revoke smoke
test. Keep the deployment's explicit auth/sharing gates off until that rollout.
Use the existing environment application secret JSON; preserve database/webhook
keys. The app reads process environment (it does not fetch Secrets Manager itself).
The Node website receives neither database URLs nor account/storage secrets.

## Storage and viewer policy

The API preserves original bytes and imposes no product account, size, dimension,
or link-count quota. It creates direct presigned multipart uploads at
`assets/<id>`. Each part URL is a 15-minute upload authorization, separate from
viewer access; an owner can request a replacement part URL. The service chooses
`max(64 MiB, ceil(byteSize/10000))` chunks, increasing them dynamically as needed.
R2's inherent provider limits are 5 TiB per object, 10,000 parts, and 5 MiB–5 GiB
per part; they are protocol bounds, not Captures product quotas. Pending objects are
never readable and ready objects can never be overwritten.

Browser uploads require bucket CORS permitting `PUT` from the exact account-site
origin, the signed request headers (including `Content-Type` when used), and an
exposed `ETag` response header. Keep buckets private; CORS does not grant object
access. This supersedes the earlier no-CORS storage foundation and requires
separately reviewed infrastructure configuration before activation.

Original GIF, video, raster, and general-asset bytes are retained. Downloads are
streamed by the [Cloudflare media Worker](../media-worker/README.md) from a private
R2 binding with single-range support. The API never receives download bytes and no
signed GET is exposed. Each GET/HEAD/range request calls the API with
`x-captures-media-key: MEDIA_WORKER_SECRET` and the viewer's credentials. The API
returns only an authorized object key and metadata; missing/invalid Worker keys
are rejected even with a valid viewer session. GIF, JPEG, PNG, WebP, MP4, WebM, and
Ogg video are safe inline media; other types are attachment `application/octet-stream`.
Neither authorization nor responses are cached. Revocation denies requests whose
authorization begins after the revocation commits; already authorized streams and
saved bytes cannot be recalled. API unavailability denies new downloads.
Every share is an anyone-with-link URL, optionally protected by Argon2id password,
and always requests no indexing. There is at most one active, editable share per
asset. In the share update body, omitted `password`/`expiresAt` preserves the current
value, `null` removes it, and a string replaces it. Viewer grants contain only a
hash of a 32-byte secret and use a browser-session cookie with no 15-minute expiry;
every request rechecks the active share, expiry, asset state, and owner state.
Changing/removing a password or stopping sharing invalidates grants. Re-enabling a
disabled share creates a new 12-character URL, permanently invalidating the old one.

Deletion first records `deleting`, denying reads even if R2 fails. Every five
minutes cleanup retries failed deletes and removes pending uploads older than seven
days, matching the bucket's incomplete-multipart lifecycle policy.
Completed ready media has no automatic expiry. Expiring/revoking a share does not
delete the owner's upload. Auth challenges/sessions receive seven-day expired
retention cleanup on issuance. Share edits/deletion remove viewer grants;
the sharing cleanup task removes old unlock attempts. Do not cache `/api/*`, `/account`,
`/s/*`, or `/media/*` in a proxy/CDN.

## Shared cluster, dedicated database, separate credentials

Use the named PostgreSQL database **`captures`** in the existing PlanetScale
`projects/main` cluster, alongside Caper's `caperchat` database. This is not a
second paid cluster. Create the database and roles before startup; changing a URL
path does not create a database. Database separation still shares cluster compute,
storage capacity, and failure modes.

Application tables and SQLx's `_sqlx_migrations` ledger use the default `public`
schema, matching Caper. The databases remain separate: Captures connects to
`captures`, Caper to `caperchat`. A schema is a namespace inside a database;
`public` does not mean publicly accessible.

The direct migration connection explicitly pins `search_path=public`, so role or
database settings and username schemas cannot redirect table or ledger creation.
Runtime pooler connections receive no startup override. For runtime and manual
queries, keep `public` on the search path without another `users` table ahead of
it. PostgreSQL's usual `"$user", public` default works unless the username schema
shadows that table. With normal lookup, use `SELECT * FROM users` and
`SELECT * FROM _sqlx_migrations`; no prefix or per-session `SET` is required.
If lookup was customized, check `SHOW search_path` and correct the role/database
defaults. Pinning migrations does not change those defaults or move existing tables.

Users retain an internal bigint ID, nullable email, verification state (false by
default), creation/update timestamps, and disabled/deleted timestamps. Deleted
rows must have no email. Emails are trimmed/lowercased with a uniqueness index;
the migration fails on conflicting existing emails instead of merging identities.
No public user identifier or mandatory username is added.

```dotenv
DATABASE_URL=postgresql://captures_app:PASSWORD@HOST:6432/captures?sslmode=verify-full
MIGRATION_DATABASE_URL=postgresql://captures_migrator:PASSWORD@HOST:5432/captures?sslmode=verify-full
```

Remote URLs require `sslmode=verify-full`, an explicit non-default database name,
and matching hosts/database paths. Migration URLs reject pooled port 6432 and
require direct port 5432 remotely; loopback tests may use other ports and plaintext.
Only `sslmode`, `sslrootcert`, and `application_name` URL parameters are accepted.
With SQLx 0.8's `runtime-tokio-rustls`, omit `sslrootcert` for bundled public roots,
or use a real CA PEM file mounted in the API container. Do not use
`sslrootcert=system`: SQLx treats it as a filename.

Use separate migration-owner and runtime roles; neither should be cluster admin.
The migration role needs `USAGE, CREATE` on `public` and owns the tables it creates.
The runtime role needs only schema usage and the table/sequence grants below, not
schema-changing privileges. At every API startup:

1. Validate both URLs; never fall back to runtime credentials for DDL.
2. Run migrations through one direct connection before listening. SQLx's advisory
   lock serializes concurrent migrations. Connection and migration stages each
   have a five-minute timeout; failures prevent startup. No custom schema is created.
3. Close the migration connection, then open the runtime pool.

The API receives both secrets. Closing the migration connection does not remove
its secret from the process environment. The explicit diagnostic command needs
only `MIGRATION_DATABASE_URL`:

```sh
cargo run -p captures-api -- migrate
```

As the database/schema administrator, allow the migration role to create tables
(adapt these example role names to the actual PostgreSQL role names):

```sql
GRANT USAGE, CREATE ON SCHEMA public TO captures_migrator;
```

After migration, use the table-owning migration role to grant runtime access:

```sql
GRANT USAGE ON SCHEMA public TO captures_app;
GRANT SELECT, INSERT, UPDATE ON users TO captures_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON auth_email_challenges, account_sessions,
    assets, shares, share_viewer_grants, share_unlock_attempts TO captures_app;
GRANT USAGE ON SEQUENCE users_id_seq TO captures_app;
```

Object ownership and name lookup are separate. Using `public` removes the need
for a schema prefix; it does not let the runtime role or a temporary web-console
role drop tables owned by the migration role.

Audit database `CONNECT`, schema, and role-inheritance privileges without globally
revoking access needed by other apps. Use a disposable database for development.

## Transition from the unused custom schema

The initial migration now targets `public` for the approved empty installation.
This is not an automatic data move or upgrade of the old migration ledger. If the
old account tables have already been deleted, **no further table deletion is
needed**. An empty `captures` schema can remain; the API no longer uses or creates it.
Caper's separate database already targets `public`; it needs no schema reset for
this Captures transition.

Before the new API image starts, stop any old Captures API instance that could
recreate the custom-schema tables. From the infrastructure checkout with the
production Kubernetes context selected:

```sh
flux suspend kustomization production-apps -n flux-system
kubectl -n default scale deployment/captures-api --replicas=0
kubectl -n default wait --for=delete pod -l app.kubernetes.io/name=captures-api --timeout=5m
```

If the old tables still exist, connect to the `captures` database using the
**table-owning migration credentials**, not the runtime role or temporary browser
console role. For this empty installation only, run the following SQL. It does
not drop schemas/databases or any `public` tables, and deliberately omits `CASCADE`:

```sql
DO $$ BEGIN
  IF current_database() <> 'captures' THEN
    RAISE EXCEPTION 'Expected captures database';
  END IF;
  DROP TABLE IF EXISTS captures.users, captures._sqlx_migrations;
END $$;
```

Keep the connection's default search path (`SHOW search_path;`). If this session
previously used `SET search_path TO captures`, reconnect or run `RESET search_path`.
The migration role must have `USAGE, CREATE` on `public` as described above. Do not
reset passwords or widen the runtime role's permissions to perform DDL.

Deploy the new API image through the existing infrastructure deployment workflow;
its startup creates `public.users` and `public._sqlx_migrations`. For an explicit
migration before deployment, run from this checkout with the direct migration URL
securely exported (this command belongs in a terminal, not the SQL console):

```sh
cargo run -p captures-api -- migrate
```

If `public.users` or a conflicting public migration ledger already exists before
this first migration, stop and inspect it rather than dropping it or changing
checksums. The migration will not overwrite existing tables. Reapply the runtime
grants above to the newly created tables. Do not restart the old API image; verify
the new API image pin in infrastructure `main` before resuming reconciliation:

```sh
flux resume kustomization production-apps -n flux-system
flux reconcile kustomization production-apps -n flux-system --with-source
kubectl -n default rollout status deployment/captures-api --timeout=15m
```

After startup, connect to the `captures` database as the migration or runtime role
and verify unqualified queries work without any `SET search_path`:

```sql
SELECT current_database(), current_schema(); -- captures, public
SELECT * FROM users;
```

Account HTTP access remains unavailable (503). Publishing or merging this PR does
not reset a production database or deploy an image.

## Build and test

```sh
cargo test -p captures-api
cargo clippy -p captures-api --all-targets -- -D warnings
cargo build --locked --release -p captures-api
docker build -f apps/api/Dockerfile -t captures-api .
TEST_DATABASE_URL=postgres://captures_test@127.0.0.1:55432/postgres \
  cargo test -p captures-api -- --include-ignored
```

The ignored PostgreSQL test creates a unique disposable database, concurrently
runs startup migrations, verifies that only `public` contains the users table and
ledger, checks unqualified reads/writes/DDL and unverified defaults, verifies a
restart preserves rows, and drops that database. The test role
must be able to create databases. Never supply production credentials.

## Image publication and routing

The `AWS API image` workflow builds a non-root `linux/arm64` API image on PRs and
merges. A disposable PostgreSQL container checks migrations, restart, health,
account unavailability, and closure of the migration connection.

On `main`, the tested image is published to `production/captures` in ECR as
`api-<full Git SHA>`. Website images retain unprefixed SHA tags. Both use the
existing OIDC publisher role. Reruns preserve immutable tags; deploy by tag and
digest, never `latest`.

Successful publication sends a **Captures API image is ready** notification with
an exact SHA/digest through `DEPLOY_NOTIFICATION_WEBHOOK_URL`. The **Deploy
Captures API** button uses Godis's `captures-api` route to
`deploy-captures-api.yml`; **Open GitHub** is the workflow fallback. Missing webhook
configuration skips notification. A failed notification can be retried without
overwriting the image. Publication does not deploy or configure secrets.

- `captur.es/api/*` and `api.captur.es/api/*` route to `captures-api`, including
  feedback, Preview updater manifests, accounts, and asset sharing.
- Before sharing activation, route `captur.es/media/*` to the media Worker; keep
  `/api/*` on the API origin so authorization subrequests cannot loop into the Worker.
  Other `captur.es` requests route to `captures-web`.
- Keep Rust `/health` internal. No native login or token storage is implemented or
  claimed tested on macOS, Windows, or Linux.

Infrastructure still owns Service/Ingress, Tunnel routing, DNS, TLS, and deployment
pins. No cluster, production database, or cloud configuration is changed by tests.
