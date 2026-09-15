#!/usr/bin/env bash
set -euo pipefail
: "${GITHUB_REPOSITORY:?}" "${GITHUB_SHA:?}" "${NATIVE_TAG:?}"
[[ "$NATIVE_TAG" =~ ^native-v[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]+$ ]]
(cd native-dist && sha256sum --check SHA256SUMS)
assets="$(realpath native-dist)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cat > "$work/notes.md" <<EOF
<!-- native-preview-tag: $NATIVE_TAG -->
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
for asset in Captures-macOS-Apple-Silicon.dmg Captures-Windows-x64-setup.exe Captures-Linux-x64.deb Captures-Linux-x64.tar.gz SHA256SUMS; do
  printf '\n- [%s](https://github.com/%s/releases/download/%s/%s)\n' \
    "$asset" "$GITHUB_REPOSITORY" "$NATIVE_TAG" "$asset" >> "$work/notes.md"
done
# Query failures must stop publication, not be mistaken for a missing channel.
gh api --paginate "repos/$GITHUB_REPOSITORY/releases?per_page=100" | jq -s add > "$work/releases.json"
# Reserve the version before creating a draft. A failed upload then consumes
# this version and the next dispatch chooses a fresh one from fetched tags.
gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
  -f ref="refs/tags/$NATIVE_TAG" -f sha="$GITHUB_SHA" --silent
id="$(gh api --method POST "repos/$GITHUB_REPOSITORY/releases" \
  -f tag_name="$NATIVE_TAG" -f target_commitish="$GITHUB_SHA" \
  -f name="Captures Native Preview — $NATIVE_TAG" -F draft=true -F prerelease=true \
  -f make_latest=false --jq .id)"
node scripts/github-release-assets.mjs sync "$id" native-dist
node scripts/github-release-assets.mjs download "$id" "$work/verify-$id"
(cd "$work/verify-$id" && sha256sum --check "$assets/SHA256SUMS")
gh api --method PATCH "repos/$GITHUB_REPOSITORY/releases/$id" \
  -f body="$(cat "$work/notes.md")" \
  -F draft=false -F prerelease=true -f make_latest=false --silent

# The permanent channel contains only a pointer and links to immutable assets.
# Never sync same-named assets into a public release: GitHub replaces them one
# at a time. A single metadata PATCH switches the whole verified download set.
channel_id="$(jq -r '.[] | select(.tag_name == "native-preview") | .id' "$work/releases.json")"
if [[ -z "$channel_id" ]]; then
  channel_id="$(gh api --method POST "repos/$GITHUB_REPOSITORY/releases" \
    -f tag_name=native-preview -f target_commitish="$GITHUB_SHA" \
    -f name='Captures Native Preview — Latest' -F draft=true -F prerelease=true \
    -f make_latest=false --jq .id)"
fi
gh api --method PATCH "repos/$GITHUB_REPOSITORY/releases/$channel_id" \
  -f body="$(cat "$work/notes.md")" \
  -F draft=false -F prerelease=true -f make_latest=false --silent
