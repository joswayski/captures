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
- TanStack Start server route for `/health`
- Tailwind CSS v4

The homepage is server-rendered so the first HTML already includes the matching
Preview download and any "still cooking" Preview status. Latest changes are
still baked in at build time.

## Develop

```sh
npm run dev:web
```

Site runs at [http://localhost:5174](http://localhost:5174). Its local health
endpoint is `http://localhost:5174/health`.

## Build

```sh
npm run build:web
```

Nitro emits a standalone Node server in `apps/web/.output`. The build fetches
recent `main` commits from the GitHub API, drops dependency bumps, and
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

The Node process serves the site:

1. `/health` is the Node container health route.
2. `/` is server-rendered so TanStack can pick the homepage download button from
   request headers.
3. Hashed `/assets/*` files and other public files are served as static assets
   with long-lived cache headers.
4. `/account` provides optional sign-in and the capture library; `/s/<id>` renders
   access-controlled share pages, with fresh server-side metadata.
5. Unknown paths return the in-app 404 page.

Nitro is the Node adapter. The Rust API owns `captur.es/api/*`; the frontend
does not define API routes.

## AWS

The production origin is one `linux/arm64` container on the AWS k3s cluster. The
process listens on port `3000` (`PORT` / `HOST` from the environment). Health
check is `GET /health`.

The `AWS image` GitHub Actions workflow builds the production Dockerfile for
`linux/arm64` on pull requests and verifies `/health` inside the resulting
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

## Cloudflare in front

`captur.es` stays on Cloudflare. The AWS k3s Deployment is the origin, reached
through Cloudflare Tunnel. Visitors hit Cloudflare; hashed JS/CSS are cached at
the edge; HTML and `/api/*` go to the origin.

1. Point `captur.es` at the production Cloudflare Tunnel, **proxied**.
2. Optional: `CNAME www` → `@`, proxied, plus a 301 redirect to `https://captur.es`.
3. SSL/TLS → Overview: **Full (Strict)**.
4. SSL/TLS → Edge Certificates: enable Universal SSL.

Required cache behavior comes from origin headers plus two Cache Rules. The
account/share bypass below must be verified before activation; this code change
does not update live Cloudflare rules.

| Rule | Match | Action |
| --- | --- | --- |
| Hashed assets | hostname is `captur.es` and URI Path starts with `/assets/` | Eligible for cache, Edge TTL 1 year, respect origin `Cache-Control` |
| Dynamic | hostname is `captur.es` and (URI Path equals `/`, `/account`, `/api`, or `/s`, or starts with `/account/`, `/api/`, or `/s/`) | Bypass cache |

The homepage already sends `Cache-Control: private` and `Vary` on the OS hint
headers, so a missed Bypass rule still should not share one download button
across macOS, Windows, and Linux. Do not use Cache Everything on `/`.

After a deploy, hashed filenames change, so visitors pick up new JS/CSS without
a purge. Purge `/` only if a stale homepage HTML response is stuck at the edge.

## Optional accounts

`/account` uses the Rust API's email-code sign-in and secure HttpOnly cookie,
then lists owner captures and their links. Authentication is optional; no tokens
are stored in localStorage. When the API disables accounts, the page shows an
unavailable notice. Multipart direct-to-R2 uploads, deletion, one editable share
link per capture, passwords and expiry are implemented; native app integration
follows the rewrite. GIF, video and raster files render in place; unknown or
unsafe inline formats are offered as downloads.

Browser calls remain same-origin `/api/*`. Set **server-only**
`CAPTURES_API_ORIGIN` on the Node website to the trusted internal Rust API origin
(`http://captures-api` in production: Service port 80 forwards to container port
3001; local default `http://127.0.0.1:3001`).
Share-page SSR forwards the viewer cookie to that origin only, rejects redirects,
and never caches metadata. Node receives no database, SES or R2 credentials.
See [`../api`](../api/README.md) for auth and storage configuration.

`/account` and `/s/*` send `no-store` and `Referrer-Policy: no-referrer`; every
share state also sends `noindex`. Noindex is a crawler instruction, not access
control. Media authorization is
enforced again by Rust, so stale page HTML cannot bypass revocation. Configure
the same-origin API routing and dynamic cache bypass before activation. The
development Vite proxy routes `/api` to the local API; production ingress must
do this separately.

Direct uploads require the R2 bucket CORS policy to allow `PUT` from the exact
website origin and to expose the `ETag` response header. The browser sends only
the headers returned by the multipart API to R2, without account cookies or
authorization headers. This documentation does not change the live R2 policy.
`npm test --workspace @captures/web` covers the upload protocol, media selection,
validation and indexing policy. End-to-end account/storage tests live in the
Rust API suite.
