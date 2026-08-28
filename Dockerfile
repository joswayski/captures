# syntax=docker/dockerfile:1

# Node server for the Captures website (TanStack Start + /api/*).
# Build from the monorepo root:
#   docker build -t captures-web .
#   docker run --rm -p 8080:3000 -e DISCORD_WEBHOOK_URL="$DISCORD_WEBHOOK_URL" captures-web

FROM node:24-alpine AS build

WORKDIR /app

COPY package.json package-lock.json ./
COPY apps/web/package.json apps/web/
COPY apps/desktop/package.json apps/desktop/

RUN npm ci

COPY apps/web apps/web
COPY shared shared
COPY docs/images docs/images

# Each Git commit SHA invalidates this layer so the homepage history fetch is
# not reused across deployments.
ARG GIT_COMMIT_SHA=local
RUN --mount=type=secret,id=github_token \
    GITHUB_TOKEN="$(cat /run/secrets/github_token 2>/dev/null || true)" \
    GIT_COMMIT_SHA="$GIT_COMMIT_SHA" \
    npm run build:web

FROM node:24-alpine

WORKDIR /app

ENV NODE_ENV=production
ENV HOST=0.0.0.0
ENV PORT=3000

COPY --from=build /app/apps/web/.output ./.output

EXPOSE 3000

CMD ["node", ".output/server/index.mjs"]
