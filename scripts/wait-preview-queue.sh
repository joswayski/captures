#!/usr/bin/env bash
# Wait until every earlier Captures Preview workflow run has finished.
# Transient GitHub API errors retry instead of failing the queue job, so a 502
# cannot drop a main-branch Preview.
set -euo pipefail

if [[ -z "${GITHUB_REPOSITORY:-}" ]]; then
  echo "GITHUB_REPOSITORY is required." >&2
  exit 1
fi
if [[ ! "${GITHUB_RUN_ID:-}" =~ ^[0-9]+$ ]]; then
  echo "GITHUB_RUN_ID must be a numeric workflow run id." >&2
  exit 1
fi

sleep_seconds="${PREVIEW_QUEUE_SLEEP_SECONDS:-30}"
if [[ ! "$sleep_seconds" =~ ^[0-9]+$ ]]; then
  echo "PREVIEW_QUEUE_SLEEP_SECONDS must be a non-negative integer." >&2
  exit 1
fi

while true; do
  set +e
  blocker="$(
    gh api \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "/repos/${GITHUB_REPOSITORY}/actions/workflows/release.yml/runs?branch=main&per_page=100" \
      --jq "[.workflow_runs[] | select(.id < ${GITHUB_RUN_ID} and .status != \"completed\") | .id] | min // empty"
  )"
  status=$?
  set -e

  if [[ "$status" -ne 0 ]]; then
    echo "GitHub API error (${status}) while checking the Preview queue; retrying in ${sleep_seconds}s." >&2
    sleep "$sleep_seconds"
    continue
  fi
  if [[ -z "$blocker" ]]; then
    break
  fi
  echo "Waiting for earlier Preview workflow ${blocker} to finish."
  sleep "$sleep_seconds"
done
