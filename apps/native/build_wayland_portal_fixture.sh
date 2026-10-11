#!/usr/bin/env bash
# Build a SHM-capable wlr portal fixture for a private smoke session only.
# Never install the backend, its D-Bus activation file or its systemd service.
set -euo pipefail

cache="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/captures-native-tools}"
mkdir -p "$cache"
cache="$(realpath "$cache")"
revision=74428f2a8fa7f252e2a46fdf5b697536c66c8a1c
backend="$cache/xdg-desktop-portal-wlr-0.7.1"
if [[ ! -d "$backend" ]]; then
  git clone --quiet --depth 1 --branch v0.7.1 \
    https://github.com/emersion/xdg-desktop-portal-wlr.git "$backend"
fi
[[ "$(git -C "$backend" rev-parse HEAD)" == "$revision" ]]
# Backport the SHM-only format acceptance removed by upstream 21288533.
# Its complete constraints rewrite loops renegotiation before 5598c436;
# 0.8.x also requires DMA-BUF at init (upstream issue #289). Session selection,
# consent and remote grants do not change.
# The exact upstream pin above bounds this zero-context backport.
patch="$(cat <<'PATCH'
diff --git a/src/screencast/screencast.c b/src/screencast/screencast.c
--- a/src/screencast/screencast.c
+++ b/src/screencast/screencast.c
@@ -204,2 +204,2 @@
-	if (cast->screencopy_frame_info[WL_SHM].format == DRM_FORMAT_INVALID ||
-			(cast->ctx->state->screencast_version >= 3 &&
+	if (cast->screencopy_frame_info[WL_SHM].format == DRM_FORMAT_INVALID &&
+			(cast->ctx->state->screencast_version < 3 ||
PATCH
)"
if ! git -C "$backend" apply --unidiff-zero --reverse --check <<< "$patch" 2>/dev/null; then
  git -C "$backend" apply --unidiff-zero --check <<< "$patch"
  git -C "$backend" apply --unidiff-zero <<< "$patch"
fi
# pw_stream_flush synchronously emits STREAMING -> PAUSED and enqueues the
# outstanding buffer, whose VideoTransform still reads cast->target. Keep that
# target alive through stream teardown. ASan confirms the v0.7.1 use-after-free;
# upstream f709e989 avoids it via a larger capture/lifecycle rewrite instead.
patch="$(cat <<'PATCH'
diff --git a/src/screencast/screencast.c b/src/screencast/screencast.c
--- a/src/screencast/screencast.c
+++ b/src/screencast/screencast.c
@@ -95,3 +95,3 @@
-	free(cast->target);
-	wl_list_remove(&cast->link);
-	xdpw_pwr_stream_destroy(cast);
+	wl_list_remove(&cast->link);
+	xdpw_pwr_stream_destroy(cast);
+	free(cast->target);
PATCH
)"
if ! git -C "$backend" apply --unidiff-zero --reverse --check <<< "$patch" 2>/dev/null; then
  git -C "$backend" apply --unidiff-zero --check <<< "$patch"
  git -C "$backend" apply --unidiff-zero <<< "$patch"
fi
if [[ ! -f "$backend/build/meson-private/coredata.dat" ]]; then
  meson setup "$backend/build" "$backend" \
    -Dsystemd=disabled -Dman-pages=disabled -Dsd-bus-provider=libsystemd >&2
fi
ninja -C "$backend/build" >&2
printf '%s\n' "$backend/build/xdg-desktop-portal-wlr"
