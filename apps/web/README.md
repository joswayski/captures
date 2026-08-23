# Captures website

Minimal work-in-progress page with a Preview download for the visitor's OS
(same stable `preview` release assets as the root README), a View source link to
[the repo](https://github.com/joswayski/captures), and recent changes from `main`.
Phones, tablets, Chrome OS, and other systems without a matching installer see
View source instead of a download button. Other desktop platforms are mentioned
with a link to GitHub Releases rather than listed as installers.

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

The current API exposes `GET /api/health` and `POST /api/feedback`. Feedback is
validated, limited to one accepted submission per client IP per minute, and sent
to Discord. Rate limits are in-memory (one replica). Client IP prefers
Cloudflare’s `CF-Connecting-IP`, then `X-Real-IP`, and never a client-spoofable
`X-Forwarded-For` value. Set `DISCORD_WEBHOOK_URL` in the production app secret.

Keep the origin unreachable except through Cloudflare so those forwarding headers
stay trustworthy.

## AWS

The production origin is one `linux/arm64` container on the AWS k3s cluster. The
process listens on port `3000` (`PORT` / `HOST` from the environment). Health
check is `GET /api/health`. Run one replica: the feedback rate limiter is
in-memory. Required env: `DISCORD_WEBHOOK_URL`.

The `AWS image` GitHub Actions workflow builds the production Dockerfile for
`linux/arm64` on pull requests and verifies `/api/health` inside the resulting
container. After a commit reaches `main`, it uses GitHub OIDC to publish that
same application shape to the private `production/captures` ECR repository.
Images use the immutable application Git SHA as the tag; Kubernetes deployments
must also pin the ECR digest and must not use `latest` or a Docker-login Secret.
The short-lived GitHub token used to fetch homepage history is mounted only as a
BuildKit secret and is not stored in the image or its build arguments.

Publishing does not deploy. Start the manual `Deploy production` workflow from
`main` to deploy only Captures and wait for its Kubernetes rollout:

```sh
gh workflow run deploy-production.yml --repo joswayski/captures --ref main
```

The workflow defaults to the current `main` SHA. For rollback, add
`-f git_sha=<full-40-character-main-sha>` for an image previously published by
the `main` pipeline. It resolves the ECR digest, assumes the Captures-only AWS
deployer role, selects a running production k3s server (`cluster=production-k3s`,
`role=k3s-server`; prefers `server_1`), and invokes a bounded SSM document; it
does not write to the infrastructure repository or receive a kubeconfig. Flux owns every other
Deployment field but leaves the live image to this workflow. Captures retains
one steady-state replica with `maxSurge: 1` and `maxUnavailable: 0`, so a deploy
temporarily starts one extra pod and removes the old pod only after the new one
is Ready.

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
