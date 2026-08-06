# Captures API

Small Rust HTTP service for early product feedback (and a foundation for later hosted features).

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/health` | Liveness check |
| `POST` | `/api/feedback` | Store a feedback submission |

### `POST /api/feedback`

```json
{
  "message": "Recording freezes when…",
  "contact": "@handle or github/user",
  "category": "bug",
  "app_version": "0.1.0",
  "os": "macos",
  "os_version": "15.5",
  "arch": "aarch64",
  "source": "desktop"
}
```

- `message` is required (max 8,000 characters).
- `contact` is optional free text (X handle, GitHub username, email, etc.).
- `category` defaults to `bug` and accepts `bug`, `idea`, or `other`.
- Rows use a `BIGSERIAL` primary key.

## Local development

```sh
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/captures
cargo run -p captures-api
```

The process applies `migrations/001_feedback.sql` on startup.

## Docker / Railway

```sh
docker build -f apps/api/Dockerfile -t captures-api .
docker run --rm -p 8080:8080 -e DATABASE_URL="$DATABASE_URL" captures-api
```

Set `DATABASE_URL` (required) and optionally `PORT` (default `8080`) / `BIND_ADDR`.

## Reading feedback

```sql
SELECT id, created_at, category, app_version, os, contact, left(message, 120)
FROM feedback
ORDER BY id DESC
LIMIT 50;
```
