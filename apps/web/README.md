# Captures website

Minimal static work-in-progress page with a Preview download for the visitor's OS
(same stable `preview` release assets as the root README), a View source link to
[the repo](https://github.com/joswayski/captures), and recent changes from `main`.
Other platforms are mentioned with a link to GitHub Releases rather than listed as
installers.

## Stack

- React 19 + TanStack Start and Router
- Vite + Nitro static prerendering
- Tailwind CSS v4

Every route is prerendered at build time. Production serves the generated files and
does not run the TanStack server bundle.

## Develop

```sh
npm run dev:web
```

Site runs at [http://localhost:5174](http://localhost:5174).

## Build

```sh
npm run build:web
```

Static output lands in `apps/web/.output/public/` — deploy those files as-is. The
build fetches recent `main` commits from the GitHub API, drops Dependabot dependency
bumps, and embeds the latest ten product changes in the prerendered page. Client-side
JavaScript still handles OS-specific downloads, clipboard feedback, relative times,
and Preview publishing status.

## Cloudflare

The site deploys as a Cloudflare Workers Static Assets project. The Wrangler
configuration has no Worker entry point: it uploads only the prerendered files in
`.output/public/`.

The web and API deployments are intentionally isolated. `npm run deploy:web`
runs Wrangler inside `apps/web` and can only deploy the assets-only `captures`
project. The separate `npm run deploy:api` command uses
`apps/api-worker/wrangler.jsonc` and can only deploy the `captures-api` Worker.

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
