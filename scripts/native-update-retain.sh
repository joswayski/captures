#!/usr/bin/env bash
# Keep the full native archives that later releases patch from on the draft
# `native-update-bases` release, named `<version>--<target>.<tar.gz|zip>`, with
# the newest six builds per target.
#
#   native-update-retain.sh [--dry-run] download DIRECTORY
#     Download every retained base into DIRECTORY for native-update-patches.mjs.
#     A missing release or failed download is a warning: without bases, clients
#     download full archives.
#   native-update-retain.sh [--dry-run] retain MANIFEST ARCHIVE_DIRECTORY
#     After MANIFEST (the signed schema-2 manifest) is live: retain each of its
#     artifacts (the file in ARCHIVE_DIRECTORY named like its URL, checked
#     against its size and SHA-256), then delete bases outside each target's
#     newest six. With RELEASE_TAG set, also delete `.capturespatch` assets on
#     that release that MANIFEST no longer lists. A client holding an older
#     cached manifest whose patch was just removed downloads the full archive.
#
# --dry-run runs only read-only `gh` calls and prints every create, upload and
# delete instead. Needs `gh`, `jq` and Node; in CI, GH_TOKEN with contents write
# and GH_REPO. Not wired into a workflow while native publication is withdrawn.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bases_release=native-update-bases
dry_run=false
if [[ "${1:-}" == "--dry-run" ]]; then
  dry_run=true
  shift
fi

usage() {
  sed -n '6,16p' "${BASH_SOURCE[0]}" >&2
  exit 2
}

# Mutating gh calls go through here; read-only calls run directly.
mutate() {
  if $dry_run; then
    printf 'dry-run: gh'
    printf ' %q' "$@"
    printf '\n'
  else
    gh "$@"
  fi
}

bases() {
  gh release view "$bases_release" --json assets --jq '.assets[].name'
}

case "${1:-}" in
  download)
    [[ $# -eq 2 ]] || usage
    mkdir -p "$2"
    if ! gh release download "$bases_release" --pattern '*--*' --dir "$2" --skip-existing; then
      echo "::warning::No retained native builds to patch from; clients download full archives."
    fi
    ;;
  retain)
    [[ $# -eq 3 ]] || usage
    manifest="$2"
    archives="$3"
    version="$(jq -er '.version' "$manifest")"
    [[ "$(jq -r '.schema' "$manifest")" == 2 ]] || {
      echo "Only schema-2 native manifests are retained." >&2
      exit 1
    }
    staged="$(mktemp -d)"
    trap 'rm -rf "$staged"' EXIT
    while IFS=$'\t' read -r target url size sha256; do
      case "$target" in
        x86_64-unknown-linux-gnu) extension=.tar.gz ;;
        aarch64-apple-darwin | x86_64-apple-darwin | x86_64-pc-windows-msvc) extension=.zip ;;
        *)
          echo "::warning::Not retaining unknown target $target."
          continue
          ;;
      esac
      archive="$archives/$(basename "${url%%[?#]*}")"
      if [[ ! -f "$archive" || "$(wc -c <"$archive" | tr -d ' ')" != "$size" ]] \
        || [[ "$(sha256sum "$archive" | cut -d' ' -f1)" != "$sha256" ]]; then
        echo "$archive does not match the manifest's $target artifact." >&2
        exit 1
      fi
      cp "$archive" "$staged/$version--$target$extension"
    done < <(jq -r '.artifacts | to_entries[] | [.key, .value.url, .value.size, .value.sha256] | @tsv' "$manifest")

    if [[ -n "${RELEASE_TAG:-}" ]]; then
      listed="$(jq -r '.artifacts[].delta.patches[]?.url | sub(".*/"; "")' "$manifest")"
      while read -r name; do
        [[ -z "$name" ]] || grep -qxF "$name" <<<"$listed" \
          || mutate release delete-asset "$RELEASE_TAG" "$name" -y
      done < <(gh release view "$RELEASE_TAG" --json assets \
        --jq '.assets[].name | select(endswith(".capturespatch"))')
    fi

    if ! existing="$(bases)"; then
      mutate release create "$bases_release" --draft --title "Native update bases (internal)" \
        --notes "Full native archives of recent builds, kept so release tooling can make per-file update patches from them. Not for download."
      existing=""
    fi
    shopt -s nullglob
    uploads=("$staged"/*)
    if ((${#uploads[@]})); then
      mutate release upload "$bases_release" --clobber "${uploads[@]}"
    fi
    # Prune against what the release holds after this upload.
    names="$(printf '%s\n' "$existing" "${uploads[@]##*/}" | sort -u)"
    node "$here/native-update-patches.mjs" stale <<<"$names" | while read -r name; do
      [[ -z "$name" ]] || mutate release delete-asset "$bases_release" "$name" -y
    done
    ;;
  *)
    usage
    ;;
esac
