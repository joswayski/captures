# Portable image for the Captures marketing site (apps/web).
# Build from the monorepo root:
#   docker build -t captures-web .
#   docker run --rm -p 8080:3000 captures-web

FROM node:24-alpine AS build

WORKDIR /app

COPY package.json package-lock.json ./
COPY apps/web/package.json apps/web/
COPY apps/desktop/package.json apps/desktop/

RUN npm ci

COPY apps/web apps/web
COPY shared shared

# Railway provides a different commit SHA for every GitHub deployment. Referencing
# it here invalidates this layer so the static site gets a fresh main-branch timeline.
ARG RAILWAY_GIT_COMMIT_SHA=local
RUN RAILWAY_GIT_COMMIT_SHA="$RAILWAY_GIT_COMMIT_SHA" npm run build --workspace=@captures/web

FROM node:24-alpine

WORKDIR /app

ENV NODE_ENV=production
ENV PORT=3000

COPY apps/web/package.json ./
RUN npm install --omit=dev

COPY --from=build /app/apps/web/dist ./dist

EXPOSE 3000

CMD ["npm", "start"]
