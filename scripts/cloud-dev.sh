#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "${1:-}" == "init" ]]; then
  if [[ -e "$repo_root/.env.local" ]]; then
    echo '.env.local already exists; leaving it unchanged.' >&2
    exit 1
  fi
  auth_secret="$(openssl rand -hex 32)"
  media_secret="$(openssl rand -hex 32)"
  (
    umask 077
    set -o noclobber
    sed -e "s/^AUTH_SECRET=$/AUTH_SECRET=$auth_secret/" \
      -e "s/^MEDIA_WORKER_SECRET=$/MEDIA_WORKER_SECRET=$media_secret/" \
      "$repo_root/.env.example" > "$repo_root/.env.local"
  )
  echo 'Created .env.local with independent local secrets. Fill in your SSO, SES and Cloudflare settings.'
  exit 0
fi

export LOCAL_UID="$(id -u)"
export LOCAL_GID="$(id -g)"
exec docker compose --project-directory "$repo_root" \
  --env-file "$repo_root/.env.local" -f "$repo_root/compose.yaml" "$@"
