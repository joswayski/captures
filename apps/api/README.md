# Captures account API foundation

The Rust/Axum service retains a provider-independent PostgreSQL users table and
startup migrations for future account development. Authentication, account
provisioning, and lifecycle webhooks are not available. There are no uploads,
billing, public profiles, organizations, or desktop login.

## Endpoints

| Route | Contract |
| --- | --- |
| `GET /health` | Public liveness; not a database readiness probe |
| `GET /api/account/me` | Always 503 with `Cache-Control: no-store`; credentials do not enable access |

The website shows an unavailable notice at `/account`. It does not create sessions,
forward access tokens, or connect to the Rust API. Local desktop capture is unaffected.

## Runtime configuration

Set variables in the process environment; the Rust binary does not load `.env`
files itself. Never put secrets in the image or desktop bundle.

| Variable | Required/default |
| --- | --- |
| `DATABASE_URL` | Captures runtime role, pooled PlanetScale port **6432**, database **`captures`** |
| `MIGRATION_DATABASE_URL` | Captures migration role, direct port **5432**, same host and database; required at startup |
| `CAPTURES_API_BIND` | `127.0.0.1:3001`; image uses `0.0.0.0:3001` |
| `RUST_LOG` | Optional filter; do not enable request/body or SQL parameter logging in production |

The Node website receives neither database URL nor account secrets.

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
rows must have no email. There is no public identifier or email uniqueness
constraint. No account writes are currently exposed by the service.

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
  feedback, Preview updater manifests, and the unavailable account endpoint.
- Other `captur.es` requests route to `captures-web`.
- Keep Rust `/health` internal. No native login or token storage is implemented or
  claimed tested on macOS, Windows, or Linux.

Infrastructure still owns Service/Ingress, Tunnel routing, DNS, TLS, and deployment
pins. No cluster, production database, or cloud configuration is changed by tests.
