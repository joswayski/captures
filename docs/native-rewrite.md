# Browser-free desktop migration

Status: **native capture/recording workflows implemented; editor work beginning;
cross-platform acceptance and renderer selection still open**.
This rewrite covers macOS, Windows, and Linux, feature by feature rather than one
complete OS at a time. AppKit and the experimental Rust/wgpu host both connect
real capture engines in opt-in development builds; neither replaces the released app.
The shipping Tauri application remains available. No WebView, JavaScript runtime,
localhost server, or Tauri dependency belongs in the replacement. The website is
unaffected.

## Progress dashboard

We are delivering stage 4 workflow slices and starting stage 5 editor slices.
**Implemented is not accepted:** native CI and private-X11/software-rendered tests
do not replace physical macOS/Windows/Linux, accessibility or mixed-DPI checks.
The detailed checklist below remains the release gate; unchecked does not mean
unimplemented. Later slice notes supersede earlier notes about missing behavior.

| Area | Implemented in this tree | Work still open |
| --- | --- | --- |
| Shared core | Settings/migrations, history/artifact lifecycle, capture coordination, recording engines/runtime, screenshot draft storage and document geometry/undo | Remaining editor actions and host bindings; installed-data migration/rollback |
| Capture and History | Region/window/display screenshots, countdown/cancel, seven configurable launch shortcuts, copy/save, counted media filters, clear all, original-recording export/reveal | Full input/coordinate/permission acceptance; large histories and editor reopen/restore |
| Recording workflow | Pause/resume/restart/mute/stop/discard, Hide/Show, passive region guide, screenshots during recording, ready/saved notices | Audio meter/device-change parity, physical recording/audio acceptance, recording editor and transcoded exports |
| Supporting UI | Appearance/preferences, resident tray/menu bar, retained preview stacks, explicit optional feedback | Onboarding, remaining Preferences parity, preview drag/fan/effects, single-instance/relaunch/login items, Open With, crash reporting |
| Editors | Shared v1 draft storage ([#594](https://github.com/joswayski/captures/pull/594)); document geometry/undo with shipping-TypeScript fixtures ([#595](https://github.com/joswayski/captures/pull/595)); real image/annotation rendering and worker-owned draft/edit sessions with retained C-ABI pixel frames; lossless image-transform command; shipping screenshot export encoding policy shared in Rust | Remaining native screenshot host controls/input, annotations/export UI and comparison acceptance, then recording playback/timeline/editing/export |
| Release readiness | Native build/test/fixture jobs on macOS, Windows and Linux; real-media private-X11 exercises | Physical acceptance, accessibility/IME, Wayland live capture, packaging/signing/updater, performance/energy and rollback gates |

The former History and recording/HUD/feedback stacks are integrated through
[#583](https://github.com/joswayski/captures/pull/583),
[#585](https://github.com/joswayski/captures/pull/585),
[#586](https://github.com/joswayski/captures/pull/586) and
[#593](https://github.com/joswayski/captures/pull/593).
[#592](https://github.com/joswayski/captures/pull/592) combines in-recording screenshots
with those flows; its tests retain both screenshot-child and saved-notice coverage.
Superseded parent PRs may be closed rather than separately merged because the
repository uses squash merges. Their functionality must not be counted as missing.

Next implementation boundary: connect shared image transforms to AppKit
and edited-image export controls to AppKit. Shared commands and encoding remain
prerequisites, not native editor/output acceptance.
Native live capture on Wayland remains explicitly
gated; no stub or X11 result closes that platform gate. Merging development slices
does not authorize a native release, renderer cutover or removal of Tauri.

## Inventory and acceptance checklist

Inventory baseline: the desktop command registry in
`apps/desktop/src-tauri/src/lib.rs`, routes in `apps/desktop/ui/src/App.tsx`,
`ScreenshotEditor.tsx`, `Onboarding.tsx`, `Feedback.tsx`, the design harness in
`DEVELOPMENT.md`, and the root README. Checkboxes mean **accepted end to end on all
supported platforms**, not that a mock screen exists. All remain open.

| Gate | Existing behavior and required non-default cases | Source / regression oracle |
| --- | --- | --- |
| [ ] Lifecycle | Background tray/menu bar, relaunch opens Preferences, launch at login, single instance, normal shutdown vs crash recovery, session lock/inactive disables capture | `src-tauri/src/lib.rs`, `state.rs`, `crates/captures-session` |
| [ ] Onboarding | Screen/microphone permissions, deny/retry/restart, optional desktop audio, persisted completion; no permission prompts on fixture launch | `ui/src/Onboarding.tsx`, onboarding commands in `src-tauri/src/lib.rs` |
| [ ] Capture overlay | Region/window/display, empty initial selection, resize/move, aspect constraints, Shift square, Enter/Esc, auto-start, frozen/live preview, repeated shortcut captures Captures UI | `CaptureOverlay.test.tsx`, `App.tsx`, `crates/captures-capture` |
| [ ] Coordinates/color | Mixed DPI, negative display origins, display unplug, window disappears, rounded windows, cursor inclusion, macOS profile → sRGB | capture geometry/model tests; capture commands in `src-tauri/src/lib.rs` |
| [ ] Countdown | Screenshot and recording countdown, cancel while another app has focus, stale session cancellation, target revalidation | screenshot/recording countdown routes; `crates/captures-session` |
| [ ] Recording selector | Screenshot/record switch, targets, audio device choice/unplug, desktop audio and mic, cursor/click highlights, capabilities explained | `RecordingSelector.test.tsx`, `src-tauri/src/recording.rs` |
| [ ] Recording HUD | Running/paused/muted, pause/resume/restart/stop/discard, hidden controls notice, saved notice, region indicator, screenshot during recording | `RecordingHud.test.tsx`, recording routes |
| [ ] Screenshot editor | Text/font/layout/background/shadows, images, shapes/arrows/freehand, rotate/snap, crop/erase/expand, layers/order/lock, duplicate, undo/redo, pan/zoom, proportional resize, off-canvas clipping | `ScreenshotEditor.test.tsx`, `lib/screenshotEditor*.ts`, `imageBackground.ts` |
| [ ] Screenshot output | PNG/JPEG/WebP, maximum/compress and quality presets, size estimate/comparison, alpha flattening, copy/save/overwrite, draft restore/discard | `src-tauri/src/screenshot_editor.rs`, `crates/captures-image` |
| [ ] Recording editor | Playback/seek, timeline thumbnails, trim/crop/resize, audio/quality controls, size estimate/comparison, MP4/GIF/WebM export, cancel/error/retry, recover drafts | `RecordingEditor.test.tsx`, `lib/recordingEditor.ts`, `crates/captures-media` |
| [ ] Mini previews | Copy/save/reveal/trash/dismiss/open, native drag to other apps, internal self-drop shake, clear all preserves history/files, transparent hit regions | `Thumbnail.test.tsx`, `lib/thumbnail*.ts`, `styles/mini-preview.css` |
| [ ] Preview layout/effects | All four corners, pile/fan/expand, move pile, overflow, incoming capture, dust/settle, reduced motion, cancellation mid-effect, monitor/scale changes | same sources; reference PR #529 |
| [ ] Viewer/history | Empty/loading/error, 30-day retention, screenshot/video/GIF filters, restore/delete/clear, drafts, missing files, large virtualized collections | `CaptureHistory.test.tsx`, `src-tauri/src/storage.rs`, `models.rs` |
| [ ] Preferences | System/light/dark, all accent/signal themes and custom colors, settings search, shortcuts/collisions/migration, capture/recording defaults, output directory, updates | `Preferences.test.tsx`, `src-tauri/src/models.rs` |
| [ ] OS integration | Open With all six file types, multiple files, clipboard formats, native drag, reveal/trash/save dialogs, global shortcuts, Escape across focus, OS shortcut takeover | `src-tauri/src` platform adapters; README shortcuts |
| [ ] Notices/updates | Launch caret top/bottom, fixed glass vs solid update surface, full/compact notes, checking/downloading/restarting/error, signed install, drafts survive restart | startup/update routes, `src-tauri/src/updates.rs` |
| [ ] Feedback/privacy | Optional feedback, unavailable/offline/submit error, no captures attached, redacted crash diagnostics, no account needed, no new telemetry | `Feedback.tsx`, `crates/captures-feedback` |
| [ ] Accessibility/input | Keyboard-only every action, focus visible, screen-reader names/roles/value changes, text input/IME, pointer/pinch, reduced motion/contrast, 1×/2×/fractional scale | platform accessibility inspection plus interaction tests |
| [ ] Distribution | macOS signing/notarization/permissions identity, Windows installer/signing, Linux deb/AppImage/desktop entry, updater rollback and storage migration | `docs/releases.md`, existing packaging scripts |

Paths abbreviated above are under `apps/desktop` unless prefixed with `crates`.
Existing platform limitations are not new regressions: Wayland lacks window
targeting/cursor/click highlights and pointer polling; Linux cannot exclude the
recording HUD from captures. Test X11 and Wayland separately. Unsupported actions
must be explicit, not silently successful.

## Architecture and ownership

- **Rust owns domain state:** capture/recording sessions, settings migrations,
  history/artifact lifecycle, editor documents/undo, media jobs/cancellation and
  capability checks. Reuse the existing capture, image, recording, media, session,
  video and feedback crates. Do not rewrite their engines in Swift.
- **Native hosts own OS/UI:** windows, input, accessibility, text/IME, clipboard,
  drag/drop, dialogs, shortcut registration, tray and render resources. macOS starts
  with Swift/AppKit and Core Animation; use Core Image on Metal for image effects.
- Extract `models.rs` / `storage.rs` and orchestration out of `AppHandle`-coupled
  modules incrementally. Keep Tauri as an adapter to the same Rust core until
  cutover. Port pure TypeScript editor behavior with saved fixtures from its tests;
  do not embed a JS engine to reuse it.
- Proposed first binding is a narrow versioned C ABI around an in-process Rust
  static library. Specify opaque handle ownership, buffer release, error codes,
  cancellation and main-thread delivery before implementing it. No per-frame JSON
  or full-frame base64. UI receives immutable snapshots/events; media stays in
  owned native buffers/files. Benchmark copies before selecting a GPU-sharing ABI.
- `shared/design.css` and `shared/themes.css` remain the token source during
  migration. The workbench compiles resolved token resources at build time; it
  does not parse CSS or run a browser at runtime. Share assets and golden fixtures.
  Platform components may differ internally but must meet the same appearance,
  input and accessibility contracts.
- Default workbench scenes use synthetic capture fixtures; `--live` explicitly
  enables the current native capture slice. Both use separate development data,
  never installed settings/history. Fixture launches do not request capture
  access. Live captures register temporary global Escape for cancellation and
  persisted New Capture and region/window/display screenshot/recording launch
  shortcuts. OS shortcut takeover and update installation are not connected yet.
  Production data migration requires backup, version checks and rollback tests.

## Reviewable stages and exit gates

The unit of delivery is a **cross-platform feature slice**, not a finished macOS
app followed by ports. Implement domain behavior once in Rust; implement its
presentation and OS adapters on each platform. Shared behavior does not require
identical component implementations or a common UI framework.

1. **Inventory + AppKit reference (merged in [#531](https://github.com/joswayski/captures/pull/531)).**
   Shared tokens and fixture preferences/history/HUD/preview screens exist. The
   workbench runs on the maintainer's Mac and native CI passes; full visual and
   resource acceptance remains open. Mock screens are not feature parity.
2. **Cross-platform foundations (implemented; acceptance open).** Bring Windows and Linux renderer
   prototypes alongside AppKit using the same fixture scenarios below. Compare
   candidates before selecting production renderers. Make resources and scenario
   expectations platform-independent; keep backend measurement adapters separate.
   Shared-core extraction can proceed in parallel, preserving the legacy host's
   behavior, but do not build a backlog of Mac-only production features while
   other hosts lack the ability to render and exercise them.
3. **First shared feature slices.** Start with persisted appearance/preferences
   and host lifecycle, then display screenshot → preview → copy/save → history.
   Add region selection as its own slice. Each slice includes the shared Rust
   contract, all three native hosts, real engine integration, and platform checks.
   Test ownership, permission denial, errors, cancellation, session lock and DPI
   where relevant. A display-capture slice does not close the whole capture gate.
4. **Remaining workflow slices.** Work through onboarding, shortcuts, remaining
   capture modes/countdowns, preview pile/drag/effects, recording selector/HUD,
   history operations and notices. Close one narrowly defined behavior across
   platforms before treating it as complete; use the inventory for full coverage.
5. **Editor slices.** Port shared document math/persistence first, then editing
   actions and their native presentation across platforms. Start with screenshot
   editing, then recording playback/export. Differential fixtures, crash recovery
   and real media outputs gate each slice, not a mock editor shell.
6. **Cross-platform release cutover.** Packaging, updater, accessibility, energy
   and long-run tests, storage rollback, signed Preview testing. Only after parity
   is accepted remove Tauri/React desktop dependencies and the legacy frontend.
   No automatic stable release or installer replacement; the website may use React.

### PR size and platform acceptance

A small slice can fit in one PR covering all platforms. Larger slices may use a
behavior-preserving Rust extraction PR followed by focused host PRs for that same
slice. Do not duplicate domain logic in Swift or platform UI code to make one
host advance faster. Do not force unrelated OS changes into a shared-core-only PR.

Each implementation PR records the slice's behavior/non-default cases and status
for **macOS, Windows, Linux X11, and Linux Wayland**. Use explicit states:
`not implemented`, `implemented / unverified`, `verified` (with evidence), or
`unsupported` (with the existing capability limitation and visible fallback).
Mocks, stubs, compilation, and missing hardware are not functional acceptance.
When host work is split across PRs, link the companion work and keep the slice
open until its platform gates pass. Never silently drop an OS to close a gate.

Run platform compilation/tests in CI where available. Maintainer runs on Mac and
Windows supply real desktop/input/GPU evidence; Linux evidence must distinguish
X11 from Wayland and hardware from the orb's graphics environment. Each handoff
includes exact commands, expected behavior, captures and raw measurement output.
Hardware results pending need not block unrelated shared work, but must remain
visible and cannot justify a renderer selection or performance claim.

## Windows and Linux evaluation plan

Native AppKit and wgpu Preferences now connect explicit, optional feedback through
`captures-feedback`. The form displays its app/system context before Send, permits
an optional contact, blocks duplicate submissions, and retains drafts after errors
or closing/reopening. Submission runs separately from capture/settings workers;
fixtures cannot send. No captures, files, or crash diagnostics are attached and
no startup network request is introduced. This advances the manual feedback slice,
not automatic crash reporting or full accessibility/physical-platform acceptance.

No renderer is selected for these platforms yet. The same fixture scenes, token
resources, resource budgets, visual checkpoints and input scripts are mandatory.

The first domain slice now extracts shipping settings types/defaults/migrations
and persistence into `captures-settings`, with a versioned `captures-settings-ffi`
static library for AppKit. Both native Preferences screens edit a separate
development settings file. Shared custom-theme derivation is checked against
TypeScript-generated golden values. This advances settings persistence and
presentation, not lifecycle/capture integration or full Preferences acceptance;
all checklist gates above remain open until end-to-end verification.

The next shared-core slice moves history metadata, 30-day retention, atomic
artifact replacement, recording recovery, and basic sRGB PNG/thumbnail encoding
into `captures-history`. The shipping desktop delegates to it; callers provide
their own history root and presentation URLs. Native capture integration can use
the same lifecycle without accessing installed history. This extraction alone
adds no native capture UI and closes no platform gate.

Screenshot editor draft storage now also lives in `captures-history::editor_draft`.
The shipping Tauri commands delegate save/load/discard and asset reads to this
shared module, supplying their existing directory and protocol URLs. The v1
manifest, opaque document JSON, incremental PNG assets, limits and broken-draft
cleanup policy are unchanged. Callers can use isolated native development roots;
no installed-data migration occurs. Each file is replaced atomically, but the
whole draft is not a transaction: the existing prune/assets/manifest write order
is preserved. Portable filesystem tests cover compatibility, byte preservation,
error paths and root isolation. This is an editor persistence prerequisite, not
a native document model or editor UI. Native editor recovery acceptance remains
open on macOS, Windows, X11 and Wayland; no platform parity gate is closed.

Recording platform dispatch, microphone enumeration and capability/exclusion
policy now live in `captures-recording-platform`; the shipping host delegates to the
same macOS ScreenCaptureKit and Windows/Linux xcap engines. Hosts still own
permissions, worker scheduling, window exclusion, recording lifecycle and media
finalization. This behavior-preserving extraction does not connect the native
Record button or close a recording acceptance gate.

Native recording microphone mute now shares one `RecordingSession` operation
across AppKit and wgpu. Running changes durably complete the accepted segment,
persist only `audio.microphone_muted`, then reopen with the same target/options;
paused changes stay paused, unchanged values do not rotate, and stale generation,
invalid-state, missing-device and reopen-failure paths preserve recovery media.
Both 430×102 HUDs expose Mute/Unmute names, selected muted state, lifecycle busy
gating and an explicit mic-less explanation. Status: macOS AppKit and Windows are
implemented / unverified on physical hosts; Linux X11 is verified on the private
software-rendered Xvfb desktop with a disposable PulseAudio null-sink microphone;
Wayland remains gated with native live capture. A synthetic tone verifies decoded
audible/silent/audible intervals across mute/unmute, not physical microphone
fidelity or gapless device/encoder transitions.

The opt-in `--live` workspace now connects full-display PNG capture and local
screenshot history on both native hosts through `captures-app`. It includes
explicit copy, export, reveal and history deletion while keeping exports and the
installed Preview's data separate. Image decode and capture/file operations run
off the UI thread. It preserves permission/session checks and hides its window
before capture. Wayland capture remains gated by the candidate's missing window
visibility support. Automatic copy and output folder/format preferences are now
connected; JPEG/WebP encoding is shared with the legacy host and history remains
lossless PNG. Screenshot countdown and temporary global Escape now share Rust
deadlines, generation invalidation, and a cancellation/commit boundary across
hosts; native countdown windows use the fixed media palette. Real mixed-DPI,
focus, compositor, accessibility, and animation acceptance remains open.
Cursor inclusion now shares sampling/compositing with the shipping host (macOS
system pixels, Windows/X11 synthetic arrow). Full region/window parity, recording, editor,
preview-stack interactions and full UI/UX parity are still open; this slice closes no complete
platform acceptance row. Hardware capture and clipboard tests remain required.

Both hosts now connect retained screenshot mini-preview stacks, backed by shared
Rust membership, layout and visibility policy. Copy uses full pixels; Save reads current output
preferences; History/Open restores the workspace; Dismiss preserves history and
exports. Show less/expand preserves capture order, overflow scrolls without a
count cap, and Clear all dismisses only snapshotted IDs, not later captures.
The four corner placements use actual monitor work areas. Private-X11
tests exercise placement, focus, minimized-root actions, exact capture inclusion/
exclusion and cancellation. AppKit tests cover panel/decode/action
lifecycles and fixed-glass rendering. Windows runtime, physical macOS, mixed-DPI,
screen-reader and compositor acceptance remain open; Wayland stays unsupported.
Drag, hover-fan/transition animation, transparent hit regions and dust remain
future slices. Static piles and scroll controls do not close the effects gate.

The resident lifecycle slice adds live-only menu-bar/tray actions and three
persisted screenshot shortcuts. One Rust dispatcher owns capture-launch and
temporary Escape delivery; native event loops drain queued actions. Focused
Preferences and capture preparation suppress launch keys. Hidden capture restores
hidden state; explicit Quit drains accepted work and drops shortcuts/tray.
Linux uses session D-Bus SNI/KSNI, requires a registered host before close-to-hide,
and recovers a hidden root when the host disappears. No watcher/host gives a
visible close-to-quit fallback. The private-X11 `--lifecycle` test uses real Xfce
SNI/DBusMenu and global input, not fake tray dispatch. AppKit has native menu,
focus, restoration and ordered-cleanup tests. Windows compilation/fixtures do
not replace real tray/input testing; physical Mac, Windows, Wayland, mixed-DPI
and accessibility acceptance remain open. Single-instance relaunch, login items,
OS shortcut takeover and the other lifecycle checklist requirements remain open.

The shortcut-editor slice adds all seven Preferences recorder rows to both hosts.
Rust owns modifier/key policy, cancellation, display tokens and persisted-field
validation, checked against 585 shipping TypeScript recording/display vectors.
AppKit intercepts focused recorder events before menu equivalents; wgpu observes
root winit physical keys before egui loses PrintScreen, keypad or Super identity.
Focused Preferences temporarily releases screenshot OS grabs, retaining desired
bindings and restoring the latest saved mapping on blur. Registration failures
leave capture routing suspended and report an error. Recording bindings remain
storage-only. The private-X11 `--lifecycle --shortcut-editing`
test covers real input, collision rejection, persistence and global reactivation;
AppKit XCTest covers controller/bridge semantics and both-appearance renders.
Physical Mac external/media keys, Windows real input, Wayland and screen-reader
acceptance remain open; this does not close the full Preferences/input gate.

The recording-shortcut follow-up connects all seven saved bindings to the shared
dispatcher and both hosts. Idle recording keys open the existing selector in
Record mode at the requested target. Within the selector, screenshot/recording
keys switch mode and target without replacing the flow, discarding the settled
region, or starting capture. Preparation, countdown and active recording remain
blocked; focused Preferences releases all seven OS grabs. This does not add
recording control keys or close physical platform/input acceptance gates.

The native recording Restart slice replaces the current running or paused take
inside `captures-recording-platform`, retaining its target/options while deleting
only that recovery bundle's active and completed segments and resetting elapsed
time. AppKit and wgpu require confirmation, rearm global Escape on the accepted
flow generation, run the stored countdown, and preserve stale-start checks before
and after replacement-engine opening. Countdown cancellation discards the replaced
session. Private X11 exercises running/paused restart and replacement-only decoded
pixels; AppKit and Windows remain implemented but require native CI/hardware, and
Wayland remains gated by the existing native recording limitation. This does not
close the Recording HUD gate: physical accessibility/compositor acceptance remains
open; the later Screenshot and saved-notice slices below supply those controls.

The recording-ready notice slice connects successful finalization to a fixed-glass,
nonactivating top-right notice in both native hosts. Save file reuses the shared
original-recording export operation; saved state offers Show in Folder. Pending
saves pause the 15.2-second expiry; failure keeps retry available. Dismiss, expiry
and new capture only remove presentation, and stale callbacks cannot revive it.
Native has no recording editor yet, so the trigger is finalization, not the
shipping editor-close event. Private-X11 input tests exercise export byte equality,
failure/retry, missing exports, intercepted OS-reveal arguments, hidden-root expiry,
dismissal and capture cleanup; AppKit provides state and render fixtures. Physical
macOS/Windows, Wayland, accessibility and motion parity remain open.

Native region recordings now retain a passive display-local guide from countdown
until finalization/discard/cancellation. AppKit and wgpu paint the fixed glass veil
and accent border strictly outside an outward-pixel-rounded transparent hole;
no centered stroke or antialias fringe enters recorded pixels. The guide does not
take focus or pointer input, survives pause/restart/hidden controls, and is absent
for window/display targets. Private-X11 checks cover the input shape, composited
inner-edge pixels, decoded MP4 corners, Hide/restore, and cleanup. AppKit has
alpha-channel render tests; physical macOS/Windows, fractional-DPI compositor and
multi-display acceptance remain open. Wayland remains gated.

The recording Hide slice keeps the accepted AppKit/wgpu session and capture-flow
generation alive while removing only its HUD. A 6.2-second click-through fixed-glass
notice replaces no controls. Menu bar/tray actions, app reactivation and a restore-only
New Capture shortcut bring the HUD back without enabling any other busy shortcut.
Stop, Discard, Restart/countdown, session loss and teardown clear hidden state; generation
checks reject stale restoration. Linux requires a live SNI host and restores the HUD plus
workspace on host loss. Windows/AppKit physical acceptance remains open and Wayland stays gated.

The recording Screenshot slice gives an accepted recording a temporary child
capture generation instead of replacing or reopening its disarmed parent. The child
owns region selection, screenshot countdown, Escape and one persistence commit;
cancel, stale replies and cleanup cannot cancel or commit the recording generation.
AppKit and wgpu reuse the existing region capture, native History, mini-preview and
auto-copy paths while preserving running/paused, microphone, guide and hidden-control
state. AppKit and Windows use their capture-UI exclusion policy. X11 hides the HUD
and guide from the still image, but its selector remains visible in the ongoing
recording because X11 cannot exclude overlay windows. Private-X11 acceptance covers
running publication, paused countdown cancellation, selection Escape, asymmetric
saved pixels, same-session continuity, final decode and recovery cleanup. AppKit CI
renders/tests the enabled HUD; real macOS/Windows capture and Wayland remain open,
so this does not close the Recording HUD parity gate.

The screenshot-editor shared-core prerequisite models the persisted layered
document separately from the bitmap renderer and ports initialization, bounded
crop, translation, canvas sizing, lossless D4 image orientation and 100-snapshot
undo/redo semantics. TypeScript-generated vectors cover fractional/off-canvas
geometry, hidden and locked layers, every orientation and history branching.
Unknown document fields survive native operations, remaining compatible with the
opaque version-1 draft manifest. This prerequisite alone does not close a native
editor acceptance gate; the first connected host slice is recorded below.

The first shared editor-rendering unit converts visible image layers into the
existing `captures-image` compositor using caller-supplied in-memory assets. It
retains canvas background/alpha, clipping, order, opacity, six blend modes,
lossless D4 bitmap orientation and arbitrary layer rotation while explicitly
rejecting unsupported visible annotation layers and invalid or oversized inputs.
It performs no host I/O; host sessions supply the decoded assets.

The closed-shape renderer follow-up adds rectangle, ellipse, triangle, diamond
and star layers in shared stack order with shipping drag-box geometry, rounded
rectangle corners, star proportions, authored rotation origin, fill/stroke
defaults, opacity and blending. At that checkpoint, visible text, line/arrow,
freehand and drop-shadow content was an explicit rendering error rather than
disappearing. This is shared rendering support, not native drawing-tool presentation
or full acceptance.

The open-stroke renderer follow-up matches shipping straight, quadratic and
multi-control lines, filled tapered arrows and midpoint-smoothed freehand paths.
Shared rendering preserves their authored rotation origins, round line/freehand
strokes, mitered tapered-arrow outlines, opacity, blending, clipping and layer order.
Visible text and enabled annotation shadows remain explicit errors. This remains
host-independent preparation only; host drawing-tool presentation and physical
acceptance are not part of this slice.

The shared editor-session boundary now opens isolated History screenshots and
version-1 drafts, owns decoded image assets and snapshot history, and validates
and renders edits before replacing the current state. Crop/canvas sizing,
document commits and undo/redo retain previous frames safely through `Arc`;
the versioned C ABI exposes independently retained frames without JSON pixels.
Draft save/discard are explicit worker operations; save failures do not mark
edits persisted, and discard does not remove a draft if its original cannot be
reopened. Existing draft storage is atomic per file, not a multi-file transaction.
Image input bytes/dimensions and the aggregate decoded asset pixels are bounded;
drafts cannot load arbitrary filesystem/network image sources. Visible unsupported
annotations remain errors rather than silently missing output. This is the same
host-independent implementation for macOS, Windows, X11 and Wayland.

Editor sessions can encode the current edited frame through the shared PNG/JPEG/
WebP quality and hard-byte-budget policy. The C ABI returns independently owned
encoded bytes, borrowed through an explicit pointer/length view and released
separately from the session. Options and result metadata use JSON; image bytes
never do. Encoding success or failure leaves document, undo/redo, draft dirty
state and original History files unchanged. Shared Rust can also publish a new
edited-file copy without clobbering an existing destination, then add a distinct
lossless History artifact; a post-publication History failure retains the saved
path for recovery. `captures_editor_save_new_v1` exposes this on the serialized
session worker with tagged result JSON and no pixel transport; null/invalid
inputs, collisions and partial success are covered without changing draft state.
Hosts still own save dialogs, overwrite-original confirmation
and clipboard behavior. These shared prerequisites are unit-verified in the Linux
orb; they do not connect native export controls or complete macOS, Windows, X11
or Wayland output/physical acceptance.

Shared layer commands now cover visibility, locking, opacity, movement, deletion,
duplication, image renaming, ordering and the four lossless image transforms through
the same transactional session and C ABI. Shipping TypeScript fixtures check all
four duplicate element kinds, reorder placements across locked boundaries and D4
orientation composition. Hidden and locked images remain transformable; transform
requests for non-image layers are no-ops, matching the shipping editor. Locked
layers otherwise block movement/deletion/reordering but permit the other panel
actions. A sole visible full-canvas image rotates its canvas, ordinary layered
overhang remains clipped and a fully off-canvas result expands the document.
Duplicates share owned image assets and remain draft-compatible.
These commands are shared across all four platforms; host integration and
physical acceptance are tracked separately below.

The first wgpu editor window now opens isolated History screenshots on its own
serialized worker, with fit preview, numeric crop/canvas fields, undo/redo,
save draft and confirmed discard. Closing unsaved edits offers save, keep the last
persisted draft without saving the new edits, or cancel. Normal quit drains queued
edits and saves dirty sessions; failure cancels quit and focuses the recoverable
editor. The original History PNG and exports remain unchanged. Live workspace and
editor windows use persisted appearance without first visiting Preferences.
`apps/native/x11_editor_smoke.py` checks real input, asymmetric crop/resize pixels,
draft geometry/reopen, prior-draft preservation, discard, failed save/quit and
successful quit retry in dark and light. Minimum-size error/scroll states are
visually inspected. Unit tests cover queued edits and stale replies during close.
Status: X11 verified on private software GL; Windows and Wayland use the same
implemented host but remain presentation-unverified.

The wgpu editor's Layers panel now exposes those shared commands with stable-ID
selection, safe long-name truncation and independent geometry/layer scrolling.
Undo, deletion and rejected commands restore valid selection and field state.
Real X11 input checks cover asymmetric movement, half-opacity/hidden preview
pixels, locks, ordering, deletion, empty-document undo and saved-layer reopening
in dark and light, including minimum-window scrolling. Windows and Wayland use
this implementation but remain presentation-unverified; AppKit layer controls
are in progress. Image layers expose a **Transform image** menu for lossless
left/right rotation and horizontal/vertical flips through the shared worker commands.
Hidden and locked images can transform, matching shipping policy; full-canvas
photos rotate their canvas, and undo/draft restore retain the orientation.
Merge/flatten, image import and drawing tools are not connected by this panel slice.

The wgpu Output panel now previews shared PNG/JPEG/WebP encoding with the shipping
quality modes, palette controls and hard byte budget. Encoding and decoding run
on the editor worker; the UI reports actual encoded bytes and switches between
the edited canvas and decoded output. Edits and option changes invalidate the
previous comparison; encoding failures retain recoverable edits and allow retry.
Preview never writes files or saves a draft. The same Windows/X11/Wayland host
code is implemented; private-X11 and unit checks do not establish physical-host
acceptance. Its **Save new copy** action runs shared publication on the same worker,
starts in the configured output directory and accepts an editable full path.
It never replaces existing files; successful exports add a distinct History entry
without modifying the original or draft. A post-publication History failure shows
the saved path and warning. Accepted writes drain before application quit.
Native save dialogs, overwrite-original and clipboard remain separate work, as
do AppKit export controls and physical-platform acceptance.

The AppKit editor host now enables **Edit screenshot** only for screenshot History
entries. Its dedicated serialized worker owns the shared Rust session and publishes
independently retained RGBA frames to a fit preview. The window exposes crop geometry,
canvas sizing, Undo/Redo, explicit draft save and confirmed draft discard; shared Rust
remains the only geometry/render authority. Drafts use the same isolated sibling root
and reopen with the screenshot. Closing an unsaved session offers save-and-close,
close without saving the current session (retaining any older persisted draft), or
cancel. Quit drains accepted work and cancels termination if its draft save fails.
AppKit CI covers bridge lifetime, pending-edit ownership, locale-aware geometry,
failure/close behavior and rendered light/dark fixtures. Physical AppKit input,
accessibility and IME acceptance remain unverified.

Across both hosts, physical input/accessibility/IME acceptance remains open.
AppKit image-transform controls, annotation tools, AppKit edited-image export and recording
editing are not connected; the screenshot-editor parity gate stays open.

New Capture connects its persisted shortcut, tray action and workspace entry to
fixed-glass screenshot controls on both hosts. Region, Window and Full screen
share one prepared Rust session and desktop snapshot, retaining selections across
target switches. A display replacement invalidates stale preparation and local
selections. Region confirmation reuses the existing audited crop/cursor policy;
countdown refresh and the cancellation/commit boundary are unchanged. The
controls include aspect selection, Enter/Escape and auto-start behavior, while
Record remains explicitly disabled. Existing direct screenshot paths remain
available. Global region/window/display keys now switch targets inside the open
menu under its exact capture generation, without a new session or keyboard
Full screen auto-start. Leaving selection clears held/pending target keys before
preparation or countdown. Recording, physical
platform input/display acceptance and full capture-menu visual/accessibility
parity remain open.

Region preparation starts with `captures-app::selection`: shared create/move/
corner-resize and settled-aspect geometry, including Shift precedence, fractional
coordinates and the shipping minimum/clamping rules. A checked, allocation-free
C ABI exposes the same functions to AppKit without per-pointer-event JSON.
176 differential vectors execute the shipping TypeScript oracle; Rust compares
both drag and settled-aspect outputs. Regenerate intentionally with
`node scripts/native-selection.test.mjs --write`; the normal repository gate
rejects stale vectors. `captures-app::region` now owns a bounded frozen-frame
session, or a live-desktop session without a retained frame. Confirmation uses
fresh pixels/cursor after any countdown, rejects changed display geometry/scale,
crops using actual buffer edges, and shares the cancellation/commit gate before
persisting region history. Cursor compositing happens after cropping so an
outside hotspot cannot leave a clipped arrow fragment. Its opaque C ABI lends
read-only pixels without a full-desktop temporary file or JSON image transfer;
hosts must retain the session until every image provider and worker has finished.
Rust pixel/source-selection tests and Swift ABI tests cover these contracts.
The AppKit host now connects a native region panel with draw/move/corner resize,
all six aspect presets, Shift-square override, Enter/Cancel and automatic start.
Its layer-backed frozen image stays separate from the input-driven scrim canvas
and native controls. Preparation/selection keep the same Escape generation; the
countdown starts only after confirmation. The Windows/X11 candidate connects the
same shared session and selection geometry, with a private-X11 repeated-capture
pixel/persistence gate. Wayland's host visibility/placement gate remains open.
Neither this stage nor its synthetic input/render checks close the
capture-overlay gate: real display/permission/session/VoiceOver verification,
magnifier, blur and full capture-menu UI parity remain required.

Window capture begins with a behavior-preserving extraction of pixel-source
policy into `captures-capture`. The shipping host uses the shared stack-occlusion
check, composited-crop/native fallback and blank-frame heuristic. A failed native
capture must never fall back to pixels from a covering window; known same-app
untitled transients retain the existing exception. Error messages/categories and
the existing shipping tests are preserved. Native-coordinate buffer scaling,
clipped window rectangles, freeze-frame corner-radius inference and antialiased
macOS corner masking now use the same shared algorithms. The host still supplies
the macOS fallback radius and decides where that mask applies; Windows and Linux
do not gain rounded masks. This extraction alone implements no new native capture
mode on any OS.

A follow-up shared-core stage moves window target classification into
`captures-capture`: display membership, empty/minimum-size filtering, Captures'
internal surfaces, shell edge strips, desktop backdrops and excluded system apps
now produce shared capturable/shell-chrome groups. The macOS Screenshot and
Windows NVIDIA overlay exclusions retain their compile-time platform gates. The
shipping host still owns enumeration failures/logging and applies snapshot chrome
refinement after classification.

`captures-app::window::WindowSession` now owns native preparation, frozen pixels,
target descriptors and window/display confirmation. Its versioned C ABI exposes
the same session to AppKit, including borrowed RGBA storage with the region
session's lifetime contract. Hosts supply the OS corner-radius fallback, hide and
settle their windows before capture, and retain the event-loop capture-flow guard.
No pixels enter JSON or temporary preview files. Safe frozen crops keep their
original pixels/cursor; countdowns always refresh pixels, window geometry and
cursor. Unsafe/blank crops use the shared native-surface fallback, never a crop of
an occluding window. Display/shell selections persist display-mode history;
window selections persist window-mode history. Cancellation and session checks
surround capture and use the existing commit gate before persistence.

The native session deliberately fails closed when live target enumeration fails,
the target disappears or moves to another display, or display geometry changes.
It retains the **unfiltered** stack for occlusion checks: a window too small to
pick may still cover the selected target. These are stricter than the legacy
host's stale-descriptor fallback and filtered frozen stack; legacy behavior is
unchanged. Unit tests distinguish frozen/fresh geometry and pixels, small
occluders, source failures, cursor spaces, output metadata and macOS-only masks.
Native window selection now connects that session to AppKit and the Windows/X11
wgpu candidate: hover bounds/name, click selection, Enter/Capture, automatic start,
freeze/live selection and countdown. Both retain the flow/session through worker
completion and close native selectors before capturing. Wayland window targeting
remains unsupported. Shared-core tests and ABI compilation do not close the
cross-platform capture gate or select a Windows/Linux renderer.

Window-slice acceptance remains explicit: macOS and Windows are implemented but
real-desktop behavior is unverified; Linux X11 has private-Xvfb pixel/persistence
and injected-input coverage, not hardware acceptance; Wayland is unsupported.
The X11 test checks repeated asymmetric window pixels, frozen/fresh countdown,
live selection, native-surface fallback under occlusion, disappearing targets,
desktop fallback, manual versus automatic confirmation, cross-app Escape and
simulated session cancellation. Real permissions, multiple displays/DPI, system
lock, accessibility and compositor/GPU behavior remain open.

The window session also owns pointer hit testing: native z-order, stable equal-
level ordering, half-open edges, negative origins and Windows DIP conversion.
Shell chrome and empty desktop hits select the display. The allocation-free C ABI
returns a prepared-window index, not JSON on every pointer event. 120 differential
vectors execute the shipping TypeScript picker; regenerate intentionally with
`node scripts/native-window-hit.test.mjs --write`. The normal gate rejects stale
fixtures. Hosts still own drawing, focus and confirmation.

The macOS version-to-window-radius fallback also lives in `captures-capture`,
shared by the shipping adapter and allocation-free native C ABI. AppKit supplies
its OS major version; Rust owns the pre-26/26+ policy. Native hosts do not import
the Tauri-dependent `captures-macos-window` adapter.

The first [shared wgpu candidate](../apps/native/wgpu/README.md) uses egui/winit
with retained image textures and event-driven immediate-mode UI, an additional
approach to evaluate against the retained/native candidates below. It implements
fixture preferences/history/HUD, a fade/settle probe, editor rendering and hidden
idle. It does **not** implement dust parity, production domain logic or complete
accessibility/IME/overlay behavior. No performance win or renderer decision follows
from its existence. Compare equivalent workloads only; the AppKit dust and wgpu
fade/settle probes are deliberately not equivalent effects.

The next implementation milestone is **comparable workbenches on all OSes**, not
the next Mac-only screen. Exercise each candidate with:

- Preferences: Captures-styled controls, light/dark/themes, editable search text,
  keyboard focus and scrolling; expose accessibility roles and values.
- History: empty, 100 and 1,000 image-backed rows, filtering/scrolling, bounded
  thumbnail residency and release after closing.
- A transparent desktop preview and HUD: hover/hit regions, running/paused/hidden
  states, cold/warm dust and survivor settle, cancellation and reduced motion.
- An editor rendering probe: large image, multiple layers, pan/zoom/rotate and
  editable text/IME. This tests renderer suitability, not editor feature parity.
- Lifecycle: hidden/minimized/occluded idle, mixed/fractional DPI, repeated
  open/close and resource recovery. No recurring redraw loop for static scenes.

Extend the AppKit workbench to these same cases where it is incomplete. Reuse
tokens/assets and deterministic effect fixtures; do not translate the entire app
into each candidate just to evaluate it. Preserve custom Captures styling and
compare equivalent work, including setup costs, rather than native stock widgets
against fully styled screens. Record missing input/accessibility support as a
candidate cost, not as a task deferred until after renderer selection.

| Platform | Candidates | Questions the prototype must settle |
| --- | --- | --- |
| Windows 11 | Win32 host + DirectComposition/Direct2D/DirectWrite; shared Rust retained scene renderer using wgpu + native text/accessibility adapters | Idle wakeups, GPU allocations, composition-only motion vs texture uploads, custom controls, UI Automation/IME, transparent click-through windows, mixed DPI, device loss |
| Linux X11 + Wayland | GTK4 host/custom snapshot nodes (not default widget styling); shared Rust wgpu renderer with winit/Wayland/X11 host and AccessKit | Fractional scale, text quality/IME, AT-SPI, transparent overlays, compositor/frame callbacks, portal permissions, tray support, occlusion, integrated GPU/software fallback |

GPUI remains a comparator only: its tested CPU-raster/texture-upload dust path was
expensive; that result does not disqualify every GPU implementation of the effect.
Use ordinary custom Captures controls, not stock GTK/WinUI visual styling. Prefer
shared components only after representative screens meet parity and resource
gates. If one scene needs platform rendering, isolate that component rather than
forcing all screens into the same renderer. Record toolchain, dependency/license,
binary size and accessibility cost with the performance decision.

## Measurement protocol

The prior [AppKit experiment](https://github.com/joswayski/captures/pull/529)
passed 24/24 dust/settle checkpoints. Its AC-run dust medians were 54 MiB / 8.0%
of one CPU core / 97.5 observed changed frames/s, versus 196 MiB / 27.5% / 40.9 for
Tauri. AppKit preparation was about 132 ms. These are **component results**, not
whole-app budgets, GPU presentation timestamps, or proof this workbench is faster.

Run release builds, same machine, power mode, scale and refresh rate. Separate
resource trials from screenshots/frame observation. One discarded warmup and at
least three measured trials, rotating renderer order. Keep raw results and build
identity. Compare equivalent content and functionality; a mock screen versus a
full application is not a valid whole-app performance comparison.

| Workload | Required observations |
| --- | --- |
| Cold launch → first usable screen | Process-tree physical footprint, startup latency, first input latency |
| No windows; Preferences idle; HUD idle; history idle | 60-second CPU time, wakeups, memory; no continuous display link or polling for static screens |
| Preferences | Search/type, focus, appearance/theme switching, scroll; input p50/p95, text and accessibility parity |
| History | Empty / 100 / 1,000 entries, scroll/filter/open; image cache residency and eviction |
| Editor | Large image + multiple layers, pan/zoom/rotate, text input; recording playback/seek/export in later stages |
| Transparent preview | Hover controls, pile expand/move, new captures, self-drop shake, cold/new-image dust, warm repeat dust, survivor settle, reduced motion |
| Recording | Running/paused/hidden HUD and indicator, mic changes, real recording overhead separated from UI |
| Lifecycle | Hidden/minimized/occluded, screen sleep/wake, 1×/2× display changes, 30-minute churn, memory after close/GC/resource release |

Measure preparation in distinct phases: decode, cover crop, atlas filtering,
layer construction, animation submission, and total first-action latency. Moving
work before a timer or prewarming does not remove its cost. The workbench batches
isolated padded chips into one filter atlas to reduce repeated Core Image renders;
this is an **unverified optimization candidate** until the original visual gate
passes and cold setup improves. Do not blur a whole source image and then slice:
that changes chip edges. Do not hide preparation in an unbounded cache.

Acceptance targets to validate on hardware: no application-owned recurring frame
callbacks when static/hidden; UI-only idle CPU median below 0.5% of one core;
p95 ordinary input response below 50 ms; animation work within the display budget
(16.7 ms at 60 Hz / 8.3 ms at 120 Hz); no sustained memory growth after repeated
open/close cycles. Cold dust preparation target is under 16.7 ms, with a responsive
fallback if it cannot meet the deadline. These are targets, not measured claims.
Preserve physical-footprint/process-tree measurement from #529 for comparisons;
the initial runner's `ps` RSS is only a diagnostic, never a substitute.
