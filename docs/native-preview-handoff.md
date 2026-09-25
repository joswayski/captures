# Native preview handoff — 2026-09-25

## User direction and delivery state

Continue native UI/UX parity with the shipping Tauri app, specifically its mini
previews. **Skip Share/sign-in UI**: the user says it belongs to another PR.
The latest request was to stop, publish the current work and leave this handoff
because usage is running low. Do not infer a deployment or cutover request.

Repository: `joswayski/captures`. Parent conversation:
https://ampcode.com/threads/T-01a0d7a2-f1d8-746f-aede-d19b892a653e

- Brush slice #804 is merged into main.
- Outbound drag #808 is **open**, branch `native/outbound-drag-resume`.
  Its latest fixes gate backend-specific winit methods for Wayland-only builds
  and compare canonical Windows paths in the saved-file fixture. Root Rust CI
  passes on all three OSes and Linux native CI passes; macOS/Windows native jobs
  were still pending at handoff. Recheck rather than assuming completion.
- Current chrome branch: `native/mini-preview-chrome-parity`, stacked on #808
  (base `native/outbound-drag-resume`), **not ready to merge**. After #808 merges,
  retarget this PR to main and inspect the resulting diff. Do not force-push.
- Upload client #807 remains open, with successful Windows CI reported by its
  owner. Server #806 is merged into `accounts-sharing`, **not main**. Broader
  #613 remains a separate runtime prerequisite; do not merge it for preview work.
- No deployment, installed-data migration, stable release or Tauri removal.

## What the chrome slice implements

Both AppKit and wgpu replace permanent title/footer controls with full-bleed
cover images, idle dimensions, hover-revealed Close/Delete/Edit icons and
centered Copy plus Save file/Show in Folder. Saved cards offer Close and Delete;
unsaved Delete only dismisses. Corners and the compact icon toolbar mirror on
right placement. A shared radius token is 12 points. Fixed glass colors remain
independent of light/dark window appearance.

Source ownership:

- `apps/native/wgpu/src/mini_preview.rs`: rendering, controls, input tests.
- `apps/native/wgpu/src/live.rs`: placement wiring only.
- `apps/native/macos/Sources/CapturesNative/MiniPreview.swift`: cover image,
  preview-specific buttons, hover tracking, mirrored placement and saved state.
- `apps/native/macos/Tests/CapturesNativeTests/MiniPreviewTests.swift`: updated
  expectations and asymmetric hover/mirror/save-state fixture.
- `apps/native/x11_preview_smoke.py`: new control positions, idle/hover sampling;
  clipboard poll now ignores the reset owner's plain text until PNG arrives.
- `shared/design.css` and desktop `mini-preview.css`: extract existing 12px
  card radius to a shared token; shipping Tauri geometry is unchanged.

## Resolved: X11 drag regression after Copy

Root cause: the chrome slice only registered the card's drag widget on frames
where the pointer was not over a control. egui hit-tests a press against the
previous frame's widgets, so after the harness clicked Copy (pointer resting on
the button) a move to the image plus press in one frame found no drag widget
and no XDND session started (`Timed out: receiver negotiates COPY`). The drag
area is now registered every frame; only `Action::DragFile` and the grab
cursor/tooltip are suppressed when the press starts on a control. Regression
test: `image_drag_starts_right_after_pointer_rested_on_an_action`. Verified on
private X11: `--drag-only`, `--drag-only --reduced-motion` and `--stack` pass;
the unfixed build reproduces the CI timeout.

