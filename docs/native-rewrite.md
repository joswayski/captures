# Browser-free desktop migration

Status: **native capture/recording workflows implemented; screenshot and recording editor slices underway;
cross-platform acceptance and renderer selection still open**.
This rewrite covers macOS, Windows, and Linux, feature by feature rather than one
complete OS at a time. AppKit and the experimental Rust/wgpu host both connect
real capture engines in opt-in development builds; neither replaces the released app.
The shipping Tauri application remains available. No WebView, JavaScript runtime,
localhost server, or Tauri dependency belongs in the replacement. The website is
unaffected.

## Progress dashboard

We are delivering stage 4 workflow slices and stage 5 editor slices.
**Implemented is not accepted:** native CI and private-X11/software-rendered tests
do not replace physical macOS/Windows/Linux, accessibility or mixed-DPI checks.
The detailed checklist below remains the release gate; unchecked does not mean
unimplemented. Later slice notes supersede earlier notes about missing behavior.

Both screenshot-editor hosts keep selected move/resize/rotation and curve grips
live under the Rectangle/Ellipse/Triangle/Diamond/Star/Line/Arrow tools. Only the
active tool's own selected shape body starts a move; empty canvas still draws,
and a handle gesture cannot also create a shape. Text and Pen keep their style
Properties without this transform chrome. Geometry and shape-body hit testing
come from shared Rust. Private-X11 light/dark tests exercise active-tool edits,
undo, pixels and draft reopening; AppKit has XCTest coverage. Physical macOS,
Windows and Wayland acceptance remain open; these additions do not choose the
renderer or replace the shipping Tauri app.

Both hosts pin the Properties title outside the fields' scroll viewport, following
shipping's sticky heading without covering controls. AppKit retains its existing
title dimensions and field layout; wgpu keeps the shipping 48px title. Real-input
X11 checks compare fixed title pixels against moving fields for image, shape,
Text and Crop at the minimum size, in both appearances. AppKit XCTest checks the
same separation, resizing and reachable fields. Physical-platform gates stay open.

Curve and Straighten Properties now remain available for locked or hidden lines
and arrows on both hosts, matching shipping. Shared Rust permits these two
property edits without permitting locked canvas-dot dragging, insertion or
removal. Core tests cover exact geometry, undo and draft retention; host tests
check usable fields without canvas dots, and the X11 canvas suite exercises the
locked-line commands. Physical macOS/Windows/Wayland acceptance remains open.
Both hosts now keep Curve Properties enabled and focused during worker edits.
Discrete keyboard steps, slider releases and Straighten actions use the existing
ordered live-edit queue with separate undo keys, including keys received together
in one wgpu frame and changes that return to the original value. Clamped no-ops
do not enqueue an edit. Staged values survive older receipts; failures discard
queued edits and restore the last accepted value.
wgpu focus tests and AppKit deferred-worker XCTest cover consecutive input;
private-X11 light/dark checks apply Home plus three Right keys without refocusing
and undo every change exactly. Platform accessibility/IME acceptance stays open.

Both hosts release inline text's keyboard focus when Escape finishes, so the
first document Undo is not owned by a hidden text editor. Click-away preserves
the newly focused control; a failed finish retains the buffer and restores text
focus for retry. Host tests cover this handoff and failure recovery. X11 Undo
checks wait for the exact pre-edit layer, including its geometry, rather than
accepting an unchanged layer count. Physical-platform and IME gates stay open.

| Area | Implemented in this tree | Work still open |
| --- | --- | --- |
| Shared core | Settings/migrations, history/artifact lifecycle, capture coordination, recording engines/runtime, screenshot draft storage and document geometry/undo | Remaining editor actions and host bindings; installed-data migration/rollback |
| Capture and History | Region/window/display screenshots, countdown/cancel, seven configurable launch shortcuts, copy/save, shipping History header/card grid/empty and error states, counted media filters, History Restore to a floating preview with per-card errors, the capture menu's Capturing/Starting/Switching states and inline start/switch errors, two-step delete and delete all, missing-recording cards, original-recording export/reveal | Full input/coordinate/permission acceptance; large histories and editor reopen/restore |
| Recording workflow | Pause/resume/restart/mute/stop/discard, Hide/Show, passive region guide, screenshots during recording, ready/saved notices and HUD microphone meter; both hosts provide frame scrubbing, retained full-source thumbnail timelines, graphical/numeric trim, graphical/numeric crop, display-only Fit/100%, preset/custom output size, track volume/mute/mono, selectable GIF cadence, quality-mapped palettes and maximum width, Play/Pause with accepted-mix Sound on by default (like the shipping `<video>`), opt-in Loop preview pill, and MP4/GIF save-new-copy | Device-change parity and physical recording/audio acceptance |
| Supporting UI | First-run setup, appearance/preferences, resident tray/menu bar, live-profile single-instance forwarding/relaunch, opt-in development Open With packages and login items, retained preview stacks with collapsed drag and sway and a staggered hover fan, 3D pile tilt, Gaussian depth/hover/streak blurs and box shadows, editor presence, hover blur, stale-pointer suppression, glass tooltips, and shipping exit, flight and micro-motion, explicit optional feedback, permission recovery on a denied capture, OS reduced-motion change notifications (wgpu on Windows/Linux) | Remaining Preferences parity, remaining preview effects (backdrop blur), physical setup/login, permission revocation and installed Open With acceptance, crash reporting |
| Editors | Shared draft storage, geometry/undo, image/annotation rendering, hit-testing and encoding; both hosts connect layers, canvas selection/move/rotation/resize, move/resize snapping, curve and endpoint grips, Fit/100%/zoom steps/wheel and magnify zoom/pan/Recenter, canvas fill/transparency/trim, import, image transforms, annotation styles, Rectangle/Ellipse/Triangle/Diamond/Star/Line/Arrow/Pen/Wand/Erase/Restore with live brush pixels, Text with bundled fonts (all four families offered on every draft, pinned on first use; missing glyphs fall back to other bundled faces, then installed fonts) and shared new-text drop shadow, the shipping Erase/Restore brush ring, Trim edges hover preview, Wand loupe, the shipping header/rail and export bar (copy, overwrite Save, save-new-copy) | Remaining text parity (shipping's OS font stacks versus bundled faces, IME), Tauri inspector design parity; remaining recording-editor parity |
| Release readiness | Native build/test/fixture jobs on macOS, Windows and Linux; real-media private-X11 exercises; unsigned development package staging | Physical acceptance, accessibility/IME, Wayland live capture, release packaging/signing/updater, performance/energy and rollback gates |

Development package staging now supplies macOS Editor/Alternate document types,
Windows HKCU alternate ProgID/Application registration and Linux `%F` desktop
metadata for PNG/JPEG/WebP/GIF/MP4/WebM. It never registers automatically or changes
shipping identity/default handlers. Windows/Linux commands use `--live -- FILE...`
so paths cannot become options. The AppKit bundle defaults to live and collects
cold Apple events before instance election, including forced-secondary launches;
it resolves resources inside the package rather than a SwiftPM build path.
Python tests compare shipping formats, selective removal and actual GIO argument
expansion; native CI exercises AppKit LaunchServices and staged Windows command
delivery. Private X11 exercises the same positional forwarding path. Windows
Explorer, physical Finder/Linux file-manager acceptance, accessibility, Wayland
live capture, signing/notarization, redistributable dependency bundling and update
installation remain open. No parity gate closes from this development package.

The wgpu host now renders UI text in the token font stack instead of egui's bundled
faces: Segoe UI Variable Text / Segoe UI on Windows, and on Linux the first
fontconfig match for Inter, Roboto, Helvetica Neue, Arial, then `sans-serif` (as
WebKitGTK resolves it). A named `semibold` family backs `--weight-semibold`; egui's
faces remain glyph fallbacks. Under these wider faces the screenshot editor's header
Canvas toolbar compacts before its zoom controls at the 760px minimum.

Both native History windows now match shipping `CaptureHistory` (header, counted
filters, `.history-error`, interrupted recordings, grid). The native capture row is
gone: captures start from the tray, shortcuts and the New Capture menu (which owns
display switching), Preferences opens from the tray and app reactivation, and the
filter counts replace the "N of M captures" line. History reloads after every change
it makes, as shipping does on `capture-history-changed`, so Refresh is gone too;
AppKit relists displays on screen-parameter changes, and both hosts list them again
for a capture when the list is empty or lacks the display under the pointer (see
the Screenshot Display slice below). Permission recovery moved to the denied capture,
where shipping shows its dialog: AppKit keeps Restart & Retry and opens the
permission cards only when that prompt is unavailable; wgpu opens its recovery
dialog. X11 smokes drive captures through the shortcuts and open recovery with
`--live --permission-dialog ready`. The AppKit side is XCTest-only.

Both hosts now follow the shipping window model. **Capture History** (1020 × 720,
minimum 640 × 440), **Captures Preferences** (880 × 660, minimum 560 × 440) and the
first-run setup window (**Captures**, 620 × 560, minimum 480 × 440) are separate,
resizable top-level windows that can be open side by side; the old single fixed
workbench window and the wgpu "Capture History | Preferences" tab strip are gone.
Opening a window that is already open shows, restores and focuses it; closing one
leaves the others. Tray/menu-bar items, capture-menu setting links, Preferences'
**Capture History…** button and empty relaunches route to the matching window;
reopen follows shipping's priority (setup, then an open editor, then History, then
Preferences, else open Preferences). A visible launch follows shipping's
`interactive_launch_action` (`captures_app::app_windows::interactive_launch`): setup
while it is unfinished, the launch notice on a quiet (login) launch, no window
besides the editors when opening files, and otherwise Preferences alone; wgpu's
`--open-history` opens History instead for the automated exercises, and a failed
file open shows History with the error. Setup completion hides the setup window, as shipping does;
wgpu keeps History open instead when no tray host exists, so Captures stays
reachable, and without a tray closing the last open window quits. Captures hide
Preferences along with History. Titles, sizes and breakpoints live in
`captures_app::app_windows` (AppKit mirrors them in `AppWindows.swift`). The editor
windows use the shipping titles **Captures Screenshot Editor** and **Captures
Editor**; the wgpu editors add a " — Working…" suffix while a job runs, and the
AppKit screenshot editor a " — Unsaved" suffix, because the automated exercises
wait on them. Layouts reflow down to each minimum: at shipping's 720px breakpoint
History stacks Delete all under its heading and uses one card column, Preferences
hides its section nav, stacks inline rows and uses two-column grids, and the
560px-tall setup window drops its lede (`max-height: 600px`). wgpu keeps setup and
History in the one root window (setup is retitled and resized into History when it
completes; the two never coexist) and opens Preferences as a child window. Private
X11 exercises cover the wgpu windows, the Preferences-only visible launch and reopen
focusing an open editor; the AppKit windows, launch, reflow and focus are verified
only by XCTest.

Preferences now offer the shipping Default microphone select (Off plus enumerated
inputs; wgpu enumerates when the menu first opens, AppKit off the main thread),
the AppKit GIF export card (frames per second, maximum width, palette colors)
and AppKit scroll-spy highlighting of the section in view. The wgpu shortcut
recorder shows `<kbd>`-style key chips and the "Press shortcut…" prompt.

Both Preferences hosts now follow the shipping layout and controls, with copy,
option labels, themes, mini-preview corners, shortcut rows and find matching from
`captures-app::preferences` (AppKit through `captures_preferences_v1`): the
sidebar brand mark with hover/active entries, the header rule and save-status
pill, shadowed cards with row rules, whole-row 32×19 switches, the
System/Light/Dark segmented control, accent chips (accent swatch with a signal
wedge and a check), the custom colors editor, the mini preview corner picker,
field-styled selects with shipping labels (PNG, Off, 1 second, 1080p…), the
Recording and GIF select grids, `<kbd>` recorder chips on both hosts, and find
washes with a ringed current match. Rows that cannot work in a native build say
why: Check Now stays disabled (signed updates are not connected), the system
screenshot-shortcut row shows the shipping takeover copy (live hosts unbind the
overlapping system keys) and opens keyboard settings in live mode, and fixtures
show the login item as
unavailable. Rendering was checked on X11 only; AppKit is covered by XCTest, and
Windows, Wayland and screen-reader acceptance remain open.

