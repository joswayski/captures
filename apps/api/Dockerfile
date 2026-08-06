# Captures API (Discord-backed feedback).
# Build from the monorepo root:
#   docker build -f apps/api/Dockerfile -t captures-api .
#   docker run --rm -p 8080:8080 -e DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/... captures-api

FROM rust:1.94-bookworm AS build

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy the monorepo workspace. .dockerignore drops the desktop app (and other
# heavy paths), but root Cargo.toml still lists apps/desktop/src-tauri as a
# workspace member — cargo refuses to load the workspace if that path is
# missing. Strip it before building so Railway/API images only need apps/api
# plus the shared crates that remain in the build context.
COPY . .
RUN sed -i '/"apps\/desktop\/src-tauri",/d' Cargo.toml \
  && cargo build --release -p captures-api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

ENV PORT=8080
ENV RUST_LOG=info

COPY --from=build /app/target/release/captures-api /usr/local/bin/captures-api

EXPOSE 8080

CMD ["captures-api"]
