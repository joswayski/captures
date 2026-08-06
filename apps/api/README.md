# Captures API

Tiny Rust HTTP service that receives product feedback from the desktop app and
posts it to a Discord channel webhook. No database.

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/health` | Liveness check |
| `POST` | `/api/feedback` | Validate feedback and post it to Discord |

### `POST /api/feedback`

```json
{
  "message": "Recording freezes when…",
  "contact": "@handle or github/user",
  "category": "bug",
  "app_version": "0.1.0",
  "os": "macos",
  "os_version": "15.5",
  "arch": "aarch64",
  "source": "desktop"
}
```

- `message` is required (max 8,000 characters; Discord embed truncates longer text).
- `contact` is optional free text (X handle, GitHub username, email, etc.).
- `category` defaults to `bug` and accepts `bug`, `idea`, or `other`.
- Rate limit: **one accepted submission per client IP per minute** (HTTP 429).
  Invalid payloads do not consume the cooldown. Limits are in-memory (one pod).
  Client IP prefers Cloudflare’s `CF-Connecting-IP` (then `X-Real-IP`), never a
  client-spoofable `X-Forwarded-For` value. If you terminate TLS at Cloudflare,
  keep Railway unreachable except via Cloudflare so that header stays trustworthy.

Successful response:

```json
{ "ok": true }
```

## Why keep an API at all?

The Discord webhook URL is a secret. The desktop app should not embed it.
This service is a thin validated proxy: rate limit → format → Discord.

## Local development

1. In Discord: channel settings → Integrations → Webhooks → New Webhook → copy URL.
2. Run:

```sh
export DISCORD_WEBHOOK_URL='https://discord.com/api/webhooks/...'
cargo run -p captures-api
```

Point a local desktop build at it:

```sh
export CAPTURES_FEEDBACK_URL=http://127.0.0.1:8080/api/feedback
npm run dev
```

## Docker / Railway

```sh
docker build -f apps/api/Dockerfile -t captures-api .
docker run --rm -p 8080:8080 -e DISCORD_WEBHOOK_URL="$DISCORD_WEBHOOK_URL" captures-api
```

Required: `DISCORD_WEBHOOK_URL`  
Optional: `PORT` (default `8080`), `BIND_ADDR`, `RUST_LOG`
