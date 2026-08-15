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

## Docker

From the monorepo root:

```sh
docker build -t captures-web .
docker run --rm -p 8080:3000 captures-web
```

The Docker build gets its homepage history directly from the GitHub API. Squash-merged commits link back to their pull requests, and the build fails instead of publishing hardcoded or stale history if GitHub is unavailable.

Railway supplies `RAILWAY_GIT_COMMIT_SHA` to the Docker build, so the history-fetch
layer is invalidated on each GitHub deployment. The final image contains only the
static output and a small BusyBox HTTP server on port 3000 (override with `PORT`);
Node and the TanStack server bundle are not included.
