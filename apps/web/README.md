# Captures website

Minimal static work-in-progress page with a Preview download for the visitor's OS
(same stable `preview` release assets as the root README), a View source link to
[the repo](https://github.com/joswayski/captures), and recent changes from `main`.
Other platforms are mentioned with a link to GitHub Releases rather than listed as
installers.

## Stack

- React 19 + TanStack Start and Router
- Vite + the Cloudflare Vite plugin
- A raw Cloudflare Worker entrypoint for `/api/*`
- Tailwind CSS v4

The public website is prerendered at build time. The same Cloudflare project also
contains a framework-independent API. TanStack renders the frontend; it does not
route or implement `/api/*`.

## Develop

```sh
npm run dev:web
```

Site runs at [http://localhost:5174](http://localhost:5174).

For the feedback endpoint, create an ignored `apps/web/.dev.vars` file:

```dotenv
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

The local API is available at `http://localhost:5174/api/*`.

## Build

```sh
npm run build:web
```

The Cloudflare Vite plugin emits the deployable Worker and its static client assets.
The build fetches recent `main` commits from the GitHub API, drops Dependabot
dependency bumps, and embeds the latest ten product changes in the prerendered
page. Client-side JavaScript still handles OS-specific downloads, clipboard
feedback, relative times, and Preview publishing status.

## Cloudflare

The site and API deploy together as one Cloudflare Worker named `captures`:

1. `/api/*` runs the raw Worker entrypoint first and never enters TanStack.
2. Every other request checks the generated static assets without invoking Worker
   code.
3. A browser navigation that matches neither a static page nor `/api/*` returns
   HTTP 404 from static-asset routing.

`src/worker/index.ts` is the Worker entrypoint. It owns Cloudflare event handling
and dispatches `/api/*` to the framework-independent implementation in
`src/worker/api.ts`. Only non-API requests that Cloudflare deliberately sends to
the Worker are delegated to TanStack; that is where future explicitly configured
SSR routes would run.

The current API exposes `GET /api/health` and `POST /api/feedback`. Feedback is
validated, limited to one accepted submission per client IP per minute, and sent
to Discord. `DISCORD_WEBHOOK_URL` must be configured as an encrypted runtime
secret under **Settings → Variables and Secrets**.

There are no R2 or Queue bindings. A future Queue producer binding can be used
from API code through `env`, and a Queue consumer adds a top-level `queue()`
handler to `src/worker/index.ts`. If a real SSR route is added later, exclude it
from prerendering and add its explicit path to `assets.run_worker_first`. Unknown
paths remain 404s.

Configure Workers Builds from the monorepo root with:

- Production branch: `main`
- Build command: `npm run build:web`
- Deploy command: `npm run deploy:web`
- Root directory: repository root (leave the setting blank)

For a manual deployment from the monorepo root:

```sh
npm run build:web
npm run deploy:web
```

Each Cloudflare build gets the homepage history directly from the GitHub API.
Squash-merged commits link back to their pull requests, and the build fails instead
of publishing hardcoded or stale history if GitHub is unavailable. Attach
`captur.es` as the custom domain for the `captures` Worker after connecting the
repository.
