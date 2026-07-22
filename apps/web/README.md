# Captures website

Minimal static work-in-progress page that points to [the repo](https://github.com/joswayski/captures) and shows recent changes from `main`.

## Stack

- React 19 + Vite (static client build)
- Tailwind CSS v4

Vite can do SSR later if the site needs it; this app stays a plain static SPA for now.

## Develop

```sh
npm run dev:web
```

Site runs at [http://localhost:5174](http://localhost:5174).

## Build

```sh
npm run build:web
```

Output lands in `apps/web/dist/` — deploy those static files as-is. Normal local builds use the checked-in history snapshot so they remain deterministic.

## Docker

From the monorepo root:

```sh
docker build -t captures-web .
docker run --rm -p 8080:3000 captures-web
```

The Docker build refreshes the latest six `main` commits from GitHub before Vite creates the static site. Squash-merged commits link back to their pull requests. If GitHub is temporarily unavailable, the checked-in snapshot is used instead.

Railway supplies `RAILWAY_GIT_COMMIT_SHA` to the Docker build, so the history-fetch layer is invalidated on each GitHub deployment. The image serves the result with `serve` on port 3000 (override with `PORT`).
