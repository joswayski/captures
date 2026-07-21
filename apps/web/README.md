# Captures website

Minimal static coming-soon page that points to [the repo](https://github.com/joswayski/captures).

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

Output lands in `apps/web/dist/` — deploy those static files as-is.

## Docker

From the monorepo root:

```sh
docker build -t captures-web .
docker run --rm -p 8080:80 captures-web
```

The image is a static nginx build of this app. Map host port `8080` (or any port) to container port `80`.
