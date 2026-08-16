# Captures website

Minimal work-in-progress page with a Preview download for the visitor's OS
(same stable `preview` release assets as the root README), a View source link to
[the repo](https://github.com/joswayski/captures), and recent changes from `main`.
Other platforms are mentioned with a link to GitHub Releases rather than listed as
installers.

## Stack

- React 19 + TanStack Start and Router
- Vite + Nitro, producing a Node server
- A framework-independent `/api/*` handler in front of TanStack
- Tailwind CSS v4

The homepage is server-rendered so the first HTML already includes the matching
Preview download. Latest changes are still baked in at build time. TanStack
renders the frontend; it does not route or implement `/api/*`.

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
the Preview installer from the request `User-Agent` and `Sec-CH-UA-*` headers.
Client-side JavaScript still handles clipboard feedback, relative times, and
Preview publishing status.

Start the production server with:

```sh
npm run start:web
```

`PORT` (default `3000`) and `HOST` (default all interfaces in the image) are
read from the environment.

## Request routing

The Node process serves the site and API together:

1. `/api/*`, `/feedback`, and `/health` run the framework-independent handler
   first and never enter TanStack.
2. `/` is server-rendered so TanStack can pick the homepage download button from
   request headers.
3. Hashed `/assets/*` files and other public files are served as static assets
   with long-lived cache headers.
4. Unknown paths return the in-app 404 page.

Nitro serves those API paths from `server/routes` and never enters TanStack.
Those handlers call the framework-independent implementation in `src/server`.
The homepage is delegated to TanStack.

The current API exposes `GET /api/health` and `POST /api/feedback`. Feedback is
validated, limited to one accepted submission per client IP per minute, and sent
to Discord. Rate limits are in-memory (one Railway replica). Client IP prefers
Cloudflare’s `CF-Connecting-IP`, then `X-Real-IP`, and never a client-spoofable
`X-Forwarded-For` value. Set `DISCORD_WEBHOOK_URL` as a Railway service variable.

Packaged desktop builds from August 2026 still POST to
`https://api.captur.es/feedback` (and a few even earlier ones to
`/api/feedback` on that host). Keep those paths as aliases of the same
handlers, and keep `api.captur.es` as a custom domain on the same Railway
service so those Previews keep working.

Keep Railway unreachable except through Cloudflare so those forwarding headers
stay trustworthy.

## Railway

Deploy from the monorepo root as one Docker service:

| Setting | Value |
| --- | --- |
| Builder | Dockerfile |
| Dockerfile path | `Dockerfile` |
| Root directory | repository root (leave blank) |
| Watch paths | `apps/web/**`, `shared/**`, `package.json`, `package-lock.json`, `Dockerfile` |
| Public port | `3000` (`PORT` is injected) |
| Health check | `GET /api/health` |

Required env on the **web** service: `DISCORD_WEBHOOK_URL`.

Connect the GitHub repo so pushes to `main` rebuild the image. Railway supplies
`RAILWAY_GIT_COMMIT_SHA`, which busts the Docker layer that fetches homepage
history.

From the monorepo root, a local image is:

```sh
docker build -t captures-web .
docker run --rm -p 8080:3000 -e DISCORD_WEBHOOK_URL="$DISCORD_WEBHOOK_URL" captures-web
```

Generate a Railway domain such as `*.up.railway.app` first, then attach
`captur.es` and `api.captur.es` as custom domains. Railway will give you a
CNAME target and a TXT ownership record for each.

## Cloudflare in front

`captur.es` stays on Cloudflare. Railway is the origin. Visitors hit Cloudflare;
hashed JS/CSS are cached at the edge; HTML and `/api/*` go to Railway.

1. In Railway, add the custom domains `captur.es` and `api.captur.es` and copy
   the CNAME plus TXT records.
2. In Cloudflare DNS:
   - `CNAME @` → the Railway `*.up.railway.app` target, **proxied** (orange cloud).
   - `CNAME api` → the Railway target Railway printed for `api.captur.es`,
     **proxied**.
   - Each Railway TXT ownership record, **DNS only**.
   - Optional: `CNAME www` → `@`, proxied, plus a 301 redirect to `https://captur.es`.
3. SSL/TLS → Overview: **Full**, not Flexible and not Full (Strict). Railway
   documents that Strict does not work as intended when the orange cloud is on.
4. SSL/TLS → Edge Certificates: enable Universal SSL.
5. After DNS verifies, Railway should show **Cloudflare proxy detected**.

Do **not** 301 `api.captur.es/feedback` to `captur.es/api/feedback`. Desktop
clients POST JSON; a redirect would drop the body.

### Cache

Origin `Cache-Control` is the source of truth. Cloudflare already honors it, so
Cache Rules are optional. Confirmed live behavior:

| Path | Origin header | Cloudflare |
| --- | --- | --- |
| `/assets/*` (hashed JS/CSS) | `public, max-age=31536000, immutable` | cached (`HIT`) |
| `/favicon.png`, `/icon.svg` | `public, max-age=86400` | cached (`HIT`) |
| `/` | `private` plus `Vary` on OS hint headers | not cached (`DYNAMIC`) |
| `/api/*`, `/feedback`, `/health` | `no-store` | not cached (`DYNAMIC`) |

If you add Cache Rules anyway, keep them aligned with those headers:

| Rule | Match | Action |
| --- | --- | --- |
| Hashed assets | hostname is `captur.es` or `api.captur.es` and URI Path starts with `/assets/` | Eligible for cache, Edge TTL 1 year, respect origin `Cache-Control` |
| Dynamic | hostname is `captur.es` or `api.captur.es` and (URI Path equals `/` or starts with `/api/` or equals `/feedback` or `/health`) | Bypass cache |

Do not use Cache Everything on `/`. After a deploy, hashed filenames change, so
visitors pick up new JS/CSS without a purge. Purge `/` only if a stale homepage
HTML response is stuck at the edge.
