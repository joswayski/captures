# Captures media Worker

GET/HEAD `/media/assets/<id>` and `/media/shares/<id>` authorize with the Rust API,
then stream original bytes from a private R2 binding. IDs are 12-character NanoIDs.
The API never receives file bytes. Uploads remain direct presigned multipart R2
uploads, managed by the API. The Worker does not issue signed GET URLs.

Every request, including HEAD and video ranges, calls
`GET /api/media/{assets|shares}/<id>` with `x-captures-media-key` and the viewer's
Cookie/Authorization headers. The API returns `{key,name,contentType,byteSize}`.
For a share, `key` identifies its asset, not the share ID. New keys are
`assets/<user external_id>/<asset external_id>` (two 12-character NanoIDs).
Legacy flat `assets/<asset external_id>` keys remain readable only when the API
authorizes them. Internal bigint IDs never appear in object paths. No authorization or
media response is cached; redirects and API/storage failures deny the request.
Stopping a share denies subsequent authorizations. An already authorized stream,
browser memory, or saved download cannot be recalled.

## Configuration and activation

- `ASSETS`: private R2 binding. Named environments bind `staging-captures` and
  `production-captures`. Neither `workers.dev` nor preview URLs are enabled.
- `MEDIA_API_ORIGIN`: trusted HTTPS origin reachable from Cloudflare, with no
  path. For production, use the existing API ingress, for example
  `https://api.captur.es`, after verifying its routing. This is **not** the Node
  website's internal `CAPTURES_API_ORIGIN=http://captures-api`.
- `MEDIA_WORKER_SECRET`: independent random secret of at least 32 bytes, shared
  with this environment's Rust API. Use a Worker secret and preserve existing
  keys when adding the matching API environment secret. No database, SES, or
  S3 access credentials belong in the Worker or browser.
- `MEDIA_AUTH_TIMEOUT_MS`: optional timeout, default 5000, maximum 30000.

Before enabling sharing, separately approve and configure:

1. Matching API/Worker secrets, a compatible API image, and the API origin above.
2. The same-origin `captur.es/media/*` Worker route. Keep `/api/*` routed to the
   API, not this Worker. Host-only account/viewer cookies then work without
   broadening their domain or exposing tokens to JavaScript.
3. Cache bypass for `/media` and descendants **and** all existing `/api`, `/s`,
   and `/account` paths. Keep R2 private with no public bucket endpoints. Worker
   headers alone are not a substitute for auditing overriding cache rules.
4. Real SES sign-in and direct R2 upload, owner download, password unlock,
   cross-viewer isolation, video seeking, expiry/password edit/stop-share denial,
   and deletion tests. Verify API failures deny downloads and no object bytes
   pass through the API.

This package prepares code/configuration only. It does not apply routes, cache
rules, credentials, bucket CORS, or production feature gates. Deployment commands
require separate approval. R2 egress is free; Worker execution and storage
operations remain billable.

## Local testing

Run `npm test --workspace @captures/media-worker` and
`npm run typecheck --workspace @captures/media-worker` from the repository root.
Validate packaging without deployment:

```sh
npx wrangler deploy --dry-run --env staging --config apps/media-worker/wrangler.jsonc
npx wrangler deploy --dry-run --env production --config apps/media-worker/wrangler.jsonc
```

For local integration, put `MEDIA_API_ORIGIN=http://127.0.0.1:3001` and a local-only
matching `MEDIA_WORKER_SECRET` in ignored `apps/media-worker/.dev.vars.staging`.
HTTP is accepted only for loopback hosts. Start the local API, then:

```sh
npx wrangler dev --local --env staging --config apps/media-worker/wrangler.jsonc
```

This uses emulated R2, not the real staging bucket; seed test objects with
`wrangler r2 object put ... --local` using the same environment/config. The web
dev server proxies `/media/*` to port 8787 by default (`CAPTURES_MEDIA_ORIGIN`
overrides it). Never use live bucket data or credentials for local tests.
