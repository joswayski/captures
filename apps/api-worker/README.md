# Captures API Worker

Cloudflare Worker for the same-origin `https://captur.es/api/*` API. It deploys
separately from the assets-only website even though both projects use this Git
repository.

## Deployment isolation

| Project | Command | Wrangler configuration | Public traffic |
| --- | --- | --- | --- |
| `captures` | `npm run deploy:web` | `apps/web/wrangler.jsonc` | Static `captur.es/*` assets |
| `captures-api` | `npm run deploy:api` | `apps/api-worker/wrangler.jsonc` | Worker route `captur.es/api/*` |

The web command runs inside the `@captures/web` npm workspace and cannot discover
or deploy this configuration. The API command runs inside the
`@captures/api-worker` workspace and has no static-assets configuration.

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/health` | Liveness check |
| `POST` | `/api/feedback` | Validate, rate-limit, and post feedback to Discord |

The feedback request format and Discord payload match the existing Rust service.
The Cloudflare rate-limiting binding permits one accepted submission per client IP
per minute. Invalid payloads do not consume the limit.

## Local development

Create an ignored `apps/api-worker/.dev.vars` file:

```dotenv
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

Then run from the repository root:

```sh
npm run dev:api
curl -sS http://localhost:8787/api/health
```

## Validation

```sh
npm run build:api
npm run deploy:api -- --dry-run
```

## Cloudflare Workers Builds

Create a second Worker project by importing this same GitHub repository:

- Project name: `captures-api`
- Production branch: `main`
- Build command: `npm run build:api`
- Deploy command: `npm run deploy:api`
- Root directory: repository root (leave blank)
- Build variable: `NODE_VERSION=24`
- Builds for non-production branches: disabled unless API preview URLs are wanted

This assumes `captur.es` is already connected to the assets-only `captures`
Worker as its Custom Domain. The more-specific Wrangler route sends only
`captur.es/api/*` to this API Worker. All other requests continue to the static
assets project and do not invoke API code. The API Worker's `workers.dev` route is
disabled.

After the first Worker version exists, add `DISCORD_WEBHOOK_URL` as an encrypted
secret under **Settings → Variables and Secrets**, then redeploy. It can also be
set from the repository root with:

```sh
npx wrangler secret put DISCORD_WEBHOOK_URL --config apps/api-worker/wrangler.jsonc
```

The existing Railway service at `api.captur.es` remains active for released
desktop clients. After this Worker is verified, a follow-up can switch new clients
to `https://captur.es/api/feedback` and route the old hostname to this Worker for
compatibility before Railway is retired.
