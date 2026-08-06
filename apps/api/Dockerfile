# Captures API (feedback and future hosted services).
# Build from the monorepo root:
#   docker build -f apps/api/Dockerfile -t captures-api .
#   docker run --rm -p 8080:8080 -e DATABASE_URL=postgres://... captures-api

FROM rust:1.94-bookworm AS build

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy the full workspace so path dependencies resolve. Only captures-api is built.
COPY . .
RUN cargo build --release -p captures-api

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
