# Captures website

Minimal work-in-progress page with a Preview download for the visitor's OS
(same stable `preview` release assets as the root README), a short product
screenshot gallery (the stills in `docs/images/`, shared with the README; each
shot shows in full; swipe or use Previous/Next to change stills — those buttons
hide at the first and last shot — pinch to zoom in place, or tap for a larger
view), a
View source link to [the repo](https://github.com/joswayski/captures), and
recent changes from `main`.
Phones, tablets, Chrome OS, and other systems without a matching installer see a
desktop-app note and View on GitHub — not download copy or a GitHub Releases
link. Other desktop platforms are mentioned with a link to GitHub Releases rather
than listed as installers.

## Stack

- React 19 + TanStack Start and Router
- Vite + Nitro, producing a Node server
- A framework-independent `/api/*` handler in front of TanStack
- Tailwind CSS v4

The homepage is server-rendered so the first HTML already includes the matching
Preview download and any "still cooking" Preview status. Latest changes are
still baked in at build time. TanStack renders the frontend; it does not route
or implement `/api/*`.

## Develop

```sh
npm run dev:web
```

Site runs at [http://localhost:5174](http://localhost:5174).

For the feedback endpoint, create an ignored `apps/web/.env` file:

```dotenv
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

The local API is available at `http://localhost:5174/api/*`.

## Build

```sh
npm run build:web
```

Nitro emits a standalone Node server in `apps/web/.output`. The build fetches
recent `main` commits from the GitHub API, drops Dependabot dependency bumps, and
embeds the latest ten product changes in the homepage payload. The server picks
the Preview installer from the request `User-Agent` and `Sec-CH-UA-*` headers,
and looks up Preview publishing status from the GitHub API (cached in memory for
one hour) so "still cooking" is in the first HTML. Client-side JavaScript still
handles clipboard feedback and relative times.

Start the production server with:

```sh
npm run start:web
```

`PORT` (default `3000`) and `HOST` (default all interfaces in the image) are
read from the environment.

## Request routing

The Node process serves the site and API together:

1. `/api/*` runs the framework-independent handler first and never enters TanStack.
2. `/` is server-rendered so TanStack can pick the homepage download button from
   request headers.
3. Hashed `/assets/*` files and other public files are served as static assets
   with long-lived cache headers.
4. Unknown paths return the in-app 404 page.

Nitro serves `/api/*` from `server/routes/api` and never enters TanStack.
Those handlers call the framework-independent implementation in `src/server`.
The homepage is delegated to TanStack.

The base API exposes liveness at `GET /api/health`, dependency readiness at
`GET /api/ready`, and feedback at `POST /api/feedback`. Feedback is
validated, limited to one accepted submission per client IP per minute, and sent
to Discord. Desktop Preview builds may also POST `category: "crash"` after an
unexpected quit (version, OS, and a redacted panic or OS crash summary — never captures).
Rate limits are in-memory (one replica). Client IP prefers
Cloudflare’s `CF-Connecting-IP`, then `X-Real-IP`, and never a client-spoofable
`X-Forwarded-For` value. Set `DISCORD_WEBHOOK_URL` in the production app secret.

Keep the origin unreachable except through Cloudflare so those forwarding headers
stay trustworthy.

## Sharing and accounts

Set `SHARING_ENABLED=true` to add account and asset-sharing routes to this same
Nitro process. There is no separate API service, upload worker, or public bucket.
The API stores ownership and sharing state in PostgreSQL and returns short-lived
presigned URLs so browsers and desktop clients transfer bytes directly to an
S3-compatible private bucket.

Required configuration:

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Pooled PostgreSQL URL used by requests |
| `DATABASE_MIGRATION_URL` | Direct PostgreSQL URL used for startup migrations |
| `PUBLIC_ORIGIN` | Canonical origin, normally `https://captur.es` |
| `STORAGE_BACKEND` | Metadata label such as `r2` or `tigris` |
| `STORAGE_ENDPOINT` | S3-compatible endpoint |
| `STORAGE_REGION` | S3 signing region (`auto` for R2) |
| `STORAGE_BUCKET` | Private asset bucket |
| `STORAGE_ACCESS_KEY_ID` / `STORAGE_SECRET_ACCESS_KEY` | Bucket-scoped S3 credentials |
| `AUTH_CODE_HMAC_KEY` | Base64 encoding of at least 32 random bytes; generated once |
| `AUTH_PUBLIC_SIGNUP` | Must remain `false`; enabling it fails startup until both clients implement Turnstile |
| `AUTH_ALLOWED_EMAILS` | Comma-separated private-beta emails |
| `AUTH_ALLOWED_CIDRS` | Comma-separated private-beta IPs or CIDRs |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | Optional Google OAuth credentials |
| `SES_SMTP_HOST` / `SES_SMTP_PORT` / `SES_SMTP_SECURE` | SES SMTP connection |
| `SES_SMTP_USERNAME` / `SES_SMTP_PASSWORD` | Captures-only SES SMTP credential |
| `SES_FROM_EMAIL` | Verified Captures sender, normally `Captures <login@captur.es>` |
| `SES_CONFIGURATION_SET` | Captures SES configuration set |
| `SES_TENANT` | Captures SES tenant |
| `SES_SNS_TOPIC_ARN` | Exact topic accepted by the signed SES/SNS event endpoint |

The private beta checks both the email allowlist and the trusted client IP. The
public source code and route names are not treated as protection. Public signup
is intentionally rejected at startup until both the web and desktop clients have
a complete Turnstile attestation flow. Login codes are six digits, expire after
ten minutes, and are stored only as keyed hashes.

The important routes are:

- `GET /api/auth/providers` reports optional login methods without exposing credentials
- `POST /api/auth/email/start` and `/verify` for web or desktop login
- `GET /api/auth/google/start` and `/callback` for optional web Google login
- `POST /api/auth/refresh` for desktop refresh-token exchange
- `GET /api/me` and `POST /api/auth/logout`
- `GET|POST /api/assets`, with NanoID cursor pagination plus owner read/delete/media routes
- `POST /api/assets/:id/complete` and `/parts` for verified uploads and expiring-URL refresh
- `PATCH|POST /api/assets/:id/share` to change access or rotate the random link
- `GET /api/shares/:shareId` for a shared asset
- `POST /api/email/events` for signed SNS subscription, bounce, and complaint events

Uploads up to 100 MiB use one presigned PUT. Larger uploads use 16 MiB multipart
parts. Reservations and multipart uploads expire after 24 hours; presigned URLs
expire after 15 minutes. Completion verifies object size, content type, SHA-256
metadata, and representative magic bytes before charging the account quota. The
per-original and per-account limits are both 1 GiB, including previews and
pending reservations. An hourly in-process cleanup takes a PostgreSQL advisory
lock, aborts expired multipart uploads, deletes stale objects, and releases quota.

Only two access states exist: `private` and `shared`. Private assets appear only
in their owner's library. Shared assets use a separate NanoID link identifier;
rotating a link does not change the asset ID. There is deliberately no global
gallery or enumeration endpoint. The schema reserves a password hash and access
version so password-protected links can be added without changing link identity.
See [`../../docs/asset-sharing.md`](../../docs/asset-sharing.md) for the data model
and request flows.

## AWS

The production origin runs two `linux/arm64` replicas on the AWS k3s cluster. The
process listens on port `3000` (`PORT` / `HOST` from the environment). Health
checks remain on `GET /api/health`, which waits for sharing migrations and
dependencies during process startup and becomes a process-liveness signal once
initialization succeeds. `GET /api/ready` exposes the same startup gate for
operators. The feedback limiter remains in-memory per replica. Sharing
authentication limits are database-backed across replicas.

The `AWS image` GitHub Actions workflow builds the production Dockerfile for
`linux/arm64` on pull requests and verifies `/api/health` inside the resulting
container. After a commit reaches `main`, it uses GitHub OIDC to publish that
same application shape to the private `production/captures` ECR repository.
Images use the immutable application Git SHA as the tag; Kubernetes deployments
must also pin the ECR digest and must not use `latest` or a Docker-login Secret.
The short-lived GitHub token used to fetch homepage history is mounted only as a
BuildKit secret and is not stored in the image or its build arguments.

Publishing does not deploy. Rollout is the `Deploy Captures` workflow in
[`joswayski/infrastructure`](https://github.com/joswayski/infrastructure):

```sh
gh workflow run deploy-captures.yml \
  --repo joswayski/infrastructure \
  --ref main
```

Leave the SHA blank to pin the newest published ECR image. Paste
`-f git_sha=<full-40-character-main-sha>` only for rollback; that SHA must
already exist as an immutable ECR tag from this repository's `main` image
pipeline. Host lookup and the bounded SSM document live in infrastructure so
they are not copied per app. The workflow writes the pin into git; Flux
applies that image. Captures retains one steady-state replica with
`maxSurge: 1` and `maxUnavailable: 0`, so a deploy temporarily starts one
extra pod and removes the old pod only after the new one is Ready.

CI passes `GIT_COMMIT_SHA` (the GitHub SHA) so the Docker layer that fetches
homepage history is not reused across commits.

From the monorepo root, a local image is:

```sh
docker build -t captures-web .
docker run --rm -p 8080:3000 -e DISCORD_WEBHOOK_URL="$DISCORD_WEBHOOK_URL" captures-web
```

When sharing is enabled, the pod also needs the sharing variables above. They
are synchronized from the Captures application secret by the infrastructure
repository; no object-storage, database, or SMTP credential belongs on the EC2
node role.

## Cloudflare in front

`captur.es` stays on Cloudflare. The AWS k3s Deployment is the origin, reached
through Cloudflare Tunnel. Visitors hit Cloudflare; hashed JS/CSS are cached at
the edge; HTML and `/api/*` go to the origin.

1. Point `captur.es` at the production Cloudflare Tunnel, **proxied**.
2. Optional: `CNAME www` → `@`, proxied, plus a 301 redirect to `https://captur.es`.
3. SSL/TLS → Overview: **Full (Strict)**.
4. SSL/TLS → Edge Certificates: enable Universal SSL.

Cache behavior comes from origin headers plus two Cache Rules:

| Rule | Match | Action |
| --- | --- | --- |
| Hashed assets | hostname is `captur.es` and URI Path starts with `/assets/` | Eligible for cache, Edge TTL 1 year, respect origin `Cache-Control` |
| Dynamic | hostname is `captur.es` and (URI Path equals `/` or starts with `/api/`) | Bypass cache |

The homepage already sends `Cache-Control: private` and `Vary` on the OS hint
headers, so a missed Bypass rule still should not share one download button
across macOS, Windows, and Linux. Do not use Cache Everything on `/`.

After a deploy, hashed filenames change, so visitors pick up new JS/CSS without
a purge. Purge `/` only if a stale homepage HTML response is stuck at the edge.
