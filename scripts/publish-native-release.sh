#!/usr/bin/env bash
set -euo pipefail
: "${GITHUB_REPOSITORY:?}" "${GITHUB_SHA:?}" "${NATIVE_TAG:?}"
[[ "$NATIVE_TAG" =~ ^native-v[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]+$ ]]
(cd native-dist && sha256sum --check SHA256SUMS)
assets="$(realpath native-dist)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cat > "$work/notes.md" <<EOF
> [!WARNING]
> Experimental native Preview; feature parity with Tauri is incomplete.

Default downloads use Swift/AppKit on macOS, Win32 on Windows, and GTK4 on Linux.
Linux requires X11; the .deb installs distro FFmpeg. The .tar.gz is not a
self-contained AppImage and needs GTK4, glibc 2.39+ and FFmpeg/ffprobe/ffplay.

Install Captures Native Preview separately and quit Tauri before launching it.
Settings/history and OS permissions are separate. Updates are manual; no Tauri
updater payload or automatic profile migration is included.

[Installation and platform limitations](https://github.com/$GITHUB_REPOSITORY/blob/$GITHUB_SHA/README.md)

Built from [source](https://github.com/$GITHUB_REPOSITORY/commit/$GITHUB_SHA).
EOF
# Query failures must stop publication, not be mistaken for a missing channel.
gh api --paginate "repos/$GITHUB_REPOSITORY/releases?per_page=100" | jq -s add > "$work/releases.json"
# Reserve the version before creating a draft. A failed upload then consumes
# this version and the next dispatch chooses a fresh one from fetched tags.
gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
  -f ref="refs/tags/$NATIVE_TAG" -f sha="$GITHUB_SHA" --silent
for tag in "$NATIVE_TAG" native-preview; do
  id="$(jq -r --arg tag "$tag" '.[] | select(.tag_name == $tag) | .id' "$work/releases.json")"
  if [[ -z "$id" ]]; then
    id="$(gh api --method POST "repos/$GITHUB_REPOSITORY/releases" \
      -f tag_name="$tag" -f target_commitish="$GITHUB_SHA" \
      -f name="Captures Native Preview — $tag" -F draft=true -F prerelease=true \
      -f make_latest=false --jq .id)"
  elif [[ "$tag" != native-preview ]]; then
    echo "Refusing to replace an existing dated native release: $tag" >&2
    exit 1
  fi
  node scripts/github-release-assets.mjs sync "$id" native-dist
  node scripts/github-release-assets.mjs download "$id" "$work/verify-$id"
  (cd "$work/verify-$id" && sha256sum --check "$assets/SHA256SUMS")
  gh api --method PATCH "repos/$GITHUB_REPOSITORY/releases/$id" \
    -f target_commitish="$GITHUB_SHA" -f body="$(cat "$work/notes.md")" \
    -F draft=false -F prerelease=true -f make_latest=false --silent
done