The AppKit build also failed to compile: `override` on the panel's
`NSDraggingDestination` methods (fixed on #808 with `@objc`) and a card
property named `rightAnchor` shadowing `NSView.rightAnchor` (renamed
`mirrored`).

Then check AppKit CI compilation/rendered fixtures, Windows native CI and the
new PR's checks. AppKit is uncompiled in the Linux orb. In particular exercise
actual hover and focus traversal, clipped aspect-fill on a non-square image,
saved-state transitions and hidden buttons; do not equate synthetic
`performClick` with real accessibility or keyboard acceptance.

## Verification already run

- `npm run check`: 109 release tests, 870 desktop tests, 36 web tests, plus
  typechecking, lint and production build passed.
- Root `cargo fmt --all -- --check`, `cargo test --workspace` (873 passed,
  9 existing ignored), strict whole-workspace Clippy passed.
- Private wgpu tests: 238 + 1 passed; strict all-target Clippy, fmt and debug
  build passed. Includes idle/focus visibility, disabled-primary contrast,
  image vs button press routing, Save/Reveal busy guards and compact behavior.
- Native Python suite: 16 passed; `git diff --check` passed.
- X11 `--stack` passed all four corners, per-card exact clipboard pixels,
  Copy/Save/Reveal/Edit, disposable Trash failure/retry, focus preservation,
  History preservation, overflow, collapse/fan/move, incoming capture and
  Clear all. This preceded the final busy-label contrast-only correction and
  new idle/hover pixel assertions; rerun combined final coverage.
- Final drag attempt verified idle/hover media contrast and restore before the
  later receiver timeout. Inspected final hover capture: correct control layout
  and dark text on yellow; no old footer or icon/text overlap.
- The shipping Tauri harness was rendered and inspected at DPR 2, idle and
  hovered. No physical platform acceptance is claimed.

## Reproduce locally in an orb

Current worktree: `/home/user/workspace/captures-preview-parity`. Root checkout
`/home/user/workspace/repo` is stale and was not switched. Use a dedicated
worktree from the fetched remote branch in another orb. Local paths/caches do
not transfer between threads.

```sh
node apps/native/prepare.mjs
node apps/native/prepare.mjs --output apps/native/wgpu/resources
CARGO_BUILD_JOBS=2 cargo +1.95.0 test --manifest-path apps/native/wgpu/Cargo.toml
CARGO_BUILD_JOBS=2 cargo +1.95.0 clippy --manifest-path apps/native/wgpu/Cargo.toml --all-targets -- -D warnings
CARGO_BUILD_JOBS=2 cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml
/usr/bin/python3 apps/native/x11_preview_smoke.py --stack \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench --output /tmp/chrome-stack-new
/usr/bin/python3 apps/native/x11_preview_smoke.py --drag-only --reduced-motion \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench --output /tmp/chrome-drag-reduced-new
```

This orb reused `CARGO_TARGET_DIR=/home/user/workspace/captures-brush-resume/target`
for root Rust 1.94 and
`CARGO_TARGET_DIR=/home/user/workspace/captures-brush-resume/apps/native/wgpu/target`
for private Rust 1.95. Limit build jobs to two. `npm run check` needs the
command-local fixture override `GIT_CONFIG_COUNT=1
GIT_CONFIG_KEY_0=commit.gpgsign GIT_CONFIG_VALUE_0=false` here. The temporary
node_modules symlink was removed. Browser and reference dev service were stopped.

Shipping reference: `apps/desktop/ui/src/App.tsx` (`ThumbnailCard`) and
`styles/mini-preview.css`. Run its existing harness at
`?view=thumbnail&mock=1&stage=1&placement=bottom-right`; test both sides and
saved/unsaved cards. Use supervised orb services, `agent-browser`, and inspect
captures. Reference captures in this orb: `/tmp/tauri-preview-idle.png`,
`/tmp/tauri-preview-hover.png`. Final native captures are in the parent thread's
`.amp/in/artifacts/native-preview-chrome-{idle,hover}.png`.

## Continue visual parity after the blocking checks

1. Add matching image hover blur and subtle card/button shadows. Current native
   hover is immediate 50% dimming only. Prefer a cached small preview/filter
   path; do not read back screenshot pixels every frame.
2. Match metadata byte sizes, live clipboard confirmation and Copy visibility,
   and the Edit → In editor / Show in editor presence pill. Bind to actual
   state, not optimistic static labels.
3. Reproduce stationary-pointer hover suppression after arrival/expansion,
   tooltips, control animations and Show less hover morph. Keep focus-visible
   actions usable and test nonactivating-panel keyboard/screen-reader behavior.
4. Continue pile perspective/rotation/scale/blur, stagger, entry/exit dust and
   settle behavior, reduced motion, then cross-display/anchor changes. Follow
   `docs/native-rewrite.md`; stubs and unverified hosts do not close parity gates.

Keep the work cross-platform and in focused PRs. Share/sign-in remains excluded.