Accessibility and keyboard parity for setup, overlays and the main windows:
wgpu now names the direct overlays like AppKit (AccessKit groups "Capture region
selector" and "Capture window selector"; the window group's value is the hovered
target, with a polite live "Target: …" description, and a capturable region reads
"Selected region W × H logical pixels"). Its setup and recovery cards follow the
shipping `Onboarding.tsx` aria attributes: the header is named by its title, the
cards are a polite live region, each card is an article named by its heading, and
errors are alerts. Statuses read "Granted", because shipping hides the check mark;
AppKit matches that and exposes each card as a named group. AppKit windows no longer
rely on the geometric default key-view loop. Setup, Preferences, History, the
recording editor and the screenshot editor (whole window, with the export footer in
shipping order) install explicit loops that follow the shipping DOM order, and each
sets a first responder: the first usable setup control, the first Preferences
section, the History grid, recording Play, and the active screenshot canvas.
Preview cards reveal their controls on keyboard focus like shipping `:focus-within`.
The AppKit panel stays nonactivating: it takes keyboard focus (Ctrl-F6, Cmd-`) only
while Captures is already active, and clicks never make it key. On Windows the wgpu
preview reaches its controls with Tab once its window is active. On X11 the preview
is an override-redirect window that never receives keyboard focus, so its controls
are pointer-only there. Feedback (being rebuilt) is not covered. XCTest checks the
loop orders and wgpu tests inspect the AccessKit tree. VoiceOver, Narrator, Orca and
physical Full Keyboard Access checks remain open.

Shared UI primitives now follow shipping `base.css` and `primitives.css` on both
hosts, from one helper per host (`Primitives.swift`, wgpu `primitives.rs`). Keyboard
focus draws the `--focus-ring-tight` token ring: AppKit `CaptureButton` (Feedback,
setup, History and notices) uses it in place of a solid accent stroke, and wgpu
paints it around whichever stock egui control (button, text edit, checkbox…) holds
focus at the end of each pass unless a custom control drew its own indicator.
AppKit clips drawing to a view, so its ring sits just inside the control. Scroll
bars are a 4 pt pill thumb inside a 10 pt bar in `--border-strong` (`--text-faint`
on hover) over a transparent track. wgpu sets this globally as overlay bars so no
layout reflows; AppKit uses a token `NSScroller` that keeps the system scroller style,
so overlay scrollers still fade when idle, and the recovery list no longer forces
legacy scrollers. Preview stacks hide their scroll bar like `.thumbnail-stack`.
Selects share one primitive per host. wgpu draws the `CustomSelect` field trigger
(media palette in the capture menu, borderless for the recording filename format)
and a token listbox with option descriptions, placed and driven by shared
`captures_app::controls::select` (ArrowUp/Down, Home/End, Enter/Space, Escape), in
Preferences, the capture menu, the region aspect picker and the recording editor. An
open wgpu listbox counts as an egui popup, and raw-pointer gestures (editor canvas and
viewport, recording timeline and crop) ignore presses on any foreground layer above
them, so a row or popover over a canvas takes the click and keys instead of the canvas.
AppKit's token `ClosurePopUpButton` trigger replaces the
capture menu's glass popups and the recording editor's stock popups and opens the same
token listbox (`TokenSelectListView`, glass in the capture menu) in a child window
instead of a native menu, so its rows take their own clicks: each option's description
sits under its label in `--text-xs` faint text, the selected option carries a check,
and the shared select model drives its keys (including Home/End) and placement through
`captures_controls_v1`. The screenshot editor's selects use the same primitives: Crop's Aspect ratio,
the selected text's Font, the export bar's Output size (with shipping's option
descriptions) and format suffix on both hosts, plus AppKit's Save quality, Compress
and file-size unit, which already did. The text style pickers draw shipping's
`TextStylePicker` trigger (preview chip, label, chevron) over chip rows; AppKit's
`ClosurePopUpButton` draws the selected item's chip. The header zoom preset keeps a
token trigger over a plain menu on both hosts, matching shipping's native `<select>`.
Recording editor number fields (crop, output size, trim; wgpu also Position) follow
`NumberInput`: wgpu `primitives::NumberInput` and AppKit `TokenNumberField` draw the
token field with Increase/Decrease steppers ("Increase {label}", hidden while
disabled, outside the Tab order) and step on ArrowUp/ArrowDown through shared
`captures_app::controls::number` (AppKit via `captures_controls_v1`). Sliders follow
`RangeSlider`: wgpu `primitives::RangeSlider` (value readout, 4 pt accent track,
14 pt thumb, optional ticks, labels and description; arrow, Page, Home and End keys)
replaces egui sliders for the encoded split and track volumes; AppKit `TokenSlider`
draws the same track and thumb on its NSSliders and keeps the editable volume
percent fields. The maximum file size field stays a plain text field on both hosts.
Physical focus-visibility and scroller checks on macOS and Windows remain open.

Direct region and window overlays (shortcut, tray and screenshot-during-recording)
now follow the shipping `CaptureOverlay`: no toolbar, a completed region drag
commits on release, a window/desktop click commits that window or the display,
and a click without a region shows "Click and drag to select a region" for 1.8
seconds. "Automatically start on selection" applies only to the New Capture
controls, which keep aspect presets and Enter confirmation.

Both hosts now draw these overlays like shipping `CaptureOverlay`/`CaptureDim`,
using new `capture-shade`, `capture-shade-window` and selection hairline tokens.
Region mode dims with the lighter shipping shade. Its square marquee has no corner
handles: a 1.5 px accent line between a dark outer and a light inner hairline, with
the accent size badge at its left edge. The window overlay has no toolbar and stays
clear until the pointer is over something. A hovered window gets the window shade,
with a hole that follows its rounded corners, plus a 14% accent fill, an outer ring
and its glass title chip. Over the desktop the screen stays clear, with an inset
outline and an "Entire display" chip at the top left. AppKit takes the outline's
corner radius from the screen's outline, like shipping. wgpu has no display radius
on Windows or Linux, and shipping uses 0 there too. New Capture uses the same shades
and frame hairlines, and keeps its handles and centered badge. The direct overlays
now share New Capture's guidance chip (see the overlay guidance slice below). The
region overlay's shade, and New Capture's region and window shades, fade in over
`--dur-4` `ease-in-out` once the overlay is revealed (`captures_app::motion`
`capture_shade_fade`) while the frozen snapshot is opaque from the first frame;
the direct window overlay and New Capture's Full screen shade appear at once, as
in shipping. AppKit draws the dim in its own view and fades its layer
(presentation-only); wgpu scales the shade's alpha. New Capture's desktop-hover
dim still differs from shipping.
Rendering was checked on X11. AppKit is covered by XCTest only.

Shortcut, tray and New Capture flows now start on the display under the pointer,
like the shipping `capture_display_at_point`: wgpu resolves it through the shared
`XcapBackend::display_id_at_point` and AppKit through `NSEvent.mouseLocation`. When the pointer position is unavailable
(Wayland) the current display is kept. Multi-monitor and mixed-DPI physical
acceptance remains open.

Screenshot Display, capture failures and display recovery now match shipping on
both hosts. The display shortcut and tray "Screenshot Display" open the capture
menu in Screenshot mode on Full screen, with its display picker, instead of
capturing at once (`open_capture_controls_with_target`). Opening on Full screen
never auto-starts; only choosing it or another display does. While a recording
runs or is paused they take a display screenshot beside it instead (see the
screenshots during recording slices below). A failed tray, shortcut or menu capture shows shipping's
`report_capture_error` dialog, titled "Captures" with one OK button (copy in
`captures_app::capture_error`, exposed to AppKit through the settings ABI). AppKit
uses a sheet over History; wgpu draws the dialog in its own window, so it shows
while History is hidden (shipping's stock dialog needs `zenity` on Linux). wgpu
recording flows that end before a take use the shipping "Captures Recording"
title. The History error card now only shows History load and delete failures;
capture, shortcut and startup display-list failures no longer land there. Each
capture resolves its display fresh, like shipping's per-capture monitor lookup:
when the list is empty or lacks the display under the pointer, the host lists
again before starting, so a failed first list recovers without a relaunch. X11
smokes open the menu from the display shortcut and the tray, confirm the
preselected Full screen target, and check the failure dialog, the untouched
History card and recovery in both appearances. macOS is covered by XCTest only;
Windows, Wayland and multi-monitor acceptance remain open.

Development login items are now explicit, OS-authoritative Preferences controls:
per-profile macOS LaunchAgents, Windows HKCU Run values and Linux XDG autostart
files. They use the current executable and exact development settings/History
paths, never Tauri registration or an automatic settings side effect. Conflicts
and symlinks are preserved; disable before moving/removing a binary. Hidden live
startup retains the resident host and relaunch path instead of the idle fixture
UI. Missing X11 tray hosts expose the recovery UI; Wayland hidden startup remains
gated. Shared tests exercise conflict preservation and argv roundtrips through
GIO, plistlib and Windows process parsing; host tests cover authoritative async
state, retry and fixture isolation. Physical macOS/Windows/X11 sign-in,
accessibility and installed-update lifecycle remain unverified. No parity gate
closes from this development registration slice.

Fresh live profiles now gate capture, shortcuts and queued external media on native
setup. Checking never prompts; explicit macOS requests record the executable
identity before asking, offer Settings after denial, and keep microphone optional.
Completion rechecks screen access and preserves trusted settings fields against
older Preferences saves. AppKit re-checks access every 1.5 seconds while setup
waits for a grant and restarts automatically when the user returns from 2.5 seconds
or more in System Settings with screen access still unreported, like shipping
`Onboarding.tsx`; Restart Captures stays available. The restart flushes work, stops delivery,
drains queued media and releases the instance owner before spawning the same
development profile. Windows/X11 have no upfront screen prompt; hidden first-run
launches expose setup, while completed profiles retain resident startup behavior.
Wayland capture remains gated. Rendered AppKit fixtures and private-X11 interaction
checks are diagnostics; physical TCC/signature changes, OS microphone prompts,
Windows presentation, accessibility and capture-time permission revocation/retry
remain open. This slice does not close the onboarding acceptance gate.

Completed native profiles now have a permission-recovery dialog, opened by a
denied capture (see the History parity note above). It shares prompt-free checks and explicit macOS screen/mic
requests with setup, but never calls completion or exposes first-run restart.
Done remains available after denial/check errors, retaining the workspace and
editors; queued external media waits until dismissal. Capture and shortcut actions
are blocked while it is open. Foreground return and Refresh recheck access without
prompting. Windows/X11 report no upfront screen grant and unknown microphone status;
Wayland capture remains gated. Physical revocation/retry, OS prompts and accessibility
acceptance remain open. No permission or onboarding acceptance gate closes here.

Both hosts now render setup like the shipping `Onboarding.tsx` window: app mark,
"Welcome to Captures" eyebrow, the per-platform title ("Required permissions" on
macOS, "You’re ready to capture" elsewhere), the shipping privacy lede, and
permission cards with icon, description and either an action or a status
("Granted ✓", "Ready ✓", "Restart required", "Still off"). The macOS-only
microphone card carries "Optional" and offers Open Settings only after it was
asked once this launch. The primary action is Start capturing, or Restart Captures
when macOS needs a relaunch; its halo pulses like shipping's CTA
(`captures_app::motion::OnboardingCtaPulse`, 2.6 s) and rests under reduced
motion. First-run setup has no Refresh button, like shipping;
permission recovery keeps Refresh status in both hosts.
`captures_app::onboarding` derives the copy and per-state decisions once
(`State::presentation`, `copy()`), exposed to AppKit through the settings JSON ABI
(`onboarding` responses carry `presentation`; `onboarding_copy` and
`onboarding_presentation` are I/O-free). Permission recovery reuses the same cards
in both hosts; the wgpu dialog is a token-styled card instead of a stock egui window.
Private-X11 onboarding smoke covers the new layout in light/dark; AppKit XCTests
render setup/recovery states. Physical macOS TCC, Windows presentation, screen
reader and Wayland acceptance remain open; no onboarding gate closes here.

Both hosts now render the live workspace's History like the shipping
`CaptureHistory` window instead of a list and preview pane: the "On this device"
header and lede, counted filter pills, the Interrupted recordings card, and a
virtualized auto-fill grid of thumbnail cards with date, "W × H · size"
(plus duration and dropped-frame warnings), Edit and Restore for screenshots, Edit and
Save file → Show in Folder for recordings, a trash control that arms **Delete forever**, and a **File missing**
state for recordings whose media is gone. Header **Delete all** arms
**Delete all forever** with Cancel; both confirmations revert after four seconds
or on Escape. Loading, empty and error states use the shipping copy.
`captures_app::history_view` owns copy, card details/actions, the missing-media
rule and grid metrics; AppKit reads them through the `history_copy`,
`history_cards` and `history_grid` settings operations. Native differences: no
card is selected on load, but explicit selection, arrow keys and Return are
supported; secondary click lists card commands (including Copy image, Save image
and Show in Folder for screenshots). Private-X11
history, recovery, capture and preview smokes exercise the wgpu grid; AppKit XCTests
cover cards, filters, two-step deletion and rendered light/dark grids but have not
run here. Physical macOS/Windows, Wayland and screen-reader acceptance remain open;
this does not close the Viewer/history gate.

History **Restore** now matches shipping `restore_history_artifact`: screenshot cards
only, "Restoring…" then "✓ Restored" for 2.5 seconds, with the "Bring this screenshot
back as a floating preview" tooltip. Both hosts reopen the screenshot through their
mini-preview stack as the front card, without a capture generation, auto-copy or
History change. A card already in the stack is neither duplicated nor moved but still
confirms Restored; an empty stack opens on the workspace's selected display (AppKit
falls back to the workspace window's screen), while a non-empty pile keeps its
display and position. A card dismissed before it decodes ends the restore quietly. History **Edit** on a screenshot now restores
its preview through the same path before opening the editor, as shipping's
`restore_history_artifact` call does, without Restore's busy state or feedback; a
failed restore opens no editor. A failed Restore or Edit restore (no display for an
empty stack, unreadable settings, an undecodable image) shows shipping's
`.history-card-error` under that card's actions, not in the status line; the card's
next action clears it. Like shipping's CSS grid, the error grows its row and the
row's other cards stretch while keeping their actions in place
(`history_view::RowExtras`, `card_error_extra`); both hosts measure the wrapped text.
Other card actions still report through the status line. The private-X11 history
smoke restores a card, checks that a second Restore opens no extra window and that
Delete all removes the preview; wgpu unit tests and AppKit XCTests (not run here)
cover success, already-showing, dismissal, the card error and its row growth.
Physical macOS/Windows and Wayland acceptance remain open.

Preview stack motion now plays on both hosts from shared data
(`captures-app::motion` keyframes and transitions, `captures-app::preview_motion`
for the exit state machine, survivor settle, Clear all stagger, toolbar presence,
seeded dust particles and sparkle tables): the Close streak, the dust Delete (with
the scale-and-fade fallback), survivors sliding into the held slot, the list ↔ pile
flight, the stack toolbar in/out/exit/clear keyframes, the 240 ms Show less morph,
the capture highlight, main-action icon pop, clipboard chip arrival and the hovered
pile sparkle, plus the "Not in History"/"Clipboard unavailable" card warnings (native
captures reach only the latter, after a failed copy). Reduce Motion skips all of
them. wgpu draws the hover blur and the Close streak with real separable Gaussians
(see the preview effects slice below). Dust chips follow shipping's canvas path on
both hosts: each chip is cut sharp from the card media, padded with 8 pt of
transparency and blurred on its own (`blur(2px) brightness(.5)`), so flying chips
keep soft edges. AppKit filters one Core Image atlas; wgpu blurs one atlas at a
pixel per point when the exit first paints. A saved card's Delete now runs in
shipping's order: the card dissolves at once, the Trash request goes out after the
dust has played and the stack settled (at once under Reduce Motion), and a failed
Trash puts the card back in its slot with the error so it can be retried; a card
presented again under the same ID meanwhile is left alone. wgpu holds the
dissolved card's empty slot (and the preview window) until the reply, as shipping
keeps the card until `trash_artifact` resolves; AppKit rebuilds the stack when the
dust ends and re-adds the card if the Trash fails. wgpu unit tests and
AppKit XCTests (not run here) cover the state machine, exits, flight, morph,
warnings and the Trash order. The private-X11 preview
`--stack` smoke now waits for settled pixels after exits and flights (and before
its frozen-capture comparison) and lets the hovered pile's sparkle dots through its
fan checks. Physical macOS/Windows and Wayland acceptance remain open.

Both native hosts now connect pointer dragging on the collapsed preview front
card, separately from click-to-expand. Desktop-coordinate tracking compensates
for the native window moving under the pointer. Shared geometry clamps the pile
to its capture display; the clamped session edge survives expansion, cancellation
and new captures. Empty/disabled piles and changes to the preferred corner clear
the custom position. Private-X11 interaction checks cover a minimized root,
external-app focus, arrivals and reset; AppKit tests cover native pointer events
and Retina/negative-origin geometry. Cross-display dragging, automatic anchor
changes, native file drag, hover-fan/effects, physical Windows/macOS input and
accessibility acceptance remain open; Wayland capture remains gated. This does
not close the preview or layout/effects parity gates.

The former History and recording/HUD/feedback stacks are integrated through
[#583](https://github.com/joswayski/captures/pull/583),
[#585](https://github.com/joswayski/captures/pull/585),
[#586](https://github.com/joswayski/captures/pull/586) and
[#593](https://github.com/joswayski/captures/pull/593).
[#592](https://github.com/joswayski/captures/pull/592) combines in-recording screenshots
with those flows; its tests retain both screenshot-child and saved-notice coverage.
Superseded parent PRs may be closed rather than separately merged because the
repository uses squash merges. Their functionality must not be counted as missing.

The screenshot-editor stacks from
[#633](https://github.com/joswayski/captures/pull/633) (AppKit) and
[#634](https://github.com/joswayski/captures/pull/634) (wgpu and shared prerequisites)
are combined in this tree. Both hosts connect layers, import, lossless transforms,
output previews, save-new-copy and rectangle/ellipse drawing. AppKit now also
connects annotation styles ([#637](https://github.com/joswayski/captures/pull/637))
and Line/Arrow/Pen ([#638](https://github.com/joswayski/captures/pull/638)), matching
the existing wgpu command boundary. Both hosts connect crop gestures and clipboard
output. AppKit's Crop tool retains shared Rust `CropDrag` geometry through an
independent UI-thread C owner; no worker session, JSON or file access occurs during
pointer feedback. Free/preset ratios and Shift latching use the same shared rules
as wgpu. The preview and the read-only Width/Height track one selection; Apply crop
sends one crop transaction (a rejected crop stays staged), Clear or Escape drops the
selection with the tool still ready, and leaving the tool or closing cancels it.
Focus loss and viewport changes cancel only an active pointer gesture, without
committing the selection. Dragging does not dirty drafts or invalidate encoded output.
Physical AppKit pointer/mixed-DPI acceptance remains open.
The AppKit drawing slice passed 136 Swift tests in macOS CI;
its light/dark transient, committed, dot and minimum-size error fixtures were inspected.
These are development implementations, not completed platform acceptance gates.

Both hosts now expose pre-placement annotation stroke/fill colors, stroke and fill
toggles for closed shapes, 2–40 px width and 0–100% opacity. Shared Rust supplies
initial style values; subsequent choices are editor-local, survive tool switches
and worker responses, and do not alter documents, output or drafts until drawing.
Closed shapes, Line/Arrow and Pen submit those values through the existing single
undo transaction. Open strokes ignore the retained closed-shape stroke/fill toggles.
AppKit's allocation-owned geometry ABI accepts explicit arrow width while retaining
the original v1 default contract; its opacity composite does not affect brush guides.
Both hosts now render uncommitted closed shapes, Line/Arrow and Pen through the
shared renderer on their serialized worker. Until its first result arrives, a host
vector guide remains approximate; afterward fill/stroke overlap, caps and shadow
pixels use the same renderer as commit. One render is in flight with one replaceable
pending request, avoiding a pointer-event backlog. Cancellation, commit and editor
closure invalidate late frames. Previewing leaves the document, undo/redo, published
pixels, assets, encoded output and drafts unchanged; release creates one layer.
The C bridge returns independently owned pixels without a JSON pixel payload.
Erase/Restore remain outline-only until release. Large documents can lag behind
the pointer while rendering; physical input latency and resource acceptance remain open.
Both hosts also expose pre-placement drop-shadow enable,
color, opacity, blur and X/Y offsets for closed shapes, Line/Arrow and Pen.
Untouched shadow defaults follow stroke width through Rust's resolver; editing a
shadow field retains a custom style through tool switches, disabled/enabled toggles
and worker responses. These settings do not edit the document until drawing.
AppKit's pure C default resolver does not touch a session, filesystem or renderer.
Physical platform input, accessibility and Wayland live acceptance remain open.

Both hosts now connect canvas click-selection and transactional drag-move in Layers.
Shared `Element::selection_bounds`, `selection_outline` and `Document::hit_test` match
the shipping rotated local-box picking rules, including stroke/shadow padding,
curved lines/arrows, empty/dot paths, hidden/locked layers and zero-opacity content.
Shipping-TypeScript fixtures exercise both sides of boundaries and exact fractional
edges. Unsupported text layout returns an explicit error; no approximate font
metrics or silent selection through unsupported content. A press inside the fitted
edited image picks once at eight view points of tolerance. A click selects or clears
without changing the document or encoded output. A drag of at least three view points
shows a translated shared outline and submits one `LayerEdit::Translate` on release;
pixels update only after the worker succeeds. Failed moves preserve prior selection.
Escape, focus loss, close, leaving Layers, a pending command or preview resizing cancels
transient input. AppKit picks from cached immutable document JSON, never a borrowed
worker session; wgpu handles raw events once, in order, across egui layout passes.
Both hosts also expose a rotation grip for the selected visible/unlocked layer.
Shared Rust chooses a grip that fits the bitmap, preferring outside top/bottom then
inside top/bottom, and owns the rotated outline and angle normalization. Shift snaps
to the configured stops, including modifier changes without pointer motion.
Both hosts expose **Layers → Shift rotation snap**, rounded/clamped to 1–180 degrees
with a 15-degree initial value. This is per-editor UI state: changing it does not
edit a layer, clear encoded output, create history or write a draft. AppKit parses
the field using the editor locale. Shared geometry retains Tauri's signed half-tie
rounding and finite-range/default rules; the original C v1 call keeps 15-degree
stops and v2 adds the configurable increment. The grip wins over overlapping layer
bodies. A changed angle submits one `LayerEdit::Rotate` on release, with normal
render-before-publish, undo and draft ownership; clicks and cancellation do not edit
the document. Partial overflow remains clipped; fully outside rotated bounds expand
the canvas. AppKit's C boundary is allocation-free, without per-event JSON or worker
session access. TypeScript-oracle fixtures check angles, grip placement, gestures and
document edits. Both hosts retain outline-only feedback until release.
Custom-increment tests distinguish 37-degree stops from the former hard-coded 15,
including stationary Shift changes, release, cancellation and draft restore.
X11 software-rendered checks and AppKit host fixtures are diagnostics, not physical
macOS/Windows/Wayland input or accessibility acceptance; those gates remain open.
Both hosts also connect eight resize grips and border hit regions. Shared Rust
retains original element geometry and snap lines for a gesture; AppKit holds an
independent immutable C owner, without per-event JSON or worker-session access.
Shift locks corner aspect ratio while edge grips stay single-axis. Unrotated
resizes snap to canvas and other visible-layer edges (including locked layers);
rotated resizes skip axis snapping and preserve the opposite world anchor.
Images retain D4 orientation; arrows scale controls and stroke, while paths retain
their stroke width. Preview outlines and guides do not modify pixels or drafts.
A release after three view points submits one worker transaction; cancellation,
clicks and failures preserve the document, and fully outside content expands the
canvas. Text layers use the text resize rules described with the text slices below.
Canvas movement also retains immutable original geometry and snaps painted world
bounds to canvas and visible-layer edges, including locked and zero-opacity layers
but excluding hidden layers. Shared Rust matches Tauri's strict ten-view-point
threshold, line/edge tie rules and up to four coincident-edge guides. Hosts keep
clicks and movement below three view points unsnapped. A `drag_move` release
commits once; numeric `translate` remains exact. Preview is outline-only, and
fully outside moves expand the canvas. TypeScript oracle fixtures cover rotated
geometry, threshold boundaries, ties, hidden/locked siblings and overflow.
Both hosts now connect ephemeral viewport state through shared Rust geometry:
Fit, 100%, 1.25× zoom steps, Recenter, anchored Cmd/Ctrl-wheel/native magnification
and Cmd/Ctrl-primary or middle-button pan. Manual zoom uses Tauri's 5–800% bounds
and tenth-percent rounding. Pixels and edit overlays share the transformed rect
and viewport clip. Viewport changes cancel active edit gestures without document,
draft, undo or encoded-output changes. Fit resets pan; Recenter preserves zoom.
Both hosts accept Cmd/Ctrl +/− (1.25× steps) and 0 (100%, not Fit), also with a
text/numeric field focused. They cancel pending canvas gestures and use the viewport
center as zoom anchor. wgpu consumes ordered/repeated key events before egui global
UI zoom and acts once across layout passes; AppKit routes key equivalents and field
editor events in the editor window. Sheets/confirmation popups retain keyboard
ownership. This does not register new OS-global shortcuts. Automated host tests
cover bounds, event ordering, field focus, cancellation and no document/output writes;
physical keyboard layouts and accessibility acceptance remain open.
Both hosts route editor-local Cmd/Ctrl Z and Shift Z through existing Undo/Redo
commands. wgpu uses egui's text-edit focus state, leaving typing undo untouched
while allowing document history from focused action buttons; open popups and
confirmation/closing states retain keyboard ownership. Events are consumed once
across layout passes, and repeats never queue behind accepted work. AppKit checks
the native first responder before routing either modifier, replacing unconditional
button key equivalents. Disabled history actions do nothing. These bindings retain
normal output invalidation and render-before-publish behavior; they do not save
drafts or register OS-global shortcuts. Physical input/IME/accessibility remain open.
Both hosts also route Cmd/Ctrl D to duplicate the selected layer and Delete/Backspace
to delete it unless locked. Hidden/locked selections can be duplicated through the
existing shared command. Duplication selects the fresh ID after acceptance; failed
keyboard duplication retains the original selection. Text fields and dialogs keep
keyboard ownership, accepted shortcuts cancel transient gestures, and repeats do
not queue behind the worker. Arrow keys now nudge an unlocked selection by one
image pixel, or ten with Shift, through shared `LayerEdit::Translate`. Hidden
layers remain editable; keyboard movement neither snaps nor expands the canvas.
Each accepted nudge retains normal undo and output invalidation. AppKit protects
field/selector/slider responders; wgpu reserves arrows for any focused widget.
Both hosts copy and paste layers. Physical keyboard/IME/accessibility
acceptance on macOS, Windows, X11 and Wayland remains open.
Both hosts connect Tauri's tool keys to their existing tools: V Select, C Crop,
T Text, R Rectangle, O Ellipse, L Line, D Diamond, S Star, A Arrow, P Pen and
B background removal. B recalls Wand/Erase/Restore, initially Wand. These
case-insensitive canvas keys accept Shift but not Cmd/Ctrl/Alt. Focused native
controls retain typing and letter navigation; pending work and dialogs block
switching. A different tool cancels transient drawing, layer transforms, crop and
pan; repeating the current tool preserves its candidate. C starts crop mode but
never applies it, and tool selection does not change the document, undo or output.
wgpu consumes each event once across layout passes. Host tests cover the map,
background-mode recall, repeated tools, cancellation and focus/accepted-work
gates; the X11 shortcut suite also creates Star/Rectangle with keys, moves the
same layer with V and cancels a crop without publishing it. Physical keyboard,
IME, accessibility and Windows/Wayland presentation acceptance remain open.
The next layout slice adds a persistent left tool rail on both hosts in Tauri's
order: Select, Crop, Text, grouped Shapes, Arrow, Pen and background removal.
Its neutral icon buttons use the accent for the current tool and expose labels,
tooltips and native button actions. Shapes uses an AppKit/egui menu (not Tauri's
three-column flyout) for Rectangle, Ellipse, Line, Triangle, Diamond and Star;
it remembers the last grouped tool. Rail actions reuse the shortcut activation
path, including transient cancellation, and remain disabled during accepted work.
The existing inspector controls remain available. The canvas gives up rail width
but retains shared Fit/zoom/pan and pointer mapping; minimum windows remain
760×540. Header, inspector and footer still differ from Tauri, so this is not
visual parity. Automated host fixtures and private X11 cover selection/menu,
minimum layout and busy gates; physical focus, accessibility and Windows/Wayland
presentation acceptance remain open.
The editor chrome slice then gives both hosts the shipping header and rail, from
`captures_app::editor_chrome` (AppKit through `captures_editor_chrome_v1`) and the
shared `EditorIcon` set. The workbench title, lede and two-row toolbar are gone: one
52-point header holds the Canvas toolbar (W × H fields that commit one resize on
Enter or leaving a field, Trim edges and a Background color trigger opening the
canvas background card) and, on the right, Undo/Redo, the Fit/−/log slider/+/preset
zoom group and Add images. As in shipping, Undo/Redo hide at 1040 points and below
(Cmd/Ctrl Z still work) and the slider and preset narrow from 92/76 to 72/72; the
Canvas toolbar drops its label, then shows Trim and Background icon-only, before
clipping. Drafts autosave as in shipping (`editor_session::DraftAutosave`, 700 ms
after each change, in the background on each host's session worker) and flush on
close without a prompt; `Request::AutosaveDraft` removes the draft when undo returns
to the unedited capture, and draft saves only write new images. There is no header
draft menu. A draft found at open shows the shipping "Restored unsaved edits
from last time." banner; its Discard resets without confirmation and Dismiss hides
it. Recenter becomes the fixed-glass pill shown only while pan leaves the canvas
mostly off screen. The Geometry/Layers/Draw tabs are gone: the rail's tool chooses
the inspector. The rail uses the shipping labels (Eraser (B)), 38-point buttons with
2-point gaps, hover and accent states and immediate glass hover tips; wgpu shows the
Shapes corner cue and shipping's three-column icon flyout (44-point buttons).
Inspector sections open with the tool name (Crop, Freehand, Eraser…) or shipping's
Layers heading with a count and Add image layer; layer rows show the shipping names
and kinds with eye and lock quick actions (lock also selects its row). wgpu editor
errors use the export status line, as in shipping.
The inspector/Layers slice then removes the native Draw tool grid and AppKit's Tool
menu, so the rail alone picks the tool (Eraser shows shipping's Wand/Erase/Restore
mode group). Both hosts always show Layers above Properties in shipping's
`minmax(188px, 40%)` split. Rows are 54 points with a grip, a live thumbnail rendered
by shared Rust (`editor_layers::ThumbnailCache`, sent to AppKit as PNG data URLs in
`layer_thumbnails`), the name over the kind, and eye/lock/⋯ quick actions. Dragging
a row submits one `LayerEdit::Reorder`; double-clicking an image row renames it in
place. The ⋯ popover holds Blend mode (`LayerEdit::BlendMode`, already honoured by the
shared compositor for display, saves and exports), a live Opacity slider, image
Transform tiles, Bring to front/Send to back (`LayerEdit::Arrange`), Merge down,
Merge visible, Flatten image, Duplicate and Delete. Image Width/Height/X/Y
(`LayerEdit::Geometry`, aspect-preserving like shipping) and selected text apply
live; `Request::Live` folds each field's burst into one undo step, and edits made
while a job runs queue on the host. Annotation stroke color and width, opacity,
fill and shadow also apply live (Apply style and Reset fields are gone): a burst in
one field folds into one undo step and each toggle is its own.
The inspector sections slice then lays Properties out like shipping's
`.screenshot-properties`. Under the heading each section has `--s-5` padding, a
`--s-5` item gap and a rule below; labels sit `--s-3` above their control in
`--text-sm` muted text, numbers pair in two `--s-4`-apart columns, hints are
`--text-sm` subtle paragraphs and checkboxes are 15 pt `.screenshot-check-row`
boxes. Sections follow shipping order: a selected layer opens with Shift rotation
snap (and its increment hint), then image Width/Height/X/Y, or text (Text style
picker showing the layer's current treatment, Text, Font beside Size, B/I/alignment,
Text color, Text background, Background color, Drop shadow), or annotation (Stroke,
Stroke color, Stroke width, Opacity, Drop shadow, Filled shape, Fill color, Curve).
Drop shadow is shipping's `DropShadowFields`: the check row, then Shadow color
swatches, Opacity and Blur sliders and the X/Y offset pair indented to the label.
Drawing tools show the grouped-shape picker (Shapes), the tool preview, Stroke,
Color/Stroke color, Size and Opacity sliders, Filled shape, Fill color and Drop
shadow; Text shows New text style, New text size and Drop shadow; the Eraser keeps
its intro, mode group, sliders and hints; Crop shows the Aspect ratio select and,
once a selection is dragged, its read-only Width/Height, Clear and the pulsing
Apply crop, or shipping's drag hint. Native-only rows and copy are gone: the Outline
and Rounded plate controls (the Outlined and Rounded Box styles set them, and "Text
background" adds shipping's `#111318` plate, clearing both) and the per-tool helper
paragraphs. wgpu draws every row from `editor/inspector.rs` with the shared
`NumberInput`, `RangeSlider`, `Select` and `ColorField` primitives (a typed decimal
such as 37.5 now commits as typed when focus leaves). While the rail's Crop is
active, wgpu stays ready for a new selection after Apply crop, Clear or Escape, as
shipping does, and a selection starts only once a press becomes a drag. AppKit follows
the same order, spacing, labels and control types: the selected layer (Shift rotation
snap first, with its hint and rule), selected text, annotation style and the drawing
and text defaults use `EditorMarkedSlider` `RangeSlider`s with value readouts (Stroke
width and Size 2–40 px, Opacity 0–100%, shadow Opacity 0–100% and Blur 0–100 px),
check rows and one shared `EditorDropShadowFields` (Shadow color swatches, the two
sliders and the X/Y offset `TokenNumberField` pair, ±500 whole pixels committed on
Enter, leaving the field or a stepper, like shipping's `commitOffset`); unchanged
settings keep their authored precision. Shift rotation snap (1–180), New text size and
the selected text's Size (8–512) are `TokenNumberField` NumberInputs whose steppers and
ArrowUp/ArrowDown keys step like the layer Width/Height/X/Y fields. Drawing tools open with the grouped-shape
picker (`EditorShapePicker`, three 40-point columns) and the rail's Shapes button
opens the three-column flyout. Its Crop shows the Aspect ratio select, then the
dragged selection's read-only Width/Height with Clear and Apply crop, or the drag
hint; the selection comes only from a canvas drag. Both hosts use shipping's 320 px
sidebar column (AppKit panels start `--s-5` inside it; wgpu's panel is 320 points).
X11 smokes cover both appearances; AppKit is covered by XCTest only.
Both hosts expose a zoom preset menu with Fit, 50%, 100% and 200%. Its selected
value tracks custom percentages from steps, wheel and magnification; obsolete
custom rows are removed. Selecting a preset uses the existing shared viewport
math, cancels transient editing and does not enqueue document or output work.
Fit now uses Tauri's 2–100% scale range: small screenshots stay at actual size,
larger images use the limiting viewport axis, and manual zoom can still enlarge them.
AppKit retains centered placement and wgpu retains top-left placement inside their
existing viewport insets. Both hosts now connect a continuous logarithmic zoom
slider using shared Rust's shipping 5–800% mapping and tenth-percent rounding.
Fit supplies actual displayed scale, not the zero sentinel. Slider changes anchor
the viewport center and cancel transient editing without document/output work;
presets, wheel and shortcuts update the thumb. AppKit exposes the displayed percent
as its accessibility value description. This does not reproduce Tauri's full layout.
Physical trackpad/mouse behavior still requires platform acceptance.
Both hosts connect canvas fill/transparency through the shipping
`CanvasBackgroundPicker` card: a Solid background toggle and the compact
`ColorField` row (eight swatches from `captures_app::editor_chrome::colors` plus a
custom tile; wgpu opens an inline picker, AppKit the system color panel). Each toggle,
swatch or custom change submits one `set_background` worker transaction at once, as
shipping commits each change; an unchanged color adds no undo step. Changes made
while the worker is busy coalesce to the latest one. Turning Solid back on restores
the last solid color (initially `#f7f7f5`). The shared renderer composites beneath
existing layers. Color changes participate in undo/redo and draft reopen; copy/export
use the newly rendered pixels. These are canvas fills, not image-background removal
or text backgrounds. Stroke, fill and shadow colors in Layers → Annotation style use
the same swatch row and apply at once, as do selected text's Text color and
Background color and the drawing defaults' Stroke color (Color
for open tools) and Fill color; selected text shows shipping's five-column B, I and
alignment icon buttons (`editor_chrome::text_format`). Text style menus show the shipping
preview chips, preset labels use shipping title case (Mono Box, Rounded Box) and
the font menu lists Sans serif, Serif, Monospace and Rounded rather than pinned
asset names. AppKit Geometry scrolls to keep
the existing crop/canvas controls and new background controls reachable.
The shared image-background prerequisite now maps document clicks through image
rotation/orientation and supports contiguous/global magic-wand removal. It picks
the frontmost visible image even when locked; transparent pixels do not let the
wand reach an underlying image. Edited pixels become a fresh owned asset, retain
the first pre-edit source for future restore, and clear the solid canvas fill in
one undoable render-before-publish transaction. Invalid/no-match requests leave
history and assets unchanged; retained originals survive draft reopen. The existing
100-million decoded-pixel asset budget also counts retained edits/undo sources.
Both native Draw panels now bind Wand clicks through the existing viewport mapping
and serialized worker. Tolerance defaults to 36 on shipping's `RangeSlider`
(0–120 with 0/36/80/120 marks; values are the engine's 0–255 channel distance,
unscaled, as in Tauri); contiguous removal defaults on. Pan and off-image clicks do not submit edits.
AppKit's Draw panel scrolls at minimum size. X11 coverage exercises exact alpha,
disconnected-color global removal, locked images, no-match recovery, undo/redo,
draft reopen and copied PNG pixels; AppKit has bridge and rendered-state tests.
Windows/Wayland presentation and physical AppKit input remain unverified.
Physical acceptance remains open; these bindings do not complete the editor gate
or reproduce the shipping Tauri toolbar layout.
The shared brush prerequisite now accepts a completed erase/restore stroke with
document-space samples, brush diameter and softness. It locks the first visible
image, ignores later off-image samples, uses orientation-aware natural-pixel brush
scaling, and matches Tauri's pixel-center stamps, feathering, interpolation and RGBA
rounding. Changed strokes publish one undoable owned asset, retain the first original,
and clear canvas fill; no-op strokes preserve fill, pixels and redo. Restore reads
that retained original, including after draft reopen. Shared Rust/TypeScript vectors
check exact pixels. Both native Draw panels now connect Erase/Restore with shipping's
Size (28 px, 4–120) and Softness (18%, Hard/50%/Soft marks) sliders and hint copy. They sample press/movement/release into one
worker command, including stationary release stamps that affect soft-edge alpha.
Both hosts render live brush pixels on the serialized worker from the complete
gesture and published assets, with one in-flight render and one replaceable pending
request. Preview never publishes assets, history or drafts; late replies cannot
revive a cancelled gesture. The size ring remains visible; release applies one
stroke. Escape, focus loss, close, viewport or tool/section changes
cancel without editing. Pan and clipped/off-image initial presses never paint.
X11 tests cover cancellation, actual feathered alpha, erase/restore, undo/redo, drafts
and clipboard; AppKit has input/bridge tests and minimum light/dark/error fixtures.
Windows/Wayland presentation and physical AppKit input remain unverified; sampling
cadence remains parity work (the brush ring is described below).
Both hosts now draw shipping's `.screenshot-brush-cursor` from
`editor_chrome::brush_cursor`: over a visible image (and for a whole stroke once one
starts) the system cursor hides behind a ring sized `max(1, size × display scale)`,
a 1.5 pt white border with a dark halo outside and a faint dark line inside over a
4% white fill; Restore dashes the border over an 8% accent fill. Elsewhere on the
canvas the cursor is `not-allowed`; pan-ready (Cmd/Ctrl) or panning hides the ring,
and the ring is not clipped to the image. AppKit tests image cover with snapshot
selection outlines, as shipping's `hitTestImageElement`. wgpu has unit coverage and
the private-X11 brush smoke checks the ring's border and halo pixels; AppKit is covered
by XCTest only (macOS CI is its first compile). The dash pattern approximates CSS
`dashed`; physical cursor behaviour on macOS, Windows and Wayland is unverified.
Both hosts connect Geometry → Trim edges through a shared `trim_canvas` command.
It fits visible layer geometry, including locked/zero-opacity and off-canvas layers,
rounds bounds outward, and translates every layer including hidden siblings. Empty,
hidden-only and already-tight documents are no-ops that preserve redo and pixels.
It includes rotated image/shape/path bounds and annotation shadows, not an alpha scan.
Changed trims are one render-before-publish undo step; draft and output use the new
dimensions. Shipping TypeScript vectors cover fractional/rotated/shadowed geometry;
host tests cover controls, undo/redo, output invalidation, drafts and clipboard.
Trim hover-margin feedback and the shipping header toolbar arrived later (see the
editor chrome and final editor-parity slices).
Windows/Wayland presentation and physical macOS input remain unverified.
Both hosts connect the shipping Compress presets: Tiny (55), Smaller (70), Balanced
(85), High (92) and Highest (98). Presets derive the PNG palette (there is no
separate PNG colors control, as in shipping) and reuse shared encoding: Tiny–High try 32/64/128/256 colors (retaining lossless pixels
when that is smaller), Highest keeps exact pixels with lossless packing.
JPEG/WebP retain their lossy quality mapping. Custom
numeric quality remains available and labeled Custom when active. Preset changes
refresh the automatic comparison without editing documents, drafts or undo.
Both hosts connect Original/75%/50%/Custom output dimensions and custom aspect lock.
Shared Rust resolves percentage dimensions by rounding width first, then preserving
the document ratio. Requested resized output is limited to 16,384 pixels per axis
and 100M pixels. Worker-owned export pixels use premultiplied-alpha, sRGB Lanczos3;
this prevents invisible RGB bleeding into edges but does not promise pixel identity
with the browser's unspecified high-quality canvas filter. Original/equal-size output
borrows exact pixels. Save-new uses the same once-resized frame for export, History
dimensions, History PNG and thumbnail. Document/draft/undo and full-resolution copy
remain unchanged; option changes invalidate encoded previews. Windows/Wayland
presentation, physical macOS input and output acceptance remain open.
Both hosts' export bars now overwrite the opened screenshot's existing saved path
by default when the output format matches (see the export-bar paragraph below). The session pins that path; shared
Rust rechecks current History identity/path/type and file availability before encoding.
Sibling-temp publication replaces only that destination. Once-resized export, private
History PNG, thumbnail and dimensions update the same artifact ID/date, rather than
adding a copy. A post-publication History failure reports the saved path and warning.
Hosts dismiss that artifact's stale mini preview and reload History with fresh decode
generations after file publication, including partial success. Accepted writes drain
on quit. Document, pixels, draft,
undo/redo and encoded preview are retained. Undo does not revert the saved file;
discard reloads the current History image. As in Tauri, a new file saved with a
History entry becomes the editor's next overwrite target; drafts are not
flattened/deleted.
Validation at write start is not a cross-process compare-and-swap or a two-store
transaction: an external change during encoding is not locked out. The file and
History publication are separate, and a History failure cannot roll back a saved file.
Physical macOS/Windows/Wayland and full output acceptance remain open.
The shared text prerequisite uses `cosmic-text` advanced shaping and CPU Swash
rasterization for a single line from caller-supplied fonts, with fixed locale.
It returns logical advance, baseline, painted bounds and
straight-alpha pixels, including ligatures, combining marks, bidi ordering and
negative bearings. Glyph images are scoped to one operation, with line/size/pixel
limits and explicit missing-family errors. A glyph the requested face lacks falls
back, like a browser, to the other supplied faces in a caller-given family order
(no platform family lists, so covered text shapes identically everywhere); only a
line with a glyph no supplied face covers is reshaped with optional platform faces
(`PlatformFonts::System` scans the installed fonts once per process and never lets
them shadow a supplied family name), and a glyph nothing covers draws the requested
font's missing-glyph box. Original generated fonts give independent metrics for
tests rather than depending on installed fonts.
Color-outline and embedded-bitmap glyphs use different alpha representations;
the primitive normalizes outlines before compositing and retains bitmap RGB.
Swash's color-outline flattening has integer alpha-rounding loss. Font bytes must
come from a trusted source; output budgets are not a font-parser sandbox.
The single-line primitive also offers centered contour strokes with round joins,
preserving shaped advances, baseline, explicit faces and fractional glyph placement.
This uses scalable glyph paths, not bitmap dilation; colored and bitmap glyphs return
explicit outline errors. Filled and outlined masks cannot leak between operations
or stroke widths, and the existing raster bounds/pixel budgets still apply.
Both hosts now apply Outline live with the other text properties. Document rendering uses
Tauri's max(1.5, font size × 0.08) stroke width, including plates, shadows and rotation.
Outline-only edits preserve authored width/position, invalidate output transactionally,
and participate in undo/redo and saved reopen. Unsupported glyphs reject property
edits even on hidden text; accepted pixels and staged host input survive failures.
This does not establish physical input or platform acceptance.
Shared paragraph helpers now use explicit, fallible measurements for shipping word
wrapping, scalar-based hard breaks, ECMAScript whitespace, alignment, auto-width
anchor preservation and composing widths, plus square/rounded plate geometry.
The paint minimum and wrap minimum remain distinct for narrow/fractional boxes.
TypeScript-generated vectors and original-font tests exercise threshold boundaries,
non-additive shaping, Unicode, metadata preservation and measurement failures.
Measurement avoids glyph bitmaps and permits advances beyond the raster extent
so long tokens can wrap; rasterization retains its extent/pixel budgets. Paragraph
inputs are limited to 4096 UTF-8 bytes and type sizes greater than zero through 512.
These helpers describe measured paint layout, not Tauri's estimated selection bounds;
font-specific control-character support remains the supplied measurer's contract.
An opt-in document renderer now accepts caller-owned fonts and explicit mappings
from document family keys to supplied font names. It composites filled/outlined paragraphs
and square/rounded plates in layer order, including alignment, italic bearings,
per-paint opacity, blend modes and canvas clipping. It centers raster ink vertically;
pixel-aligned ink boxes can differ subpixel-wise from Canvas outline metrics.
ASCII tabs, carriage returns and form feeds become spaces for both measurement and
painting, following Canvas text preparation; remaining interior line-control
characters are rejected by the single-line shaper, not silently omitted.
Text bitmaps have a shared 16,777,216-pixel budget across the visible document,
in addition to individual line budgets. Missing font families, invalid styles and
budget failures return errors without changing the document or assets. The lower-level bitmap compositor
now supports a shadow/source pass using transformed pixel alpha, layer opacity,
canvas-space offsets, blur and blend mode. Shadow work is clipped to output plus
blur support; existing vector shadow/crisp passes are unchanged. Font-backed text
uses the shipping paragraph paint order: all glyph shadow/source passes precede
all crisp glyph passes. With a plate, only the plate receives a shadow. Both hosts
apply a Drop shadow toggle and custom color, opacity, blur and X/Y offsets live.
Both hosts use Rust-resolved defaults and submit only changed enabled
fields; saved precision and unknown style metadata survive toggling. Shadow-only
edits do not refit text. Failed transactions retain typed input, and values that
cannot apply yet (a partial color) are not sent. Legacy low-level `Shape::Text`
shadows remain unsupported.
Text and plate paints now share the
shipping selection pivot for rotation, including when estimated wrapping differs
from actual glyph layout. Shared selection/hit testing, move snapping, Trim and
resize accept text geometry. Fixed-width side drags reflow without changing type
size; other drags scale type, while auto-width labels refit. Interaction bounds
use shipping's UTF-16 width estimate and rounded minimum, not measured paint or
glyph bounds; plate and default/custom shadow padding are included. Shipping-generated
vectors exercise Unicode wrapping, fractional widths, side/corner classification,
8–512 resize clamps and metadata preservation. Session tests cover accepted pixels,
undo/redo and reopening after text transforms. Shared `create_text`/`edit_text`
commands now create plain, left-aligned auto-width labels with fresh IDs and edit
content, font family/size, bold/italic, alignment, color and square/rounded plates.
Content/type edits refit from owned-font measurements while preserving alignment
anchors; paint/alignment-only edits do not refit. Blank text keeps the shipping
eight-em composing field; fixed-width and legacy fields remain intact. Property
edits match shipping's hidden/locked-layer behavior and validate those layers too.
Missing families, control characters and invalid requests preserve frames, redo and saved drafts;
successful changes use the existing render-before-publish transaction. Font-face
matching retains the existing shaper's closest supplied face behavior; it does not
acquire missing faces. Both hosts now connect click-to-place Text, fresh-ID selection,
and staged multiline content, size, bold/italic, alignment, color and plate controls.
Apply uses one worker transaction; Cancel restores accepted values. Failed Apply and
unrelated responses preserve staged input; pending text must be applied/cancelled
before closing. Property-only changes do not resend unchanged typography.
This is sidebar typing, not shipping inline canvas composition or IME acceptance.
Editor sessions now accept explicit trusted fonts and own the shaper on their
serialized worker. Commit/crop/import/undo/redo and output use the same font-backed
frame; failed text renders preserve accepted pixels and history. Drafts store family
mappings plus immutable font sidecars, not raw bytes in JSON, within the existing
80 MiB image-plus-font save budget (including bounded full license notices in the
manifest). New native sessions use twelve unmodified Liberation Sans/Serif/Mono
2.1.5 static faces plus four unmodified Nunito 3.601 rounded faces (4,987,528
font bytes total, shared across workers), with complete OFL 1.1 notices in
native resources, `--font-license` output and text-bearing saved drafts. Image-only
drafts do not persist the worker's unused font set. No OS fonts are copied
and no network fallback occurs.
Shipping (`EDITOR_TEXT_FONT_STACKS` in `lib/screenshotEditor.ts`) draws the four
family keys with OS font stacks (system UI, Georgia/Times, SF Mono/Consolas,
ui-rounded/SF Pro Rounded) and stores only the key in its draft JSON; it bundles,
imports and persists no font files and offers no OS font picker, so neither do the
native hosts. Its browser falls back to other installed fonts for glyphs a face
lacks. The native equivalent keeps rendering deterministic for covered text:
a glyph the chosen face lacks uses the session's other pinned faces in Sans,
Serif, Mono, Rounded order, then the installed system fonts (read in place, never
pinned into a draft), then the missing-glyph box; it no longer rejects the text.
Nunito's regular cmap is narrower than Liberation Sans's (938 versus 2,327
code points): é, Ω and Ж render in Nunito, and Greek λ renders from Liberation
Sans. Editor preview, thumbnails, copy, export and reopened drafts share one
shaper, so they stay identical; a line that needs installed fonts depends on the
fonts present, as shipping does, and the hosts' inline composing field uses its
toolkit's own fallback until the text is committed.
Both hosts apply family changes live with the other text fields. Like shipping,
which offers all four families on every document, the family picker and style
presets read the session's pinned map plus the host's bundled families a reopened
draft lacks, so an older Sans-only draft offers Serif, Mono and Rounded (and the
Rounded Box new-text default). Choosing one, by Font, Style or placement, pins
that family's four faces and license notice into the session; the next draft save
keeps them. Offering alone pins nothing, pinned families and bytes never change,
and a draft that already embeds a bundled family name under another key keeps its
own. A failed edit may leave a family pinned for the session, which only adds its
unused faces to later saves.
Both selected-text inspectors offer a Style menu whose choice applies at once.
Rust supplies the shipping seven-style catalog filtered by the session's offered
font families: the bundle offers all seven styles, including Rounded and Rounded Box,
also for older Sans-only drafts, which pin Rounded when it is chosen. A draft opened
without the host bundle offers only its pinned families' styles; Rounded/Rounded Box
require an actual `rounded` face and are not substituted with Sans. Presets change only
family, plate/outline flags and (when no plate existed) the default plate color.
They preserve content, size, alignment, traits, text color, custom plate colors,
shadows and unknown metadata; the usual worker edit/refit rules still apply.
Shipping-TypeScript fixtures check the catalog; live failure handling and
font filtering are covered separately in host/session tests. This is not the
shipping style-picker layout or a new-text-default picker.
Reopening prefers the saved font set over host
defaults (which only add unpinned families); missing/corrupt fonts return errors
without silently substituting or deleting the draft. Font cleanup follows successful manifest publication; the
existing image save order is still per-file atomic, not a whole-draft transaction.
Discard removes the draft and restores the capture while retaining the worker's
font capability. Original generated-font tests cover exact restored/exported pixels,
changed defaults, failure recovery and storage limits. Light/dark private-X11 tests
restore a seeded font-backed draft, select/move/resize/quarter-turn it with undo,
save/reopen it and verify clipboard ink/plate pixels (eight checks per appearance);
transformed and minimum-size captures were inspected. These synthetic-font
fixtures do not provide Text input controls or establish macOS, Windows, Wayland
or physical Text-tool presentation/input acceptance.
The basic Text tool is implemented in AppKit and wgpu; host verification is recorded
per slice, not inferred from shared tests. Font import and OS font selection are not
shipping features; physical input/IME/accessibility remain open.
Both hosts now offer new-text style and size (8–512) before placement.
Like shipping, the Text section also shows the drawing defaults' Drop shadow:
one shared toggle and custom style, whose untouched fields scale from the new text
size (`editor_text::new_text_shadow_style`, `text_default_shadow` on the AppKit
chrome ABI). `TextCreate` carries `dropShadow`/`dropShadowStyle`, clamped by the
shared resolver, so each placement copies them as `createPlacedTextElement` does.
As in shipping (`createPlacedTextElement` takes `defaultStyle.color`), there is no
separate new-text colour: the Text section has no Color row, and each placement
uses the drawing defaults' one shared Color (Color for Arrow/Pen/Line, Stroke color
for closed shapes), starting at annotation red `#ff3b5c`.
Choices are per-editor UI state, not document/draft/undo or settings; accepted
responses and failed creation retain them and a new editor starts from the
defaults again, like shipping's `useState`. Both hosts' new editors start at Rounded Box when the
snapshot offers it, otherwise Standard, then Plain. Shared Rust
supplies Tauri's initial size: 5.5% of the original capture's shorter side,
rounded and clamped to 24–72. It uses History dimensions,
not the resized/cropped canvas of a restored draft; later user choices remain
unchanged across editing responses. Plain retains the explicit saved-family path for custom-font
drafts. Presets come only from pinned or pinnable fonts; Rounded is not substituted. Shared
Rust validates the chosen preset and creates boxed text centered at the click using
the eight-em composing width, retaining the anchor when content later refits.
Placement is one render-before-publish transaction with fresh selection and normal
output invalidation; invalid/unavailable styles preserve pixels, redo and drafts.
The initial style now matches shipping Rounded Box where the saved font set permits
it, but typography and inline composition do not reproduce the Tauri layout.
In both hosts, an explicit selected-text named Style choice also sets that offered
preset for future new text in the same editor, even if Apply fails or the selected
edit is cancelled. It does not copy selected size/color/traits or manual family,
and a later new-text Style choice wins. Selection, snapshots, undo and reopening
do not carry this choice.
AppKit now starts an on-canvas native multiline responder when Text places a new
layer or hits an existing visible, unlocked text layer; double-clicking such a
layer from Select starts the same transaction. Like Tauri's
`.screenshot-inline-text-frame`, an `NSTextView` sits on the canvas in the layer's
bundled face (registered from `captures_editor_chrome_v1` `text_face`), size, colour,
opacity, outline stroke, plate, padding and alignment, and `frameCenterRotation`
turns it with the layer; `inline_text_layout` supplies the shared frame. The
session preview omits that layer while it is typed. The responder retains local typing,
selection, clipboard and marked-text ownership while shared preview rendering is in
flight, coalescing replacements to the newest buffer. Return inserts a newline;
Escape or clicking away commits one undo step, as shipping has no Done/Cancel.
Blank new input is discarded and blank existing input removes the layer; after a
failed Begin, Escape retries and a cleared box is dismissed. Save, copy, import and unrelated document actions remain blocked until
the transaction resolves. Begin/update/finish failures keep retryable input, and
close/quit drain accepted work, preserving the latest commit buffer before draft
handling. TextKit line layout approximates CSS line boxes; blend modes and a
draft's own non-bundled font (which falls back to the system face while typing)
remain parity work. Only seeded test drafts carry non-bundled fonts, since neither
shipping nor the native hosts import fonts. Existing inspector styling remains staged outside active composition. Automated macOS
fixtures cover light/dark normal, 760×540 and failure states, but physical macOS
IME, VoiceOver, keyboard layout and mixed-scale acceptance remain unverified.
No host text parity gate is closed.
The Windows/Linux candidate now connects a multiline on-canvas composing field
to the shared transient text transaction. New placement and existing Text-tool hits
retain a local typing buffer while one worker update fits/renders at a time.
Escape, outside clicks and close finish one undoable edit (shipping has no
Done/Cancel). The box is painted in the layer's transform: bundled face, size,
colour, opacity, plate, padding, alignment and rotation, with the accent outline
`--s-3` outside it, while the session preview omits that layer. Rotated labels
rotate their glyphs, plate and caret, but egui keeps the selection highlight and
pointer caret placement unrotated; outlined labels draw filled glyphs. Empty new
text creates nothing; empty existing text removes that layer, and a cleared box
after a failed Begin is dismissed. Quit drains the latest buffer before draft
saving, and failed updates retain it for retry. Output actions cannot publish
unfinished pixels.
Private X11/software-GL exercises are implementation evidence, not Windows,
Wayland, physical input, IME or accessibility acceptance. AppKit composition is a
separate host slice. This does not close screenshot-editor or visual parity.
Next implementation boundary: text and remaining output. The shipping Tauri editor remains the design
reference; this slice does not reproduce its layout or live pixel dragging.

### Recording editor: first wgpu host, not playback parity

History's **Edit recording** resolves the selected artifact through the shared
`RecordingEditorSession`, probes retained media and opens a separate window with a
decoded frame, source-relative scrubbing, numeric trim/crop, custom output size and
a fixed save bar. The wgpu trim row also has graphical start/end grips. Shared
`recording_timeline` geometry preserves the pointer-down offset, waits for three
logical pixels of movement and keeps at least one millisecond selected. Far-out
pointer glitches retain the last accepted sample; valid motion recovers from the
original origin. Release, Escape, lost pointer/focus and layout changes end the
gesture without rolling back staged values. Handles accept focused arrow keys
(1 ms under 60 seconds, otherwise 10 ms) and Page Up/Down (1 second). They never
decode or publish media during drag: numeric values and the range update together,
and, as in shipping, the edit applies live once the drag ends (see live edits below).
Until the edited preview decodes, pending trim gates seek, estimation and save. The wgpu track now
displays the shared 12-frame full-source thumbnail strip, center-cropped vertically
to the compact row. Excluded ranges are dimmed and grips retain the same hit regions.
Generation runs once on the serialized worker after open, with independent cancel
and retry; failure leaves editing available. Edits/seek retain the source strip and
never regenerate it or change the accepted preview. Close/quit waits for generation.
The wgpu host also offers Play/Pause through a persistent shared FFmpeg
decoder capped at 30 fps and 1280 × 720. Motion frames retain the accepted spatial
edits but stay separate from session state, dirty identity, estimates and History.
A single latest-frame slot bounds pending UI work; no timer remains after stop.
Pause retains the last presented source position, EOF replays from accepted trim
start, and failures restore the accepted still. Focus loss/minimize requests Pause;
close cancels and waits for teardown before the normal unsaved-edit confirmation.
Seek/edit/save/estimate remain gated while decoding. Loop preview defaults off;
it can change while playing without changing accepted edits, estimates or History.
Looping is gapless like shipping's `<video loop>`: one shared playback stream
(`looping_playback`, FFI `playback_open_v3`) serves every lap. While a lap plays it
pre-rolls the next lap's decoders at the accepted trim start, then continues on one
presentation timeline, so the wrap costs one frame interval (about 33 ms at 30 fps,
measured by the shared FFmpeg unit test) instead of a decoder reopen. Video and audio
agree on one continue/stop decision per lap: the flag is read at the lap's video EOF,
or up to about 80 ms (plus device latency) earlier when audio must commit the next
lap to avoid an underrun. Turning Loop off finishes the current lap; a lap that
presented no frame never restarts; Pause, close and failure never restart. Each new
editor defaults to one pass. AppKit's Loop control uses the same stream (see below).
Both hosts implement Sound, which defaults off per editor and can change only when
the worker is idle, not during playback or Pause teardown. Opt-in Sound uses the
shared accepted-mix audio API; silent v1 remains unchanged. One metadata event per
operation reports whether audio is actually enabled. GIF/no-track/muted/zero-gain mixes use silent playback
without a device. Audible MP4 uses the default output device; device failures remain
visible and require an explicit Sound-off retry to play silently. Looped Sound keeps
one output device, ring buffer and audio clock across laps; the next lap's PCM follows
with 4 ms fade-out/fade-in edges at the loop point, so neither an underrun gap nor a
waveform step clicks, and lap lengths (and A/V sync) are unchanged. The private-X11
`--sound` smoke measures consecutive lap onsets on the virtual sink (lap plus gap) and
`--playback` measures the recorded 500 ms trim-end run before a real wrap (locally 0 ms
audio gap; 467–600 ms trim end at the 15 fps capture's resolution). Sound survives edits/Seek/Pause/errors but
does not change edits, estimates, dirty identity, exports or History. Private-X11
checks capture real CPAL output through an isolated PulseAudio sink, not physical
speakers. AppKit host tests exercise the same v2 metadata and lifecycle contract;
physical A/V-sync/device acceptance remains open.
Raw-input tests exercise multi-pass delivery, keyboard focus,
thresholds, cancellation and busy gates; private-X11 tests cover staged values,
thumbnail loading/cancel/failure/retry and temporal pixels, exported duration/colors
and immutable source. Windows presentation and physical macOS/Windows/X11/Wayland
input/accessibility remain unverified; no parity gate closes.
Crop uses source-pixel coordinates. Numeric crop dimensions start
aspect-locked, follow the current crop ratio and fit the remaining source bounds;
unlocking permits independent dimensions, and relocking uses the adjusted ratio.
The wgpu host's explicit **Adjust crop** mode lazily loads a full-source still at
the accepted source position, independently of the accepted cropped/output preview
and paused motion frame. Eight handles and interior move use shared source-pixel
geometry and the current aspect lock; arrows nudge one pixel, Shift ten. Release,
Escape, focus loss and layout changes end the gesture without reverting staged
values. Loading is cancellable/retryable; the still is cached until accepted seek
changes position. Crop gestures apply live when they end while Adjust crop stays on
the cached source frame; **Done cropping** shows the accepted (edited) preview, and
playback is gated during adjustment. This host path
is shared by Windows/X11/Wayland; automated real-media interaction is exercised on
X11, not physical Windows/Wayland acceptance. The AppKit host path is described below.
Typed dimensions commit on Enter/focus loss so partial input does not change the
ratio. The lock is an input preference, not an export edit. Custom output width/height
remain independent (no output aspect lock). Original, 1080p maximum and 720p maximum presets
reuse shared `MaxResolution::constrain`: cap height without upscaling, preserve
the current crop's aspect ratio and round to even pixels. Presets stay selected
after edits and seeks so later crop changes recompute the dimensions; Custom overrides
the preset and disabling Custom restores it. These values apply live together with
format/quality and preview the accepted export configuration.
Shared GIF encoding honors both explicit dimensions, including square-pixel aspect and proportional
size-budget retries, instead of silently ignoring output height. Re-encoded MP4 on
Windows/Linux fits within 3840 × 2160 (portrait: 2160 × 3840); format-aware preview
now reflects that cap without applying it to Preserve copy/remux or GIF paths.
The accepted format/quality and frame dimensions appear beside the preview.
Both native recording previews have display-only **Fit / 100%**. Fit retains their
existing scaling; 100% maps each decoded pixel to one logical screen point and
scrolls overflowing pixels inside the preview. Smaller images stay centered.
This applies to accepted, motion and crop-source frames without media I/O, edits,
estimate invalidation or History changes. Edits and seeks retain the mode; another item
defaults to Fit. Crop gestures use the scrolled image rectangle and end on scroll,
scale or layout changes. Motion remains capped at 1280 × 720 regardless of display
scale. Physical AppKit input/accessibility and Windows/Wayland/mixed-DPI acceptance
are still open.
Available system/microphone tracks have 0–200% volume, an include (unmute) checkbox
per track and a Convert to mono control. Availability comes from the accepted session's trusted audio
identity, not caller-provided track flags. Audio stages with geometry/format and
uses the same live-edit/save/dirty guards; failed updates preserve staged controls
and accepted output state. Accepted audio also feeds opt-in Sound preview. GIF replaces
the audio rows with the shipping note while keeping settings for a later MP4 export.
No-track recordings show no Audio card. Private X11 smoke uses
distinct stereo tones in a retained playback mix plus separate system/mic tracks,
then measures decoded export frequencies/amplitudes, mono channel count, mute,
GIF silence, restored MP4 settings and History audio identity.
Both native hosts offer 8/10/12/15/20/24/30 GIF FPS (default 15), applied live
through the accepted export boundary. It participates in save,
playback, estimate and dirty guards, survives an MP4 roundtrip without modifying
MP4 cadence, and resets for a new item. A failed live edit retains both the accepted
frame and the staged correction, and is not retried until edited again. Private X11 light/dark coverage exports 24 and
72 frames over the same three-second source at 8 and 24 FPS, checks duration,
dimensions, colors, source/History immutability, failure/retry and minimum layout.
AppKit real-media coverage exercises the same asymmetric trim at both cadences;
physical macOS/Windows/Wayland acceptance remains open.
Both native hosts offer shipping-compatible GIF maximum widths of
320/480/640/800/1200 pixels (default 800). The cap applies after crop and
preset/custom output sizing, never upscales, and always recomputes from the
independently retained MP4 base instead of compounding an accepted GIF reduction.
Live-edit/save/seek/failure/dirty/new-item behavior stays on the existing boundary.
Private X11 light/dark coverage saves 800/1200/320 px GIFs from a 1600×900 source,
restores 1600×900 MP4, and checks source/History immutability. AppKit CI exercises
the same sizing lifecycle and real GIF dimensions. Physical acceptance remains open.
Both native hosts map their GIF quality choice to the shipping palette
limits: Tiny 64, Smaller 96, Balanced 128 and High/Highest/Preserve 256 colors. No separate
palette control is added. Maximum uses the remembered quality for the palette while
forcing Preserve export quality; MP4 omits the GIF field without losing the choice.
Private X11 exports a high-color source at Tiny and High and checks decoded colors,
live-edit/save gating and source/History immutability. AppKit CI distinguishes 64- and
256-color saved GIFs while retaining the high-color edit-preview pixels, including
a failed live edit and accepted preview/save identity. Physical macOS/Windows/Wayland
verification remains open.
Format/quality edits apply live alongside geometric edits and gate save and seek
until their preview decodes; failed updates retain all accepted state and preserve
staged values for correction.
Save uses the accepted configuration, and format/quality-only changes require
save or explicit discard. Both native hosts' **Maximum file size** controls accept a decimal
KB/MB/GB cap of at least 100000 bytes through the shared v2 `save_export` contract.
Maximum mode uses Preserve quality, shows the typed cap (or shipping's "—" while it
is invalid) instead of sampling an estimate, and keeps the previous quality
preference for leaving maximum mode.
Changing units preserves whole bytes; invalid/partial input never applies and gates playback,
seek and save. Budget-only changes participate in accepted/dirty identity.
Still and motion previews use the budget-free `preview_export`; a visible warning
explains that fitting retries may lower resolution, cadence or audio quality.
Failed/cancelled/unattainable saves leave source, accepted state and History intact.
Private X11 covers capped MP4, a real GIF retry with different saved dimensions,
unattainable export and light/dark normal/minimum controls. AppKit CI covers the
same accepted-save lifecycle, real capped outputs and native rendered states;
physical macOS/Windows/Wayland acceptance remains open and no parity gate closes.
**Live edits.** As in shipping, there is no **Apply edits** button: trim, crop,
resize, format, quality, GIF and audio edits take effect as they are made. Shipping
renders edits on a live `<video>`; natively the edited preview decodes on the
serialized worker once staged values settle for the shared
`recording_editor_ui::LIVE_APPLY_DELAY_MS` (250 ms): after a drag or held pointer is
released, and after typed values commit on Return or focus loss. The existing guards
remain: edits wait for decoding, encoding and playback, invalid values never apply,
and a failed edit keeps the accepted frame and the staged correction without
retrying until the user edits again. Pending edits gate seek, playback and Save
until the preview decodes. Accepted-versus-saved dirty state, History and the
close/quit discard guard are unchanged.
**Est. size** is automatic, as in shipping: the shared Tauri estimator runs on the
accepted edit/export configuration `recording_editor_ui::ESTIMATE_DEBOUNCE_MS`
(600 ms, shipping's debounce) after the accepted settings settle, once per identity,
on the serialized worker. There is no **Estimate size** button and no Cancel for it:
newer edits, Save, Play, Seek, source crop, thumbnails and close supersede and cancel
a running estimate, which then runs again for the new settings. Copied bytes and
fully encoded short ranges report exact byte counts; longer sampled ranges and
audio-only Preserve changes are marked approximate. Shipping's labels come from
`captures_app::recording_editor_ui::estimate`: "Estimating…" before the first
value, the previous value (muted, without a percentage) while a newer estimate is
pending, "—" for failures (no error message), WebM and an invalid Maximum, and
"≤ <cap>" for a valid typed Maximum. Estimation creates no History entry and never
marks unsaved edits as saved. No estimate promises a byte budget. Both native hosts
display nonzero percentage change versus immutable source bytes beside an estimate,
preserving exact/approximate meaning. They follow shipping rounding (including
negative half ties), hide unknown/zero baselines and rounded-zero deltas, and
suppress the percentage while pending, failed, WebM or Maximum. Seek (including
failure) retains the result; new items reset it. wgpu unit tests and AppKit CI cover
the debounce, supersession, pending, failure and cap states; the private X11 smoke
checks automatic sampled/exact labels at normal and minimum sizes without publishing
an estimate to History. Physical macOS/Windows/Wayland acceptance remains open.
Close/quit waits for accepted work, as with export.
The shared recording comparison ABI retains independent before/after frames from
a read-only encoding sample at the accepted source-relative position. As in
shipping, both native editors show the comparison automatically while Compress or
Maximum is accepted and playback is paused on the accepted still: 350 ms after the
accepted identity settles they encode a sample on their serialized workers and
draw `captures_app::compression_compare`'s split (Before/After size badges with
"% smaller", a draggable handle and **Hide**; **Show before / after** in Save quality
brings it back). Cancellation and failures stay in the comparison frame and are not
retried until Hide/Show or a new accepted identity. A paused transient playback
frame never selects the comparison frame. Generation, cancellation,
accepted revision, position and preview export guard delivery. Staging, playback,
crop, seek, new item and close discard comparison without changing accepted edits,
dirty state or History. Both hosts' splits take pointer dragging and the shipping
range keys, and Preserve recentres them. Maximum displays the budget-free first
attempt and warns that final capped-save pixels can differ. Requested/fallback seek positions are not decoded
PTS; output cadence can select neighboring frames. Physical macOS, Windows and
Wayland input, accessibility and mixed-DPI acceptance remain open.
Both hosts now render the shipping editor's page: an **Edit recording** (or **Edit
GIF**) header with the dropped-frames caution when the source lost frames, a Preview card whose toolbar holds Sound, **Loop preview** and a
Fit | 100% segment above a sunken viewport with an accent overlay Play/Pause circle,
then a timeline card (range summary, "… selected", filmstrip with dimmed exclusions,
accent grips and a playhead; clicking the track seeks; Start/End fields, **Reset trim**
and the position control), **GIF settings** (Frame rate, Maximum width), **Crop & size**
(X/Y/Width/Height, Lock aspect ratio, Adjust crop, **Output resolution** with
"Original — W × H", 1080p/720p maximum and Custom), **Save quality** (Quality mode:
Preserve quality, Compress or Maximum file size; the Tiny/Smaller/Balanced/High/Highest
preset; Est. size with a green/red delta pill) and **Audio** (System audio/Microphone
checkboxes that include a track, 0–200% volume, Convert to mono; GIF output shows only
"GIFs do not include recorded audio."). The fixed save footer has Filename, "Saving to
<folder>" with **Change…** (a folder picker), the filename field with its attached
.mp4/.gif/.webm format, the status line, a thin progress bar, a Cancel named for the
running operation, **Show in Folder** after a successful save, a **Save as new file**
switch and **Save**. Shipping copy and formatting come from
`captures_app::recording_editor_ui` (AppKit: `captures_recording_editor_ui_v1`): titles,
`formatEditorTime`, trim summary, `formatFileSize`, the Est. size states and delta,
the dropped-frames warning (from the snapshot's additive `dropped_frames`),
stage labels, saved messages, filename validation and every menu's labels and
descriptions. As in shipping, Preserve quality is offered only for MP4: choosing GIF
moves Preserve to Compress at the remembered preset (Highest by default), while an
accepted Preserve GIF keeps showing its mode. WebM hides Preserve and the Audio card
as shipping does.
**Save** follows shipping's semantics. The footer starts on the original's folder
and filename (shipping `recordingUserFacingDefaults`: the permanent save, never
private recovery media; a History-only recording is named
`Captures_YYYY-MM-DD_HH-MM-SS_mmm` from its capture time). With **Save as new file**
off, Save overwrites the original and its History item at once, without a
confirmation. Turning the switch on names the copy `<original>-edited` (shipping
`recordingEditedFileStem`) while the name and folder are still the original's, and
turning it off restores the original name. Choosing another format turns it on and
names an `-edited` file beside the original. Natively the switch is also locked on,
with the `-edited` name, when the source has no replaceable original (a reference or
a History-only recording): shipping would overwrite that file or promote recovery
media, which the shared native replacement does not do. After a successful save,
Save stays disabled until an edit, the name, folder, format or switch changes, as
shipping's `alreadySaved` does; the saved toast (`Video saved — <size>.`) and
**Show in Folder** follow both copies and replacements.
**WebM** is offered as in shipping. Shipping's bundled FFmpeg has no libvpx, so its
WebM export fails; natively the accepted preview keeps the MP4 settings, Est. size
shows shipping's "—", the comparison is hidden, and Save shows shipping's error
("media processing failed: WebM export is not available in the bundled media
tools", `recording_editor_ui::WEBM_EXPORT_ERROR`) without encoding.
Deliberate native differences remain: the edited preview decodes after a short
settle delay rather than on a live `<video>`, renaming or moving while overwriting is
not supported (Save with the switch off always replaces the original at its path),
and AppKit keeps a Position slider where wgpu has a Position (ms) field with Seek.
Physical audio playback acceptance remains open.
One worker serializes media operations; failed seek/edit preserves the accepted
frame, and values that have not applied yet gate scrubbing/export. Failed edits keep
the staged values available for correction. MP4/GIF Save new copy uses
shared encoding, reports progress and accepts independent cancellation. It never
replaces an existing file or the original History artifact. Post-publication
History failure reports the successfully saved path rather than inviting re-export.
Both native editors replace the opened session's exact permanent MP4/GIF path
(**Save** with **Save as new file** off) without a confirmation, as shipping does.
Saved-path/format UI hints are not eligibility proofs:
shared Rust verifies matching regular permanent and private recovery files and
source identity. Serialized work reports progress and accepts cancellation during
preparation; committed success can follow a late cancellation. Success rebases the
accepted position/edit/export, clears source-dependent frames, comparison,
estimate and thumbnails, regenerates thumbnails, and reloads the existing History
item. Ordinary failure preserves accepted state for retry; a `requires_reopen`
failure disables media until close/reopen. Physical acceptance remains open.
Close blocks accepted work; unsaved edits require explicit discard, and normal quit
is refused until they are saved or closed. Like shipping, the recording editor keeps no
edit drafts; shipping's `RecordingDraftManifest` is capture recovery, which both hosts have.
Both native workbenches list interrupted native capture bundles in a bounded History
section separate from artifact rows. Recover/Discard use the shared per-root lease,
expected identity, serialized worker, and an inline **Discard permanently?**
confirmation on the row (the second press discards; Escape disarms), replacing the
earlier modal.
Unavailable or corrupt entries are read-only; cancellable preparation leaves the
bundle intact and late cancellation cannot hide committed success. Recovery refreshes
History and opens the recovered recording only if selection is still current. Terminal
recording sessions retire before listing or preparing another take, releasing the
lease without discarding retained media. Quit waits for recovery/discard to finish.
This is limited to isolated native development roots: no installed-data migration
or Tauri recovery change. Linux X11/software-GL input verifies real media,
confirmation/cancel and History publication failure/retry. Windows shares that host
code; physical Windows and Wayland runtime/input/accessibility remain unverified.
AppKit CI covers lifecycle and rendered fixtures; physical acceptance remains open.
No parity gate is closed.
The shared Replace original operation requires a regular permanent MP4/GIF outside
History with byte-identical private recovery. It stages edited media, publishes the
permanent path atomically, then updates History; a History failure restores the
permanent path from intact recovery or requires reopening an indeterminate session.
The read-only original-save-path accessor supplies the accepted session's path
for host confirmation; it does not claim replacement eligibility or alter v1/v2
snapshots, and publication revalidates the opened metadata and file identity.
The old recovery bytes remain available during publication, but the two directories
are not crash/power-loss atomic: a process kill can leave new permanent media with
old or hidden History. History-only and reference-only recordings are unsupported.
Shipping's recording editor has no edit drafts or undo, so neither is a parity gap.

Platform status: shared Rust/C ABI is connected to both hosts. The first AppKit
slice opens recordings from History in a separate native window with retained
decoded frames, source-relative seek, numeric trim, MP4/GIF/WebM format and quality,
automatic size estimation, progress/cancel, collision-safe Save new copy and dirty close/quit
guards. AppKit's graphical trim handles use the shared allocation-free geometry and
only stage the existing numeric values; pointer movement never seeks or decodes, the
edit applies live once the drag ends, and estimate/save gating is unchanged. AppKit now also stages independent volume
and mute for trusted system/microphone tracks plus mono output in that same atomic
live-edit flow. GIF disables audio controls while retaining MP4 values, and the decoded
frame preview remains explicitly silent. AppKit also stages source-relative numeric
crop and Original/1080p/720p or independent custom output dimensions through the
same atomic live-edit flow. Aspect-locked crop dimensions and resolution presets use the
shared allocation-free geometry; the lock remains UI-only, and Original omits explicit
output dimensions. AppKit's trim row also shows the shared fixed 12-frame full-source
thumbnail strip. Generation runs once after open on the serialized worker, is retained
separately from accepted edited frames, and has independent loading, cancel, failure
and retry states; failure leaves the rest of editing available, while accepted work
keeps the existing close/quit gate. Seeking and applying edits do not regenerate the
strip or turn thumbnail clicks into a new seek gesture. AppKit also provides
Play/Pause of the accepted trim and spatial edits, with accepted-mix Sound on by
default like the shipping unmuted `<video>`; the Sound and ↻ Loop preview pills can turn
them off or on. A transient Loop control can
repeat nonempty trims without changing accepted edits, exports or dirty identity. It
passes a shared Rust Loop flag to the gapless `playback_open_v3` stream, so laps keep
their decoders pre-rolled and one audio device; no Swift toolchain verified this slice.
Persistent bounded FFmpeg playback delivers retained latest frames and a source-relative playhead without
mutating the accepted frame/position, dirty state, History or source. Pause, focus loss,
minimize, close, item switching and quit retain cancellation through decoder teardown;
errors restore the accepted still preview. AppKit's **Adjust crop** mode lazily decodes
and caches one immutable full-source frame at the accepted source position. Eight
resize handles and interior movement call the shared source-pixel crop geometry and
stage the existing numeric fields without per-pointer decoding or publication. The
overlay maps top-down source coordinates through letterboxing in AppKit's flipped view;
crop gestures apply live when they end while the source frame stays shown, and Done
shows the accepted (edited) preview. Source loading has the existing serialized cancel, close,
item-generation and retry guards. Sound-selected GIF/no-track/inaudible mixes stay
silent without opening a device; default-device failures remain visible for retry.
Its display-only Fit/100% control uses the currently decoded accepted, motion or
crop-source frame without a new decode. At 100%, one decoded pixel occupies one
logical point inside a bounded two-axis native scroll view; smaller frames remain
centered. Edits, Seek, Pause and frame delivery retain the item-local mode, while a
new History item defaults to Fit. Scrolling, scale changes and layout changes end an
active crop gesture, and crop mapping uses the exact scrolled image rectangle.
Windows/X11 implement the same edit controls through wgpu; private X11/software-GL
exercises provide implementation evidence only. Windows and Wayland presentation,
physical macOS input, accessibility, playback audio, physical audio output, draft
restoration and physical original-replacement verification remain open. No
recording-editor or cross-platform parity gate closes.

### Update notice surface: stub status source, no updater

Both native hosts now render the shipping update notice: a solid
`--surface-raised` card in a transparent, always-on-top window with a CSS-style
triangle caret toward the tray/menu-bar icon. Shared Rust owns everything both
hosts show. `captures_app::update_notice` ports the Tauri status model, the release
note parser, stacked notes with PR links, size formatting, per-state copy, card
heights and Escape blocking. `captures_app::tray_notice` ports the tray placement,
including caret edge/offset and fallbacks. AppKit reaches both through
`captures_update_notice_request_v1`. The covered states are Update available
(stacked or single notes, Hide / What’s new, Update now or View release,
open-captures warning), Downloading with progress, the restart countdown
("Updated" / "Reopening in N seconds…"), Update failed with Try again and the
download-page link, Checking and Up to date. Escape and Later/Close dismiss unless
the notice is busy. Hide / What’s new writes `show_update_changelog`.

**There is no native updater.** The notice is reachable only from workbench
fixtures (`--scene update --update-state …`), driven by a deterministic stub that
simulates install progress and the restart countdown. The stub never downloads,
verifies, installs or relaunches. Pull request, release and download links are
reported as fixture events, not opened. The tray/menu bar has no Check for
Updates item, and Preferences keeps its disabled placeholder. Signed updates,
installers and rollback remain distribution work. Private X11 checks
(`apps/native/x11_update_notice_smoke.py`) cover rendering, resizing and fixture
input. AppKit has XCTest coverage and compiles in macOS CI only. macOS, Windows
and Wayland presentation, placement at a real tray icon, focus and accessibility
are unverified. The Notices/updates gate stays open.

All **19 end-to-end acceptance gates remain open**. The large remaining workstreams
are screenshot editing, recording editing, Tauri visual/interaction parity, OS/workflow
integration, physical cross-platform acceptance, and renderer/distribution/cutover.
This is not a near-release checklist or a percentage-complete claim: implemented
features still need acceptance, and the native editor inspector still differs from shipping.
Shared commands and encoding remain prerequisites, not native editor/output acceptance.
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
| [ ] Recording editor | Playback/seek, timeline thumbnails, trim/crop/resize, audio/quality controls, size estimate/comparison, MP4/GIF export, explicit unsupported WebM export state, cancel/error/retry, recover drafts | `RecordingEditor.test.tsx`, `lib/recordingEditor.ts`, `crates/captures-media` |
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

## Mini-preview sharing integration — required, not implemented

The primary desktop cloud flow is capture → mini-preview Share icon → native
upload/share-settings popup. Track this as a separate cross-platform slice even
if the accounts/API PR or rewrite merges first; neither merge completes this
feature. API/web implementation: [#613](https://github.com/joswayski/captures/pull/613).
Do not ship a decorative Share action or substitute a website handoff for the
native flow. Local capture remains signed-out and never uploads automatically.

- [ ] Launch from the selected mini-preview artifact with the Lucide Share icon
  and an accessible name. Preserve its identity: an editor's Save new copy is a
  different local artifact, not an implicit replacement for the original upload.
- [ ] Signed-out users enter email and OTP in native controls; retain the selected
  artifact/settings through sign-in. Shared Rust owns account/session state and
  uses explicit bearer transport; OS credential vaults persist tokens, never
  plaintext preferences. Canceling sign-in leaves the local capture untouched.
  The unconnected `captures-account` prerequisite covers explicit request/verify,
  account lookup, bearer persistence/retry, invalidation and logout with platform
  vault adapters. Host controls, artifact retention and physical-vault acceptance
  are still open on macOS, Windows, X11 and Wayland; this does not check the gate.
- [ ] The popup previews the selected file and offers link access, optional
  password and expiry before explicit Upload and share. No upload merely from
  opening the popup. Existing API semantics are anyone-with-link plus optional
  password, not an authenticated recipient ACL. Fully public discovery/indexing
  is a separate unresolved product option, not an implemented visibility mode.
- [ ] Shared Rust uploads original bytes directly through the API's presigned
  multipart R2 contract, with progress, cancellation, expiry-aware part retry and
  failure recovery. Never show a usable share link before upload completion and
  successful share configuration; configuration failure must not re-upload bytes.
- [ ] Reopening manages the existing remote asset/share rather than duplicating
  the upload. Persist the local-artifact/remote-asset association. Show shared date,
  Copy/Open link, editable/removable password and expiry, and adjacent Share/Stop
  sharing actions. Stopping denies subsequent access; enabling again rotates the
  link. Cloud Trash retains bytes and restore does not revive old links.
- [ ] Integrate both AppKit and wgpu through thin host launch/presentation seams;
  coordinate MiniPreview/Workbench and mini_preview/live changes with the rewrite
  integration owner. Do not fork the auth/upload rules into platform hosts.
  Resolve the stable artifact and reject stale preview actions; an accepted upload
  outlives preview dismissal under the sharing coordinator's own lifecycle.
- [ ] Verify signed-out, expired-session, offline, missing-file, upload failure,
  cancel/retry, password edit, stop/re-enable and reopening states. Record macOS,
  Windows, X11 and Wayland implementation/verification separately; no stub or
  software-only host test closes the parity gate.

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
  shortcuts, unbinding overlapping OS screenshot keys like shipping (unit-tested
  only; not yet checked on a physical macOS, Windows, GNOME or KDE session).
  Update installation is not connected yet.
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
`captures-feedback`. Like shipping, both hosts open it in its own Send Feedback
window (About → Open or tray Send Feedback…) with `Feedback.tsx`'s layout, copy,
category cards, placeholders and limits shared through `captures_app::feedback`
(AppKit reads them through the feedback bridge's `copy` operation). The form
displays its app/system context before Send, permits
an optional contact, blocks duplicate submissions, and retains drafts after errors
or closing/reopening. Submission runs separately from capture/settings workers;
fixtures cannot send. No captures, files, or crash diagnostics are attached and
no startup network request is introduced. This advances the manual feedback slice,
not automatic crash reporting or full accessibility/physical-platform acceptance.
The window layout is verified on private X11/software GL (`x11_feedback_smoke.py`,
dark and light); the AppKit window relies on CI XCTests, and Windows and Wayland
remain unverified.

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
invalid-state, missing-device and reopen-failure paths preserve recovery media. A
reopen failure leaves the take paused with the error on the HUD (see the failure
and retry slice below).
Both 430×102 HUDs expose Mute/Unmute names, selected muted state, lifecycle busy
gating and an explicit mic-less explanation. Status: macOS AppKit and Windows are
implemented / unverified on physical hosts; Linux X11 is verified on the private
software-rendered Xvfb desktop with a disposable PulseAudio null-sink microphone;
Wayland remains gated with native live capture. A synthetic tone verifies decoded
audible/silent/audible intervals across mute/unmute, not physical microphone
fidelity or gapless device/encoder transitions.

The shared recording session also exposes a read-only live microphone peak through
`microphone_level` on the existing v1 recording request: `{microphone_peak}` is
finite in 0–1, and zero for countdown, pause, stopped/failed/discarded, muted or
mic-less sessions. The engine clears a disconnected microphone's meter while
retaining its warning and captured media. Both HUDs sample the existing serialized
session worker at up to 10 Hz while visible and unmuted, with one read in flight.
Their neutral fixed-glass meters clear during pause, mute and lifecycle work;
stale completions cannot revive a previous take. Hidden controls retain only the
existing warning polling cadence. Private X11 virtual-audio tests check changing
volume, silence, mute/unmute and pause alongside decoded media. Physical macOS and
Windows microphones, accessibility and Wayland acceptance remain open.

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
hosts; native countdown windows use the fixed media palette. Both hosts use the
shipping "Screenshot in" / "Recording starts in" headings and hold "Cancelling…"
for the shipping 180 ms exit window after Escape, fading in and out as shipped.
HUD, tray, guidance and Preferences copy follow the shipping strings, and recording
times use the shared `h:mm:ss` formatter. The HUD's Delete recording asks the
shipping "Delete recording?" question before discarding a take (AppKit alert,
wgpu confirmation window). Real mixed-DPI,
focus, compositor, accessibility, and animation acceptance remains open.
Cursor inclusion now shares sampling/compositing with the shipping host (macOS
system pixels, Windows/X11 synthetic arrow). Full region/window parity, recording, editor,
preview-stack interactions and full UI/UX parity are still open; this slice closes no complete
platform acceptance row. Hardware capture and clipboard tests remain required.

Both hosts now connect retained screenshot mini-preview stacks, backed by shared
Rust membership, layout and visibility policy. Copy uses full pixels; Save reads current output
preferences and becomes Reveal after export; Edit opens that screenshot directly;
Dismiss preserves history and exports. Edit leaves a hidden workspace hidden and
does not switch Preferences or change the selected History item. It reuses the
existing native editor/session rather than reimporting media or resetting drafts.
AppKit real-bridge tests cover a different selected History item and refocusing
pending crop edits; wgpu tests exercise the dispatcher, stale/busy guards, target
identity and root visibility commands. Private-X11 checks direct editor focus,
repeat activation, unchanged History and clean close. Physical-platform focus,
accessibility and Wayland live acceptance remain open.
The native chrome now follows the shipping Tauri card rather than a permanent
button footer: full-bleed cover images, idle dimensions, hover-revealed corner
icons and centered Copy/Save file/Show in Folder controls. Saved cards expose
Close plus Delete; unsaved Delete only dismisses. Right placements mirror the
corner controls. The stack toolbar appears only for two or more expanded
previews: an outer Clear all icon (tooltip "Clear all") and a Minimize icon that
swaps to a "Show less" label and widens inward on hover or focus (no morph
animation). Card icons (Close, Delete, Edit) and Clear all show the shipping
instant glass tip, on hover or keyboard focus with no delay: centered, above
the icon in bottom-anchored stacks and below it in top-anchored ones, fading
and nudging 2 pt (`.icon-button::after`). AppKit and wgpu share this chip with
the recording HUD's tooltip; the system/egui hover texts are gone, including
the native-only "Click to expand; drag to move the preview pile", "Drag the
original file to another app", overflow-cue, metadata and Copy/Save tips that
shipping lacks. `captures_app::preview_chrome` owns the tip geometry.
When an expanded stack overflows, centered chevron cues at the window edges
("Show older captures" / "Show newer captures", swapped for top placements)
scroll one card slot; `captures_app::preview` owns the edge tolerance, slot
target and copy for both hosts.
AppKit and wgpu use the same 12-point radius token and fixed-glass palette.
X11 checks exercise four corners, overflow, exact pixels, nonactivating actions,
idle/hover media contrast and repeat outbound drags that start right after a
control click (see [handoff](native-preview-handoff.md)). AppKit fixture coverage
checks mirrored geometry, hidden controls and in-place saved-state updates;
macOS CI must verify it. Windows physical presentation and input, screen-reader
and physical keyboard traversal on nonactivating panels (see the keyboard slice
below), and Wayland live-host rendering
remain unverified. Cards now show shared `W × H · size` metadata, the shipping
"Copied to clipboard" chip with Copy hidden while the clipboard still holds that
capture, and a one-second ✓ Saved confirmation. Ownership follows the shared
`captures_app::clipboard` model: the macOS pasteboard change count, the Windows
clipboard sequence number, or on Linux a host write counter plus a throttled
pixel comparison that notices other apps replacing the clipboard. New cards play
the shipping arrival (see the motion slice).

Editor presence, hover media and stale-pointer suppression now follow the
shipping `ThumbnailCard`, with rules in `captures_app::preview_chrome` (AppKit
through the `captures_preview_editor_*`, `_hover_lock_v1`,
`_icon_tooltip_frame_v1` and `_hover_media_v1` ABI):

- **Editor presence.** While a screenshot editor window shows a card's capture
  (visible or minimized), its Edit icon morphs into the "In editor" pill and
  the card gains the 2 pt accent ring with its glow (`.thumbnail-editor-active`).
  Hover or focus offers "Show in editor", which focuses the existing editor;
  the click that opened it keeps the passive label until the pointer leaves.
  Closing the editor plays the shipping 550 ms leave (the ring eases out, the
  pill shrinks and ignores clicks), then the plain Edit icon lingers for 3 s
  without hover. wgpu reads its open-editor map each frame; AppKit's screenshot
  editor reports presence changes to the preview controller. The optimistic
  morph while an editor opens is not separate: presence arrives in the same
  frame (wgpu) or after the settings read (AppKit). Shipping's viewer-active
  ring has no emitter in the shipping backend, so it has no native equivalent.
  Ring and pill appear on expanded cards only.
- **Hover media.** Hover or focus applies `blur(2px) brightness(.5)
  scale(1.015)` over the shipping 180/220 ms transitions. AppKit uses a Core
  Image Gaussian blur on the image layer, a 50% black layer and a clipped
  scale. wgpu fades in a card-sized copy blurred off the UI thread (a real
  2 pt Gaussian that fades past the card edge, composited over the card
  fill) over the darkened, scaled sharp image.
- **Stale-pointer suppression.** After an expand or a new card, hover chrome
  and the blur stay idle until the pointer moves 4 pt from its first sample or
  leaves the stack (`data-thumbnail-suppress-card-hover`); a pointer already
  outside releases it at once, and keyboard focus still reveals chrome.

The animated Show less morph, the dismiss/delete exits and other stack
transitions remain follow-up work; this does not close the visual parity gate.
Share/sign-in UI is deliberately outside this slice.
Show less/expand preserves capture order, overflow scrolls without a
count cap, and Clear all dismisses only snapshotted IDs, not later captures.
Reveal uses the current exported path, with file checks off the UI thread and
guarded async completion. A missing export reports an error without another save
or removal of the capture. Saves through History also update the preview action.
AppKit selects the export in Finder; Windows uses Explorer selection; Linux asks
the session's `org.freedesktop.FileManager1.ShowItems` implementer to select the
`file://` URI, as shipping `reveal_item_in_dir` does, and opens the parent
directory with `xdg-open` when no file manager answers. Private X11 checks both the
exact ShowItems URI and the fallback; file-manager behavior on physical desktops
remains unverified.
Trash uses the shared `trash_preview` operation: move the explicit saved export
to OS trash, then dismiss that card; unsaved cards only dismiss. Private History
bytes and metadata remain untouched, matching shipping screenshot Trash rather
than Delete from History. Saved-path snapshots and in-flight/presentation guards
reject stale actions and callbacks. Missing exports and OS errors keep the card
available for retry; directories, symlinks and private History paths are rejected.
The existing trash backend may request Finder automation permission on macOS;
Windows isolates its COM initialization on a fresh worker thread. Private-X11
smoke uses disposable XDG Trash and checks exact bytes and `.trashinfo` paths,
failure/retry, focus and History preservation. Actual Finder/Recycle Bin behavior
and Wayland desktop interaction remain unverified, not completed parity gates.
The four corner placements use actual monitor work areas. Private-X11
tests exercise placement, focus, minimized-root actions, exact capture inclusion/
exclusion and cancellation. AppKit tests cover panel/decode/action
lifecycles and fixed-glass rendering. Windows runtime, physical macOS, mixed-DPI,
screen-reader and compositor acceptance remain open; Wayland stays unsupported.
Collapsed front-card drag and hover fan are connected on AppKit and wgpu. The
shared Rust pose expands rear-card spacing from 13 to 16 points in the correct
direction for top and bottom anchors while leaving the front card and window
fixed. Rear cards remain noninteractive; press/drag holds the fan open. Both
hosts settle a 200 ms transition. AppKit follows the system Reduce Motion setting;
wgpu supports explicit `--reduced-motion` and reads Windows client-area animation
or the Linux Settings portal's standardized reduced-motion preference off the UI
thread on live startup and workspace foreground return. Reads coalesce, never
write settings, and retain the last known value if temporarily unavailable.
Live wgpu also subscribes to changes for the life of the process
(`captures_session::watch_reduced_motion`), so a change applies while the workspace
stays unfocused: on Windows a hidden top-level window on its own thread receives the
`WM_SETTINGCHANGE` broadcast for `SPI_SETCLIENTAREAANIMATION` (message-only windows
miss broadcasts) and re-reads it; on Linux a session-bus thread takes the Settings
portal's `SettingChanged` signal for `org.freedesktop.appearance` / `reduced-motion`
and applies its value without another read. Neither polls. Where no notification
source exists (no session bus or portal, or a portal that never emits the key), the
foreground re-read remains the only refresh; macOS AppKit reads
`accessibilityDisplayShouldReduceMotion` at each animation, so it is always current.
Fixtures stay independent of the host preference. Linux desktops without that key
use ordinary motion unless explicitly overridden. The private-X11 system-motion
smoke checks startup, foreground refresh and unfocused `SettingChanged` signals; the
Windows watcher is compiled only by Windows CI, and physical Windows/Linux
accessibility acceptance remains open.
Reduced motion switches immediately. AppKit
uses native frame animation; wgpu repaints only while egui's transition is active.
Both hosts also paint the shipping `glass-strong-solid` depth overlay on compact
rear cards: shared Rust calculates `min(.72, poseDepth * .14)`, with no shade on
the front or expanded images. AppKit uses a clipped native view overlay; wgpu
paints the same token over the retained image without altering source pixels.
This connects translation and depth shading; the preview effects slice below
adds the 3D tilt, depth blur, shadows and hover glow. The hover fan now eases over
`--stack-fan-dur`/`--stack-fan-ease` (`--dur-3`, `--ease-standard`) with shipping's
16 ms stagger per layer: the transform, glow and position wait `pose depth × 16 ms`
and the media blur `slot depth × 16 ms` (shared `preview_motion::StackFan`, which
retargets each card from where it is; AppKit delays each layer's Core Animation).
Expand clears the blur each card had on screen when it began, usually the fanned
pile's (`thumbnail-card-expand-blur` from `--thumbnail-stack-expand-blur-from`), over
the 0.52 s flight; wgpu also starts the flight from the fanned pose. Carrying the
pile now plays shipping's drag sway: once the fan has held open for its gather
(`--stack-fan-dur` plus the deepest layer's stagger), the rear cards lean with the
pointer's velocity through the shared under-damped spring
(`preview_motion::DragSway`, `captures_preview_drag_sway_tick_v1`) and the
`.thumbnail-stack-drag-sway` pose (`collapsed_card_sway_pose`,
`captures_preview_pile_sway_pose_v1`); dropping eases the lean back over the fan's
staggered transition. wgpu ticks the spring once per frame from desktop pointer
samples; AppKit on a display-rate timer that stops once the lean settles. Reduce
Motion never leans. Shipping keeps the pile on the primary
monitor; both hosts already open it on the capture display. Physical
AppKit, Windows and Wayland presentation/interaction are unverified; private X11
provides the Linux rendering/input evidence. The effects parity gate remains open.

### Preview effects fidelity — connected, acceptance open

The preview stack now draws its shipping CSS filters, box shadows and 3D pile
tilt from the shipping values instead of approximations:

- **3D pile tilt.** `captures-app::preview::collapsed_card_pose` also returns the
  whole `translate3d rotateZ rotateX scale` pose seen through `perspective: 900px`
  as a 2D projective map (`captures_preview_pile_projection_v1`), so the
  `rotateX` tilt keeps its keystone instead of the old vertical squash. AppKit sets
  it as the card layer's `CATransform3D` (Core Animation divides by w); wgpu
  tessellates each rear card flat and moves its vertices through the map. Unit tests
  compare the map with a direct CSS 3D evaluation.
- **Rear-card depth blur.** `filter: blur(pose × 1.15px)`, `× 0.75px` while the
  pile fans (`captures_preview_pile_media_blur_v1`). AppKit animates the media's
  Core Image Gaussian with the fan and the list ↔ pile flight. wgpu builds real
  Gaussians of the card media once per radius and cross-fades the two nearest
  prepared radii while a transition runs; settled frames show the exact radius.
- **Gaussian blur in wgpu.** `effects::gaussian_blur` is a separable Gaussian
  (σ = the CSS radius, transparent past the element like a CSS filter). It
  replaces `fast_blur` for the hover media and the averaged offset copies of the
  Close streak, which now steps through the shipping `feGaussianBlur
  stdDeviation="3.5/8/14 0"` filters at each keyframe midpoint (CSS cannot
  interpolate `url()` filters), dropping the hover brightness after the first step
  as shipping does. Blurs of a point or more run at one pixel per point. AppKit's
  streak still uses `CIMotionBlur`.
- **Box shadows.** `prepare.mjs` exports every `box-shadow` token plus the preview's
  `--thumbnail-card-shadow` (read from the shipping rule). Cards (expanded, pile and
  exiting), card icon buttons (`--shadow-sm`), main actions (`--shadow-md`), the
  editor control and pill, stack toolbar buttons, overflow cues (`--glass-shadow`)
  and the capture guidance chip draw them (AppKit's present editor pill keeps only
  its accent glow, without `--shadow-sm`). AppKit uses one masked shadow sublayer
  per layer (`shadowRadius` = blur / 2, clipped outside the element like CSS);
  wgpu draws cached Gaussian masks (σ = blur / 2) cut out under the element and
  mapped through the card's transform. The hovered or pressed pile adds shipping's
  `0 0 0 1px rgba(accent, .55), 0 0 22px rgba(accent, .28)` on every card.

Masks and blurs are built once per size or radius, so settled frames stay idle.
- **Arrival blur.** `thumbnail-arrive`'s `filter: blur(3px → 0)` plays on the
  media: AppKit animates a second Core Image Gaussian that is removed once the
  card lands; wgpu cross-fades a copy blurred at 3 pt (built with the hover blur
  off the UI thread) out as the radius falls. The card's border and chrome are not
  blurred.
- **History hover shadow.** `.history-card` draws `--shadow-sm` and eases to
  `--shadow-md` with the hover lift (`--dur-3`, `--ease-standard`). Both hosts
  cross-fade the two token shadows instead of interpolating each layer, so their
  cached masks serve every frame.

Still open: `backdrop-filter` glass (wgpu cannot read the desktop behind its
window; AppKit's `NSVisualEffectView` materials add their own tint over the 82–93%
opaque token fills, so neither host blurs the backdrop yet). Verified with Rust unit
tests, `node --test scripts/native-tokens.test.mjs` and the private-X11 preview
smoke; AppKit XCTests run only in macOS CI, and physical macOS/Windows/Wayland
visual acceptance remains open.

### Outbound preview file dragging — connected, acceptance open

Expanded screenshot media starts a COPY-only native file drag; collapsed piles
still move. Shared preparation chooses the saved artifact, otherwise copies the
full PNG or recording media (never the poster) to a unique retained export.
Temporary exports survive completion and History deletion and are cleaned at
the next startup. This is not a new recording-preview presentation slice.

AppKit uses `NSDraggingSource`; wgpu uses Windows `drag` 2.1.1/OLE and a private
winit 0.30.13 patch for XDND and Wayland, on the window's existing connection.
wgpu preparation runs off the UI thread and checks the originating press before
starting; both hosts guard completion by artifact generation. Only an accepted
external COPY dismisses. Own-app drops retain, and self-drops shake for 420 ms
unless reduced motion is enabled. Native completion resets consumed pointer
state; X11 cancellation, disappearing targets and missing Finished have bounded
cleanup. See `apps/native/wgpu/vendor/README.md` for provenance/update obligations.

| Host | Implementation | Verification / remaining acceptance |
| --- | --- | --- |
| macOS | AppKit source, async identity guards, COPY/landing policy | XCTest lifecycle cases added; physical Finder transfer still unverified |
| Windows | GUI-thread OLE adapter, retained window, preloaded icon | Isolated adapter cross-compiled; real Explorer transfer and mixed-DPI own-window classification unverified |
| X11 | wgpu bridge, same-connection XDND, Escape/timeout recovery | Real private-X11 original-byte and Unicode saved-path transfers, self-drop, reject/cancel/target loss/repeat; physical desktop acceptance open |
| Wayland | Same-connection source and serial, own-offer completion, five-second post-drop deadline | Headless Sway → independent GTK receiver verifies exact URI/bytes, reject/cancel/repeat/self-drop, target loss and missing-Finished recovery; full capture host and physical compositor acceptance open |

Windows classifies the final cursor against viewport rectangles conservatively;
mixed-DPI/overlapping windows need physical verification. Wayland tests exercise
protocol recovery on one disposable compositor, not a physical desktop or the
full capture host. No shipping Tauri behavior or parity gate changes.

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
and accessibility acceptance remain open. Login items, physical OS shortcut
takeover acceptance and the other lifecycle checklist requirements remain open.

The live single-instance slice elects one native process per canonical History
root before starting UI, capture workers or global keys. Shared Rust uses an OS
file lock plus private Unix sockets / current-user Windows named pipes; no TCP
listener, fixture singleton or installed Tauri identity is added. Sender-relative
paths become absolute without requiring files to exist; the existing host queues
retain per-file errors, source deduplication and editor/draft safety. Empty
requests restore native workspace/Preferences or hidden recording controls.
Framing, queue length and whole-exchange deadlines are bounded; only a lock holder
reclaims a stale Unix socket. Acknowledgement means queued, not successfully
opened or durable across quit/crash. A lost acknowledgement is never retried
automatically. Event-loop wakes replace idle polling. Accepted quit stops delivery,
drains host workers, then releases the lock; cancelled quit retains the owner.
Rust tests exercise concurrent election, cross-process sender CWD, killed-owner
recovery, late wake registration, queue overflow, malformed/stalled peers and
ordered delivery. AppKit and Windows include executable-level CI smoke coverage;
until those jobs pass their runtime status is implemented / unverified. Linux X11
has private software-rendered editor/forwarding tests, not physical acceptance.
Wayland uses the same transport and host implementation but remains runtime/
presentation-unverified. Installed associations, physical input/accessibility
and full lifecycle/OS-integration acceptance remain open on all platforms.

The shortcut-editor slice adds all seven Preferences recorder rows to both hosts.
Rust owns modifier/key policy, cancellation, display tokens and persisted-field
validation, checked against 585 shipping TypeScript recording/display vectors.
AppKit intercepts focused recorder events before menu equivalents; wgpu observes
root winit physical keys before egui loses PrintScreen, keypad or Super identity.
Focused Preferences temporarily releases screenshot OS grabs, retaining desired
bindings and restoring the latest saved mapping on blur. Registration failures
leave capture routing suspended and report an error. The recording bindings, then
storage-only, now open the capture menu in Record mode (see the recording shortcut
notes). The private-X11 `--lifecycle --shortcut-editing`
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
Both hosts lay it out as the shipping single row (positive check tile, copy, Save
file / Show in Folder with its icon, and a quiet × dismiss). The "Recording
controls hidden" notice shares its copy and card size through `captures-app`
(`hidden_notice` on the recording HUD ABI for AppKit) and draws the shipping
accent tile, left-aligned copy and New Capture key chips.
Both hosts now honour `open_editor_after_recording` like the shipping app: a
finished take opens the recording editor, and the notice appears when a recording
editor closes (including editors opened from History). With the preference off,
no editor or notice appears. Private-X11 input tests exercise export byte equality,
failure/retry, missing exports, intercepted OS-reveal arguments, hidden-root expiry,
dismissal and capture cleanup; AppKit provides state and render fixtures. Both
hosts play the shipping lifecycle animation (see the motion slice). Physical
macOS/Windows, Wayland and accessibility acceptance remain open.

The launch notice slice adds the shipping "Captures is ready to use" pill to both
native hosts: fixed dark glass, a painted triangle caret pointing at the tray or
menu bar item, the saved New Capture shortcut as key chips, and Close. It appears
for 15 seconds after first-run setup completes and 5 seconds on a hidden,
tray-resident live launch without media. It never appears on a visible relaunch.
The Tauri placement policy and its tests moved into `captures_app::tray_notice`,
and AppKit reaches it through `captures_startup_notice_placement_v1`. AppKit uses a
nonactivating, floating, all-Spaces `NSPanel` anchored to the status item. It
flips coordinates at the ABI boundary and retries briefly while the item is
unplaced. wgpu uses a transparent, undecorated, always-on-top, nonactivating
viewport. On Windows it anchors to `TrayIcon::rect()` with the same retry. X11 has
no StatusNotifier rect, so it uses the panel-edge fallback. Private-X11 onboarding
smoke checks the setup-completion notice title, size, focus retention and Close
dismissal. AppKit has XCTest layout/copy/dismiss coverage. The macOS, Windows,
Wayland and quiet-login paths have not been verified on physical hosts.

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
notice replaces no controls. The menu bar/tray New Capture item, app reactivation and a
restore-only New Capture shortcut bring the HUD back; screenshot shortcuts and tray items
screenshot beside the take instead (see below), and no other busy shortcut is enabled.
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
recording because X11 cannot exclude overlay windows. The wgpu child capture waits
for a completed root pass to retire its selector/countdown viewport, then settles
for 150 ms before reading pixels, matching the ordinary capture path's compositor
allowance. Escape still cancels the child during this wait without ending the take.
This applies to the Windows/X11/Wayland host; Wayland capture remains gated, and
AppKit keeps its separate native-window removal path. Private-X11 acceptance covers
running publication, paused countdown cancellation, selection Escape, asymmetric
saved pixels, same-session continuity, final decode and recovery cleanup. AppKit CI
renders/tests the enabled HUD; real macOS/Windows capture and Wayland remain open,
so this does not close the Recording HUD parity gate.

The screenshots during recording slices match shipping's screenshots beside a take.
Shipping's region, window and display shortcuts and `start_capture_from_tray` all call
`start_capture_inner(mode)` while `recording_session_is_active`, exactly as the recording
controls' Screenshot button does for region (`lib.rs` capture-shortcut handlers and
`start_capture_inner`). `captures_app::capture_error::screenshot_route` holds the rule,
with `display_route` for display's idle capture menu, and AppKit reads it through the
settings ABI: no session opens the region or window selector, or the capture menu on
Full screen for display; a running or paused take takes the screenshot beside it; a
take that is selecting, counting down, finalizing or in its editor refuses silently,
like `screenshot_capture_is_blocked` (`recording.rs`). The shared shortcut routes pass
every chord to the host while a take owns the capture flow, including while its
controls are hidden (see the busy capture slice below). The recording shortcuts and tray
items do nothing there (`prepare_capture_selector_inner` returns `CaptureInProgress`,
which they ignore), so a second recording cannot start. New Capture
restores hidden controls; with the controls showing it reports "capture already in
progress" in the "Captures" error dialog, as shipping's `open_capture_controls` does
(`capture_error::new_capture_route`). Only New Capture restores hidden controls; the
other tray items leave them hidden.

The busy capture slice matches what shipping does with a capture shortcut or tray item
while a capture is already open or in flight. `captures_app::capture_error::busy_route`
holds the rule (AppKit reads it through the settings ABI's `busy_capture_route`), and
the shared shortcut routes now deliver every chord to the host while a capture or
recording owns the flow (`CaptureShortcuts::set_capture_busy`), so shortcuts and tray
items take the same path:

- **Recapture.** Over an open region or window selector (`overlay_visible` /
  `should_recapture_visible_capture_ui`), Screenshot Region and Window open that
  selector again, and New Capture, Screenshot Display and the Record items open the
  capture menu (Record mode for Record), each on a frozen snapshot of the display under
  the pointer taken with the old selector still on screen. With the menu open, the
  shortcut of its own screenshot target (New Capture on Region) recaptures the menu the
  same way (`should_recapture_open_capture_menu`); other targets and modes switch it in
  place. That target is the one the menu opened on or its shortcuts and tray items last
  set, like shipping's selection summary (`open_menu_screenshot_target`); a target picked
  inside the menu does not change it. The old UI stays up until the new snapshot is ready, and a recaptured
  selection never counts down (`screenshot_countdown_seconds_for_capture_ui`). wgpu
  prepares the new session on the same capture generation and swaps the viewport when
  it arrives; AppKit keeps the old panel on screen with `sharingType = .readOnly` so it
  is in the snapshot, then closes it.
- **Beside a recording.** Screenshot Region and Window over the take's screenshot
  selector recapture it on its child generation; Screenshot Display saves the display
  under the pointer at once with the selector in it, then closes the selector.
- **Busy.** New Capture reports "capture already in progress" in the "Captures" dialog
  during a screenshot countdown, preparation or capture outside a recording, and
  during any recording state (`prepare_capture_selector_inner`), including a recording
  countdown or finalize. Screenshot Display reports it too outside a recording, since it
  opens the menu through the same path. Region, window and Record are refused silently
  (`screenshot_capture_is_blocked`, `CaptureInProgress`).
- **Concealed controls.** Shipping's `restore_hidden_recording_controls` shows the
  recording controls whenever their window is off screen, so New Capture during a
  screenshot beside a running or paused take brings back controls that the screenshot
  concealed (the default, without "include recording controls"); only with the
  controls showing does it report the busy take.

Shipping's brief preparing and capturing gaps race its prefetch and session maps (a
region shortcut there drops the stale session and starts again); native hosts treat
them like a countdown. Private-X11 acceptance (`x11_capture_smoke.py --recapture`) presses
the region shortcut over a live region selector and checks the new selector stays on the
frozen desktop after it changes, and the saved pixels are that desktop under the old
selector's veil, saved without a countdown; replaces
the selector with the window selector, New Capture and Screenshot Display; and during a
countdown checks the busy dialog and that other shortcuts start nothing. The
`--target-shortcuts` smoke checks the busy Screenshot Display dialog during a menu
countdown. The shared `busy_route` tests cover every action against every activity and
recording state; wgpu unit tests cover how its capture states map to those activities
and what each route requests, and AppKit XCTest covers the route through the settings
ABI and the recapture preferences. `x11_recording_smoke.py --display-screenshot-only`
checks, over the take's region selector, that New Capture brings the concealed controls
back without closing the selector and that Screenshot Display saves the display at once
without a countdown. The region and window recapture beside a recording and the menu
recapture's snapshot pixels have no end-to-end smoke. wgpu reuses the selector's
viewport for a recapture and repaints it when the new snapshot arrives. The AppKit side has not yet been built or run on macOS, and real macOS
and Windows capture, Wayland and multi-monitor acceptance remain open.

Each screenshot reuses the child generation above. Region and window open their normal
selector over the take on the display under the pointer with the current screenshot
preferences (the controls' Screenshot button keeps the recording's display); display captures the pointer's display after the screenshot countdown.
Both hosts hide the recording controls for the selector, countdown and capture unless
they are opted into captures (shipping `conceal_capture_chrome_for_snapshot`, restored
in `finish_capture`), keep the region guide and, unless opted in, mini previews out of
the image, and publish to History, the clipboard (when auto-copy is on) and the mini
previews. Hidden controls stay hidden, as in shipping. The take keeps its segments,
display, guide and output, and Escape during selection or countdown cancels only the
screenshot. On X11 the selector and countdown windows still appear in the ongoing
recording, as they do in shipping; AppKit selection panels are excluded from capture.
Private-X11 acceptance (`x11_recording_smoke.py --display-screenshot-only`) takes the
display shortcut screenshot while running and the real tray Screenshot Display while
paused, the region shortcut screenshot while running and the real tray Screenshot Window
while paused, cancels a countdown, dismisses the busy New Capture dialog, and checks the
saved pixels, the clipboard, the mini preview, same-session continuity and the decoded
recording. AppKit is covered by XCTest only; real macOS/Windows capture, Wayland and
multi-monitor acceptance remain open.

The recording HUD failure and retry slice ports the shipping `RecordingHud` failure
states to both hosts through `captures_app::recording_hud` (AppKit calls it through
`captures_recording_hud_request_v1`). That policy owns each state's enabled
controls, accessible names, tooltip copy, status label/dot, `recordingErrorMessage`
cleanup, the inline error line and tooltip placement. Behavior now matches shipping:

- **Start failure.** When the engine cannot start a take (for example a missing
  selected microphone or denied screen access), the HUD stays up in its Failed
  state: a static subtle dot, "FAILED", the error on one signal-text line below
  the controls, and only **Retry recording** and Delete enabled. The region guide
  is removed and Escape is released. Retry restarts the countdown at once, with no
  "Restart recording?" question, and a repeat failure returns to the same state.
  Delete still asks "Delete recording?". Shipping leaves Hide enabled here, but its
  command refuses a failed take, so native hosts disable it instead.
- **Resume or microphone change failure.** `RecordingSession` now leaves the take
  paused with its completed segments when the next segment cannot open, instead of
  failing it. The HUD shows the error inline so the take can be resumed again or
  saved. Cancellation (a stale generation or a locked session) still saves the take.
- **Saving.** Stop keeps the HUD up as "SAVING…" with the info dot, a frozen timer
  and every control disabled until publication. Finalize or encode failures still
  close the HUD and keep the recovery bundle, as shipping does, so the retry for a
  failed save is History's recording recovery, not the HUD.
- **Inline error line.** Engine warnings (a disconnected microphone, unwritable
  desktop audio) and HUD action failures (screenshot start, discard) move from the
  privacy line to shipping's `.recording-hud-error` line, and the privacy notice stays.
  The line clears when the next HUD action starts. A warning shows once per change,
  and a new take starts clear.
- **Styled tooltips.** A fixed-glass tooltip replaces system/egui hover text. It uses
  shipping copy ("Stop and save", "Retry recording", "Hide controls"…) and appears
  under the hovered or focused button, disabled buttons included, with no delay. It
  fades and slides 3 pt over `--dur-1`, and the last three right-align.
- wgpu also keeps the HUD window alive, with controls disabled, while pausing,
  resuming or changing the microphone. Previously the window closed and reopened.

Status: Linux X11 is verified on the private Xvfb/PulseAudio desktop.
`x11_recording_smoke.py --start-failure` covers the failed HUD, the inline error,
disabled controls, the tooltip, a failing and a succeeding Retry and Delete.
`--device-change explicit` now covers a paused take with the inline error that is
then saved. macOS AppKit has XCTest coverage but no physical-host run, and Windows
is implemented but unverified. Wayland stays gated. Onboarding/selector permission
flows are separate slices, so this does not close the Recording HUD gate.

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

The session also accepts one host-decoded in-memory RGBA image at a time without
putting pixels or asset URLs in JSON. It matches shipping visible-layer target
resolution, natural edge placement, capped stack sizing and fully-outside canvas
expansion, then validates retained asset limits, renders and commits one undo step
atomically. Imported assets survive undo/redo and draft save/reopen. Hosts still own
file decoding, pickers and batch/drag presentation; none is connected by this shared
prerequisite.

`captures_editor_import_image_v1` exposes that single-image operation on the
serialized C session boundary. Hosts pass borrowed top-down straight-alpha sRGB
RGBA8 rows plus JSON name/selection/point metadata; the adapter validates shared
render limits and all length/stride/pointer arithmetic before reading, copies into
session-owned storage, and returns the stable layer ID with the current snapshot.
Failures preserve document, frame, history, assets and files. This is an import
transport prerequisite only: decoding, file pickers, clipboard, batch import and
macOS, Windows, X11 or Wayland host acceptance remain open.

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
Hosts still own save dialogs, overwrite-original confirmation (now connected above)
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

Shared editor sessions can also create completed rectangle, ellipse, triangle, diamond and star layers from
typed start/end geometry, existing element styles and opacity. The command assigns
the stable layer ID and shipping unlocked/visible/source-over defaults, preserves
partial clipping, and expands/translates the document only when the annotation is
fully outside, including painted bounds from enabled default or custom shadows.
Degenerate closed-shape geometry is rejected transactionally instead of becoming a
synthetic filled pixel. TypeScript-derived reverse/fractional vectors and rendered
session tests cover rollback, undo/redo and draft reopen. This prerequisite is
shared by macOS, Windows, X11 and Wayland; host status is tracked separately below.

Shared sessions can likewise create completed straight lines and tapered arrows
from signed endpoints, existing element styles and opacity. Open shapes force the
shipping null fill and unlocked/visible/source-over defaults; click-only,
horizontal and vertical lines remain valid. Arrows below the renderer's 1.5
document-pixel cutoff are rejected transactionally, while hosts retain the
screen-scale `max(1.5, 3 / displayScale)` gesture cancellation policy. Creation
and transient host previews share one public tapered-arrow polygon helper with
the renderer. TypeScript-derived bounds cover reverse/fractional geometry,
partial clipping, shadow-only overlap and fully-outside sibling translation;
session tests cover pixels, rollback, undo/redo and draft reopen. This is shared
preparation for AppKit, Windows, X11 and Wayland. Connected host controls are tracked
below; physical, input and accessibility acceptance remain open on every platform.

Shared sessions can create one completed freehand path from ordered document-space
samples, existing element styles and opacity. Creation assigns the stable layer ID
and shipping null-fill, unlocked, visible and source-over defaults; one-point and
repeated-point paths remain valid. Authored sample bounds plus stroke and resolved
shadow padding preserve partial clipping and drive fully-outside canvas expansion,
including translation of every existing sibling and every path sample. Hosts retain
the shipping `1.5 / displayScale` pointer-sampling threshold, transient gesture state
and cancellation. A public centerline helper uses the compositor's midpoint-quadratic
sampling so native previews do not duplicate smoothing math. TypeScript-derived
vectors cover fractional/negative geometry, sample hulls that differ from the smooth
centerline, shadow-only overlap and outside translation; session tests cover pixels,
rollback, undo/redo and v1 draft reopen. This is shared preparation for AppKit,
Windows, X11 and Wayland. Connected freehand host controls are tracked below;
physical/input/accessibility acceptance remains open on all four platforms.

The shared layer command also accepts typed partial style patches for existing
shape and freehand-path annotations. Locked and hidden annotations remain editable;
closed-shape-only fill/stroke toggles do not mutate open shapes or paths, and shadow
customization uses the renderer's bounded defaults while preserving stored custom
and unknown fields when toggled off. Unsupported image/text targets and exact
no-ops retain history, redo and frame identity; failed rendering rolls back the
whole patch. This is a common prerequisite for AppKit, Windows, X11 and Wayland.
Host property controls are tracked below; physical-platform acceptance remains open.

The first wgpu editor window now opens isolated History screenshots on its own
serialized worker, with fit preview, numeric crop/canvas fields, undo/redo,
and a draft that autosaves after each change. Closing flushes the draft without a
prompt, as in shipping. Normal quit drains queued
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
are described below. Image layers expose a **Transform image** menu for lossless
left/right rotation and horizontal/vertical flips through the shared worker commands.
Hidden and locked images can transform, matching shipping policy; full-canvas
photos rotate their canvas, and undo/draft restore retain the orientation.
Both native hosts expose Merge down, Merge visible and Flatten image through the
Layers heading menu and clicked-row context menus. The shared session publishes
capabilities and commits each combination as one undo step only after rendering
succeeds. Merge down paints the adjacent unlocked pair even when hidden; Merge
visible keeps hidden slots and ignores locks; Flatten discards hidden layers and
bakes the canvas background into a locked image. New owned assets retain draft
and undo pixels without changing History originals. Aggregate image limits are
checked before allocating the combined raster. Rust tests cover asymmetric alpha,
ordering, atomic failure, undo/redo and reopen; X11 real-input coverage exercises
both appearances, menus, hidden layers, draft reopen and clipboard pixels.
AppKit has action and rendered-fixture coverage; macOS and Windows physical input,
accessibility and Wayland presentation acceptance remain open.

The wgpu Draw panel connects rectangle, ellipse, triangle, diamond, star, straight line, tapered arrow and freehand Pen
gestures. Preview points remain host-local until release sends one shared creation
command to the worker.
The new stable layer ID is selected and stale encoded output is cleared. Reverse
and off-canvas drags, zero-area closed-shape no-ops, cancellation, undo/redo,
persisted pixels and draft reopening have automated coverage. Shipping default fill and rounded
rectangle geometry are used. Open shapes keep signed endpoints and no fill;
horizontal, vertical and zero-length lines are retained. Arrow release requires
max(1.5, 3/displayScale) document pixels. The transient preview triangulates the
same concave tapered polygon used for shared rendering and painted bounds.
Pen keeps authored samples at least 1.5/displayScale document pixels apart,
including every accepted movement in a frame, and previews the shared smoothed
centerline. Input events are consumed once even during extra layout passes.
Click-only dots, cancellation preserving redo, exact quadratic versus polyline
pixels, off-canvas expansion and draft reopening have automated coverage.
Resize, rotation, endpoint and curve grips and the other tools arrived in later slices.
Windows/X11/Wayland share this host code; private X11 is the exercised UI, not
physical input/accessibility acceptance.
AppKit connects the same five drawing tools below.

The wgpu Layers panel connects annotation-style fields with one explicit Apply
style transaction. Local fields and color pickers emit only changed patch values;
displaying resolved defaults does not materialize legacy fields or overwrite unknown
data. A disabled shadow does not submit hidden custom controls. Reset and selection
changes discard unapplied fields; worker errors restore published values. Styled
pixels, undo/redo, draft restore and light/dark/minimum layouts are exercised on
private X11. Windows/Wayland presentation remains unverified; AppKit style controls
are described below. Physical input/accessibility acceptance stays open.

The wgpu Import image action picks PNG/JPEG/WebP/TIFF files independently
of the session worker. The worker bounds encoded input and decoded dimensions,
normalizes EXIF orientation and supplies owned RGBA to the shared import command.
Shipping decodes Add images and dropped layers in the webview, which color-manages
them into its 8-bit sRGB canvas; native import follows the same rules. RGB/grayscale
ICC profiles convert to sRGB before publication, preserving straight alpha;
untagged files assume sRGB. CMYK JPEGs (Adobe CMYK or YCCK) and CMYK TIFFs with a
CMYK profile convert their original ink samples through that profile; untagged
CMYK keeps the naive conversion shipping's decoders use. PNGs follow PNG 3
precedence: a supported cICP chunk (any H.273 primaries with an SDR transfer)
outranks iCCP, then sRGB, then gAMA/cHRM, which build a power-law source profile.
HDR PQ and HLG cICP PNGs map BT.2408 reference white (203 nits; HLG on a 1000-nit
display) to SDR white and clip brighter highlights, as an 8-bit sRGB canvas
receives them. Narrow-range or unspecified cICP falls back to the other chunks,
as in browsers; non-RGB cICP is rejected by the png decoder shipping's Open path
also uses. HDR/wide-gamut editing is not a shipping feature either: both
normalize to 8-bit sRGB. Malformed ICC profiles, or profiles whose color space
does not match the samples, still fail recoverably rather than being ignored as
browsers do. Undecodable imports report shipping's "<name> could not be loaded."
TIFF import matches the macOS webview (WebView2 and WebKitGTK cannot decode TIFF,
so native accepts more there); GIF, BMP, AVIF, SVG and HEIC layers, which shipping
accepts through the webview, remain open. Analytic fixtures in
`captures-app` cover ICC transport through PNG, JPEG, WebP and TIFF plus
grayscale alpha, a generated CMYK lut16 profile through Adobe CMYK and YCCK JPEGs
and a CMYK TIFF, cICP precedence/primaries/fallback, PQ/HLG reference white, and
gamma/chromaticity-only PNGs.
The returned stable ID selects the new layer. Cancellation, decode failures and
late results after close preserve the editor; a completed selection waits for
already accepted edits before importing. Imports do not write a draft or History
until explicitly saved, and saved assets survive deleting the external source.
Private-X11 checks exercise the actual rfd D-Bus transport with a disposable file
chooser fixture, asymmetric rendered pixels, cancellation/retry, undo/redo,
reopen and stale-close handling in both appearances. That fixture does not verify
physical file dialogs, input, accessibility or IME. Windows and Wayland share the
implementation but remain presentation-unverified; AppKit import is described below.
Shipping Tauri import is unchanged.

Both hosts now port the shipping canvas interactions from shared
`captures_app::editor_canvas` geometry and copy. Image files dropped on the
canvas (egui hovered/dropped files, AppKit `NSDraggingDestination`) show the
shared `image_drop_guide` target, edge glow or stack light and toast, then
import one file at a time through the existing external-image layer path at the
drop sample; later files stack below the previous import and unsupported drops
report an error. A selected layer past the canvas edge shows the overflow tint and
an Expand canvas action whose hover shows the dashed ghost; `expand_canvas` is one
undo step. Lines/arrows show endpoints, curve dots and starter dots with shipping
hover hints; drags preview live and commit one `curve` edit on release, double-clicks
add/remove points, and the Layers Curve slider (#841 `RangeSlider`/`TokenSlider`)
or Straighten action commit once. Curves persist in saves and drafts. Private X11
exercises a real XDND drop, Expand canvas, curve dots, undo/redo and draft reopen
in both appearances; AppKit is covered by XCTest only. Windows/Wayland share the
wgpu code but are presentation-unverified.

The final editor-parity slice adds the shipping Trim edges rule and hover
preview, the Wand colour loupe, `DrawToolPreview` and the Apply crop pulse to both
hosts. `captures_app::editor_canvas` owns `can_trim_to_content`/`trim_preview`
(snapshots carry `can_trim` and `trim_preview`); Trim edges is disabled when the
trim is a no-op, and hover or keyboard focus tints the discarded margins, dashes the
kept area and pulses the cut edges with blooms and `SNAP_PARTICLES`. The loupe
samples the same image and natural pixel as a Wand click
(`editor_image_background::wand_loupe`; AppKit through
`captures_editor_wand_loupe_v1`) and places itself with `wand_loupe_position`.
`editor_chrome::draw_preview` supplies the stroke/brush sample geometry, and
`motion` adds `editor_cta_pulse`, the trim breathing loops and the particle spec;
all rest under reduced motion (no particles, no halo). Private X11 smokes check the
disabled state, the hover tint, the loupe's sampled colour, the brush preview and
the Apply crop halo in both appearances; AppKit is covered by XCTest only (macOS
CI is its first compile). Windows/Wayland share the wgpu code but are
presentation-unverified.

Add images now matches shipping's multi-select file input on both hosts (rfd
`pick_files`, `NSOpenPanel.allowsMultipleSelection`). The chosen files feed the
canvas-drop import queue with no drop point: unsupported files are skipped (all
unsupported reports the drop error), the first image takes the default placement and
each later one stacks below the previous import, one undo step per file. The drop
guide's snapped edge and the armed Expand canvas ghost now animate like shipping:
`motion` adds `snap_bloom_breathe` (opacity and scale about the bloom's center),
`snap_edge_pulse` and `expand_ghost_breathe`, and `editor_canvas` the accent bloom
geometry (`min(96px, 42%)` with an 8 % overhang for the drop guide, 96 px for
Expand canvas). Both hosts share one bloom/bar/particle painter with the Trim edges
preview, whose bloom now breathes too. Under reduced motion every loop rests on the
element's own style (bloom opacity 0.95, bar and ghost 1) with no particles and no
redraw timer. The bar pulses' `brightness(1.15)` (drop guide, Expand canvas) and
`brightness(1.12)` (Trim edges) now cross the ABI as keyframe data and scale the
bar's and its glows' colour channels on both hosts. Private X11 checks
the multi-select portal request and import in both appearances; AppKit is covered by
XCTest only (macOS CI is its first compile). Windows/Wayland share the wgpu code but
are presentation-unverified.

Separately, both live development hosts open external PNG/JPEG/WebP/GIF/MP4/WebM
paths through the shared History-backed `open_media` request using repeatable
`--open-media` arguments (`--open-image` remains an ordered alias). The strict
`open_image` API remains available. AppKit also handles a running app's file-open
callback. Both queue startup inputs and serialize opens against editor focus and
History refresh; unsupported
paths do not block later ones. Still images reuse the bounded, color-managed decoder
above. As in shipping's `open_media`, TIFF and other stills are rejected with
"Captures can open PNG, JPEG, WebP, GIF, MP4, and WebM files." Shipping's Open
relabels decoded samples as sRGB; native converts ICC, CMYK, cICP/HDR and
gamma-only sources as described above. Already-open canonical sources preserve active edits; a
closed source reloads under the same History ID and, as in shipping, drops its
autosaved draft, but only after the new pixels decode (a bad source keeps the draft).
AppKit waits for its current editor open to settle before advancing the batch;
pending text blocks switching without losing the new History item, and edits in the
replaced capture autosave first. Screenshot source bytes stay untouched and the
source path remains the export bar's default overwrite target. Private X11 exercises bad-file
continuation, three editors, canonical aliases, decoded pixels, untouched sources,
History draft restoration and same-ID source reload that drops the draft in both
appearances. Windows and Wayland use the same host code but this
entry point remains presentation-unverified there; AppKit uses macOS CI bridge/window
tests. Neither host registers file associations or claims physical file-open acceptance.
Broader platform image formats remain separate work; live single-instance
forwarding is described above. AppKit waits for recording frame and thumbnail settlement before
advancing, focuses canonical active recordings without losing staged work, and
refuses unsafe editor switches. wgpu keeps one editor per active artifact and
includes recording IDs when requesting canonical-source focus.
GIF/MP4/WebM enter History as external recording references without copying the
source. The request matches canonical active sources before requiring FFmpeg,
validates a decoded editor frame and poster before History publication, and keeps
the same ID on closed reopen. FFprobe's combined MOV/MP4 and Matroska/WebM
demuxers are disambiguated with bounded container headers; MOV and MKV are not
silently labeled as supported formats. WebM Preserve-to-MP4 transcodes instead of
copying source bytes. Reference-backed recordings keep **Save as new file** locked
on; the existing private-recovery and permanent-save identity checks still guard
the backend operation. The private-X11 external-media fixture exercises a mixed
batch, staged GIF trim through alias focus, real container metadata despite a
misleading suffix, same-ID closed WebM reopen, H.264 MP4 export with decoded output
pixels, source-byte identity and normal/minimum recording and error states.
These checks do not close Windows, Wayland or physical-host acceptance gates.

The wgpu export settings encode shared PNG/JPEG/WebP output with the shipping
quality modes, Compress presets and maximum file size. Encoding and decoding run
off the editor worker; the canvas shows decoded output in the automatic comparison
split. Edits and option changes refresh the comparison; encoding failures appear in
the comparison frame and retain recoverable edits.
Preview never writes files or saves a draft. The same Windows/X11/Wayland host
code is implemented; private-X11 and unit checks do not establish physical-host
acceptance. Save runs shared publication on the same worker; new files never
replace existing files and add a distinct History entry without modifying the
original or draft. A post-publication History failure shows the saved path and
warning. Accepted writes drain before application quit. The wgpu host also connects
an output-folder picker and edited-image clipboard output. AppKit export and
clipboard controls are described below; physical-platform acceptance remains open.

The AppKit editor host now enables **Edit screenshot** only for screenshot History
entries. Its dedicated serialized worker owns the shared Rust session and publishes
independently retained RGBA frames to a fit preview. The window exposes crop geometry,
canvas sizing, Undo/Redo and an autosaved draft; shared Rust
remains the only geometry/render authority. Geometry and Layers views retain the fit
preview; the front-to-back layer panel exposes visibility, lock, opacity, absolute
X/Y movement through shared deltas, image rename, duplicate, delete and adjacent
ordering. Stable IDs preserve selection across replies, and shared Rust remains the
authority for locked barriers and duplicate behavior. The export settings run shared
PNG/JPEG/WebP encoding on that worker, report exact bytes and switch the fit preview
between the edited canvas and decoded output. Option or document changes invalidate
stale output; previewing has no draft, undo, clipboard or file side effects. **Change…**
chooses a directory independently of the worker; Save then serializes publication on
that worker without mutating the draft. New files never replace an existing file and
add a distinct History entry, and partial History failure preserves the saved path.
**Copy image** encodes the full-resolution edited frame as lossless PNG on that same
worker, then publishes retained bytes to the AppKit pasteboard only if the session's
generation and artifact still match. Export options (including invalid byte budgets)
do not affect copy. Copy preserves encoded-preview selection, document, undo and draft
state without writing files or History. Encoding/clipboard failures leave a retryable
editor; stale completions after termination cannot write to the clipboard. Automated
tests cover cropped PNG pixels on a named pasteboard and byte ownership after worker
close; physical cross-application paste and accessibility acceptance remain open.
Windows/X11/Wayland retain the existing wgpu clipboard path unchanged.
Drafts use
the same isolated sibling root and reopen with the screenshot. The Layers view also
imports one still image at a time through AppKit's color-managed ImageIO decoder,
normalizing EXIF orientation and straight-alpha sRGB RGBA8 pixels before the worker
copies them into the shared session. Imported layers reopen without their source file;
ImageIO-supported sources use their first image, and files without a usable color
description are rejected instead of silently relabeled.
Image layers expose shared rotate-left, rotate-right, flip-horizontal and flip-vertical
commands, including hidden or locked layers; shared Rust owns orientation, canvas fit,
clipping and expansion policy while AppKit retains the stable selected layer.
The AppKit **Draw** view connects Rectangle, Ellipse, Line, Arrow and Pen gestures to the fitted
edited preview. Pointer state stays host-local; release submits one shared
creation command, selects the returned fresh layer ID and invalidates stale encoded
output. Preview mapping preserves reverse and off-canvas coordinates without reading
unapplied numeric fields. Escape, focus loss, close, or changing sections cancels a
drag without editing the document. Shared Rust remains the authority for default
style, clipping, fully-outside expansion, rendering and undo/draft transactionality.
The C ABI supplies the shared arrow polygon and smoothed Pen centerline without
per-event JSON or session-worker access. AppKit paints round caps/joins and click
dots. Axis-aligned/zero-length lines remain valid; arrows enforce both the three-view-
point threshold and the shared minimum document length. Pen accepts each delivered
movement at least 1.5/displayScale document pixels from its last accepted sample,
including off-canvas samples, without appending the release location. Mouse event
coalescing is disabled only during a Pen stroke; every completion/cancellation restores
the previous setting. Physical mouse/tablet sample delivery and mixed-DPI remain
unverified. Windows/X11/Wayland retain their existing drawing implementation.
Both hosts also expose **Triangle**, **Diamond** and **Star**. Preview vertices and
committed pixels share the renderer's normalized Rust polygon geometry, including
the star's 0.39 inner radius; Swift does not duplicate the geometry. Creation uses
the existing worker transaction, fresh-ID selection and output invalidation.
Zero-area and cancelled drags do not create a layer. All five closed kinds support
selection bounds and existing annotation controls. TypeScript creation/expansion
vectors and rendered geometry, C ABI, host gesture, undo/redo and draft tests cover
these paths. Linux X11 is exercised with software rendering; AppKit has automated
host tests/fixtures. Physical macOS input/accessibility, Windows and Wayland
presentation remain unverified, and this does not close a migration acceptance gate.
The AppKit **Layers** view connects fill/stroke toggles for closed shapes, annotation
color/width, opacity, and shadow color/opacity/blur/offset controls. Each change
submits a minimal shared patch live through the existing serialized worker and
invalidates encoded output; selection changes restore published values.
Rust projects resolved defaults separately from the authored document. Merely opening
controls does not materialize legacy fields or truncate full-precision numbers to the
three-decimal display. Disabled shadow fields cannot accidentally re-enable a shadow.
Styles remain editable on hidden/locked annotations. The scrolling panel has light,
dark, disabled and minimum-height error fixtures; automated macOS validation is not
physical input/accessibility/IME acceptance. Windows/X11/Wayland retain the existing
wgpu controls unchanged; this slice does not close their presentation gates.
Closing an unsaved session offers save-and-close,
close without saving the current session (retaining any older persisted draft), or
cancel. Quit drains accepted work and cancels termination if its draft save fails.
AppKit CI covers bridge/export/import lifetime, pending-edit ownership, locale-aware
geometry, drawing gesture cancellation and mapping, failure/close/output/import
behavior, real shape pixels/history/draft reopen, and rendered light/dark fixtures.
Physical AppKit input, accessibility and IME acceptance remain unverified.

Internal layer copy/paste is connected in both hosts through shared session
commands. Cmd/Ctrl C retains an immutable selected-layer snapshot without
changing the document, history, encoded preview or system clipboard. Cmd/Ctrl V
creates a fresh visible/unlocked layer after the current selection (or at the
front when no selection remains), offsets each successful paste by another 24px,
and switches to Select only after acceptance. Source edits/deletion/undo do not
replace the copied snapshot. Failed paste does not consume an offset; successful
paste uses existing render-before-publish, undo/redo, asset and draft contracts.
Clipboard state is per open editor and is cleared by successful discard or close,
not persisted or shared across windows. Copy image remains separate.
Focused text controls retain normal OS text copy/paste. The wgpu native-input
adapter preserves paste key-down even with an empty/unavailable OS clipboard,
without injecting text or changing clipboard contents. Unit and private-X11
tests cover snapshot ownership, command/focus gates, empty/nonempty OS payloads,
fresh selection, offsets, undo/redo and reopening; AppKit uses native CI tests.
Windows/Wayland share the implementation but physical input/presentation remains
unverified, as does physical AppKit acceptance. No platform parity gate closes.
Both hosts expose these layer actions through a native right-click menu. The
clicked stable layer ID, not the previous selection, owns Copy/Paste/Duplicate/
Delete. Opening or cancelling a menu preserves selection and encoded output;
stale IDs, busy work and confirmations reject dispatch. Locked layers remain
copyable/duplicable but cannot be deleted. Empty list space offers Paste. The
wgpu Duplicate button now waits for accepted fresh selection like its shortcut;
a failed duplicate keeps the prior selection. AppKit tests exercise native menu
targeting/dispatch; wgpu tests use secondary pointer/menu clicks, and private-X11
captures cover normal/minimum light/dark presentation. Windows/Wayland and physical
AppKit menu input/accessibility remain unverified.

Across both hosts, physical input/accessibility/IME acceptance remains open.
The current native screenshot editor is a functional workbench, not a visual match
for the shipping Tauri editor. Functional controls and inspected fixtures do not
complete the editor layout/interaction/design parity gate.
Both hosts now place the scrolling inspector on the right of the canvas. The wgpu
placement is exercised with real X11 input in all editor test modes and normal/
minimum-size light/dark fixtures; Windows and Wayland use the same implementation
but their physical presentation remains unverified. Both hosts center the Fit canvas, retaining the 2–100%
range and no-upscale cap. wgpu centers the actual document/output texture after
crop, resize and reopen; pointer mapping and zoom anchoring use that same rectangle.
AppKit now allows window resizing down to 760×540, matching the wgpu minimum.
The canvas and inspector grow, bottom controls remain anchored, and real size
changes cancel crop/drawing/selection/pan gestures while preserving manual zoom,
pan and the published draft. Same-size notifications do not cancel gestures.
Below 1000 points wide, AppKit moves canvas dimensions to a second footer row;
the inspector retains its width and scrolling access to every section's controls.
Windows/X11/Wayland keep their existing responsive layout; physical resize/input
acceptance remains open on all hosts.
Both hosts replace the pinned Copy/Save footer and Output section with the
shipping-style bottom export bar, available in Geometry, Layers and Draw at every
size down to 760×540 (the default AppKit window grows to 1000×780 to keep the
canvas area, shrinking to fit shorter displays' visible frames). The collapsed bar shows an **Export settings** disclosure with a
`PNG · 1920 × 1080 · ≈ 240 KB` summary, **Saving to** with **Change…**, the filename
with a format-suffix menu, **Copy image** (four-second **Copied** confirmation),
a **Save as new file** switch, primary **Save**, **Show in Folder** after a save and a
hint/status line. Expanding it reveals size, the quality mode and Compress preset
(each menu item carries its shipping description), **Maximum file size** as a value
with a KB/MB/GB unit, and **Est. size** with the % change from the original. The
native-only PNG colors and canvas preview groups are gone: while Compress or Maximum
is chosen and the settings are open, the canvas shows shipping's automatic
before/after comparison (`captures_app::compression_compare`; AppKit encodes through
`captures_editor_frame_encode_v1`) 280 ms after the last change, with size badges,
"% smaller", a draggable handle and **Hide** (**Show before / after** restores it). The estimate re-encodes 220 ms after the last edit or option change,
off the session worker, replacing the explicit preview requirement.

`captures_app::editor_export` owns the save model for both hosts (AppKit reaches it
through `captures_editor_export_bar_v1`, `captures_editor_save_v1` and
`captures_editor_estimate_v1`): Save overwrites the saved source by default; turning
on the switch suggests an `-edited` filename, and renaming, choosing another folder or
changing format saves a new file (a format change always does). Overwrite needs no
confirmation because shared Rust re-checks the History entry, saved path and format
at write time and publishes through a sibling temp file; the previous confirmation
dialog is removed. New files that collide with an existing file are refused with a
filename error. A saved file with a History entry is adopted as the next overwrite
target. As in Tauri, every successful Save (overwrite or new file, with no preference
gating it) then shows the saved file in its folder through the same reveal as the
preview card: Finder on AppKit, FileManager1 `ShowItems` or the folder fallback on
wgpu, where a failed handoff changes the notice to "Saved … — its folder could not
be opened"; **Show in Folder** reveals it again on request. CI records the reveal
requests (a stub FileManager1 on X11); physical file-manager selection remains
open. Copy still uses full-resolution edited PNG pixels. Both actions retain the serialized worker and accepted-work lifecycle. Native fixtures
exercise section/resize visibility, pending-work gates, overwrite/new-file/adoption
and estimate states; X11 export tests save from every section. Windows/Wayland
presentation and physical AppKit acceptance remain open, rather than being inferred
from shared code or rendered CI fixtures. The inspector follows shipping's
sections, with the remaining differences listed in the inspector sections slice. The comparison's split follows shipping
`CompressionPreview`: the round handle drags on both hosts even with a drawing tool,
the bottom strip and the focused split's range keys (arrows 0.1 %, Page Up/Down a
tenth of the 6–94 % span, Home/End) are off while drawing or processing, and
Preserve recentres it. Private X11 drags the handle and steps the keys; AppKit XCTests
drive the same handle and keys.
The viewport controls and every shipping drawing tool are connected (see the viewport,
drawing, Wand and brush slices).
Recording editing remains open on both hosts; the
screenshot-editor parity gate stays open.

New Capture connects its persisted shortcut, tray action and workspace entry to
fixed-glass screenshot controls on both hosts. Region, Window and Full screen
share one prepared Rust session and desktop snapshot, retaining selections across
target switches. A display replacement invalidates stale preparation and local
selections. Region confirmation reuses the existing audited crop/cursor policy;
countdown refresh and the cancellation/commit boundary are unchanged. The
controls include aspect selection, Enter/Escape and auto-start behavior; Record,
disabled in this first slice, is connected by the recording slices. Existing direct screenshot paths remain
available. Global region/window/display keys now switch targets inside the open
menu under its exact capture generation, without a new session or keyboard
Full screen auto-start, and New Capture switches it to Screenshot on Region. As in
shipping, the shortcut of the screenshot target already selected (New Capture on
Region) recaptures the menu instead (see the busy capture slice below). Leaving
selection clears held/pending target keys before preparation or countdown. Recording, physical
platform input/display acceptance and full capture-menu visual/accessibility
parity remain open.

The capture-menu parity slice moves shipping `RecordingSelector` copy and small
policies into `captures-app::capture_menu`, exposed to AppKit through
`captures_capture_menu_v1` (JSON, freed with `captures_settings_free_v1`) and an
allocation-free guidance hit test. Both hosts now compute the controls-visibility
note from recording capabilities ("These controls **won’t**/**will** show in
screenshots|recordings"; Linux adds the Hide controls hint and never links). Where
controls can be excluded the note, and "Auto-capture is on…", link to Preferences:
the menu closes and Preferences scrolls to that row and highlights it for the
shipping 2.4 s. Full screen shows the display identity (OS name, W × H, and
"· N FPS" in Record) instead of guidance. The recording row uses labelled
FPS (60/30/15) / Max resolution selects, Show cursor / Show clicks / Desktop audio
switches with On/Off/Unavailable text and unavailable-reason tooltips, coupled
cursor/clicks, and the microphone select's "Selected microphone" and "Loading
microphones…" states. The primary button uses
the shipping labels and hides under auto-start unless a start failed; AppKit Record
now honors auto-start like shipping and wgpu, while tray/shortcut Record Full Screen
never auto-starts. Guidance uses the shipping copy and chip placement, stays until a
window is selected, hides while dragging a region and fades within 28 points of the
pointer (12-point leave slack). The wgpu region drag also settles at the release
point when a slow frame batches the release with later motion. The segmented
indicators slide and the Record row arrives as shipped (see the motion slice below);
both hosts draw the shared segment icons (wgpu `SegmentGlyph::Icon`), and Full screen
now shows shipping's `.recording-display-icon` above the display identity: a 68 × 50
glass tile (`--glass`, `--glass-border-strong`, `--r-xl`, `--glass-shadow`) holding the
34-point display icon at a 1.4 stroke, one `--s-4` gap above the name, with the group
still raised 60% of its height. Wayland remains open.

The menu's in-flight states now follow shipping `RecordingSelector` on both hosts,
through the shared `capture_menu::primary_action` state. A start shows "Capturing…"
(Screenshot) or "Starting…" (Record) and disables the primary; under auto-start the
hidden primary reappears for it. Shipping hides the selector as soon as
`capture_selection_screenshot` begins, so a screenshot's "Capturing…" lasts only
until the host closes the menu (a frame or so); a Record start keeps the menu up with
"Starting…" until the take is prepared (the recovery bundle, and on AppKit the FFmpeg
check), as `start_recording` does until its HUD is ready. Choosing another display in
Full screen keeps the current menu, its snapshot and its selections, up with
"Switching…" until the new display's session is ready, then replaces it (wgpu declares
one viewport per monitor). While either is in flight, further starts, display changes
and shortcut routing are ignored and Escape or Close still cancels.

A failed Record start or display switch now keeps the menu open on both hosts, as
shipping `RecordingSelector` does when `start_recording` or `select_capture_display`
returns an error: the in-flight label ends, the mode, target, region, window, aspect and
Record options stay, a switch returns to the display it was on (its session, snapshot
and select value), and the error shows inline until the next start or switch clears it,
so the user can retry or choose something else. Under auto-start the hidden primary
returns as Retry capture / Retry recording. The error is shipping's
`.recording-selector-error`: the panel's last row, a full-width band under the note
with a `--danger-border` top rule, `rgba(--theme-signal-rgb, 0.16)`,
`--theme-signal-text` at `--text-sm`, `--s-4 --s-5` padding and the panel's `--r-2xl`
bottom corners (wgpu `capture_controls::show_error_band`, AppKit
`CaptureMenuErrorBand`, which also posts a VoiceOver announcement for `role="alert"`).
Inline, as in shipping: the recording toolchain check, a vanished display, target
validation, the recording draft (`RecordingSession::prepare`, on AppKit also the
FFmpeg check) and the new display's session or snapshot. The host dialog stays only
where shipping's selector is already gone: a screenshot capture (shipping hides the
selector first), the countdown or HUD after a prepared take, and failures opening the
menu. Escape with a menu select open matches shipping on wgpu: shipping's selector
takes Escape on `window` in the capture phase, so one press closes an open
`CustomSelect` and cancels the capture ("lets one Escape close an open control and
cancel the selector exactly once"), and wgpu reads Escape before its select and closes
the listbox on the same press. AppKit's `NSPopUpButton` menu tracking consumes the
first Escape natively (it closes only the menu) and a second press cancels, an accepted
platform-native difference. Verified with wgpu unit tests and AppKit XCTest sources.
Microphones enumerate as shipping `loadAudioDevices` does: once per menu, the first
time it shows Record with a microphone available (no longer when the menu opens, and
kept across a display switch), with the select disabled and "Loading microphones…" /
"Loading microphone…" until the list arrives. Rust unit tests and AppKit XCTests (not
run here) cover the labels, the disabled and hidden primary, blocked starts and the
one-time microphone request; the private-X11 capture and recording smokes exercise
the menu's start paths.
Verified with Rust/XCTest source tests and private-X11 capture/recording smokes;
AppKit compiles and runs only in macOS CI, and Windows presentation is unverified.

The overlay guidance slice gives the direct Region and Window overlays the shipping
`CaptureGuidance` chip that New Capture already drew, one implementation per host
(AppKit `CaptureGuidanceChip`, wgpu `capture_controls::paint_guidance`). The
two-row glass chip sits 16% from the top, fades and slides in from 6 points higher,
hides while a region is dragged out and fades within 28 points of the pointer
(12-point leave slack). The window overlay switches to "Click to capture this
display" over the desktop or shell chrome. A click without a drag re-keys the chip
like shipping: the "Click and drag" copy, an 80% accent border and the sideways
nudge for 1.8 s. Placement, feedback timing and the pure pose model
(`capture_menu::GuidanceChip`) live in `captures-app`; the fade, slide and nudge
are `motion` catalog entries, so reduced motion lands every change at once. The
manual (confirm with Enter) fixture mode adds "Press Enter to confirm" to the hint
row on both hosts. Both hosts draw its `--glass-shadow` (see the preview effects
slice). Verified with Rust unit tests,
XCTest sources and the private-X11 capture smoke, which checks the chip's 16% top
edge and ducking with settled pixels; AppKit runs only in macOS CI, and Windows and
Wayland presentation are unverified.

The motion slice moves shipping animation into `captures_app::motion`: each
shipping `@keyframes` rule with its `animation` timing, and each `transition`,
as data whose durations and easings name the `--dur-*` / `--ease-*` tokens
(literal values only where the shipping CSS hard-codes them, such as the 0.52 s
preview arrival). `prepare.mjs` exports the `--ease-*` tokens as cubic-bezier
control points; wgpu samples poses with the shared solver, and AppKit reads the
same keyframes through the settings ABI's `motion` operation and hands them to
Core Animation as presentation-only animations, so model frames and alpha stay
settled for code and tests that read them. Both hosts now play:

- update notice `ui-pop-in` (`--dur-4`) and the restart exit 3 s into the restart
  state (the stub keeps the faded card until the fade ends);
- launch notice `startup-arrive`, rising from below when the caret points down;
- the recording-saved (15 s) and controls-hidden (6 s) lifecycles: arrive, hold,
  and fade out ending 200 ms before the window closes. A save that extends the
  saved notice's life holds it steady instead of replaying the entrance;
- mini-preview `thumbnail-arrive` for each newly decoded card, including its
  3 px media blur;
- countdown scrim/content fade-in and the cancelling fade-out;
- capture menu `recording-options-arrive` on the Record row, and sliding
  `.capture-segmented-indicator`s for Screenshot/Record, Region/Window/Full
  screen and the Preferences Appearance control (`--dur-4` `--ease-standard`);
- the Preferences save-status pop-in (`--dur-2`);
- History card hover lift (2 pt over `--dur-3`) with its `--shadow-sm` →
  `--shadow-md` shadow; wgpu also eases the border.

Reduced motion follows the shipping global rule (0.01 ms animations and
transitions): entrances and transitions land at rest with no frames between,
and an exit keeps its delay and lands on its final keyframe. The two lifecycle
notices are the exception: shipping's rule would jump straight to their
invisible final keyframe, so both hosts keep them still and visible until their
windows close. The existing system Reduce Motion wiring (AppKit workspace
setting; wgpu portal/Windows preference or `--reduced-motion`) drives all of it.
wgpu requests repaints only while a pose changes, and its motion wrapper keeps
widget ids stable when an animation settles. The preview arrival's 3 px blur and
the History hover shadow now play too (see the preview effects slice), as do
segment label colours (`color var(--dur-3) var(--ease-standard)` toward the
hovered or active label, `segment_label`) on the Preferences segmented controls
and the capture menu's Screenshot/Record and target switches, and the shared
select listbox's `ui-pop-in` (`--dur-2`, `--ease-out`) in wgpu. AppKit's capture
menu segments now rest on `--glass-text-muted` like shipping instead of always
`--glass-text`. Not reproduced: AppKit's History hover border easing, pop-ins on
AppKit's select menus (system menus) and on the wgpu screenshot editor's Format
and Output size menus, which are still egui `ComboBox` popups where shipping uses
`CustomSelect` (its crop aspect and font menus are native `<select>`s). Confirmation dialogs are native in shipping and have no
entrance to match. Verified with Rust/XCTest source tests and private-X11 smokes;
AppKit runs only in macOS CI, and physical macOS/Windows motion acceptance remains
open.

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
and full capture-menu UI parity remain required. Shipping's overlay has no magnifier and
only dims (`capture.css`: "never blur"); the editor's Wand loupe, which both hosts have,
is its only magnifier, and the glass panels' `backdrop-filter` stays with the other
backdrop-blur work.

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
