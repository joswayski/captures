# Portable image for the Captures marketing site (apps/web).
# Build from the monorepo root:
#   docker build -t captures-web .
#   docker run --rm -p 8080:80 captures-web

FROM node:24-alpine AS build

WORKDIR /app

COPY package.json package-lock.json ./
COPY apps/web/package.json apps/web/
COPY apps/desktop/package.json apps/desktop/

RUN npm ci

COPY apps/web apps/web

RUN npm run build --workspace=@captures/web

FROM nginx:1.27-alpine

COPY apps/web/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/apps/web/dist /usr/share/nginx/html

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
