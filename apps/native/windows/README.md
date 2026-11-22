# Captures Windows host

The Windows replacement for the egui/wgpu workbench: Win32 windows,
DirectComposition, Direct2D, DirectWrite (system Segoe UI Variable) and UI
Automation, with Captures' own token-drawn controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. It has Preferences (every shipping
card with the Custom accent's color editor, the Capture card's mini preview,
screenshot format and countdown rows, an editable Save captures to field that
keeps a blank draft unsaved like Tauri, the narrow layout below 720 px,
Find on a reusable text field, `src/textfield.rs`, and page copy you can
select and copy like Tauri's: drag, double-click a word, triple-click a
paragraph, Ctrl+C; a click on a switch row's copy still toggles it), Capture History
(`--scene history`) and a first screenshot editor slice, opened by History's
Edit or thumbnail or `--scene editor --artifact ID` (`src/editor.rs`). Dragging a
History thumbnail carries the capture's file into another app like shipping's
`tauri-plugin-drag`: an OLE `DoDragDrop` of the shell data object (`CF_HDROP`),
copy only, with the 284 × 160 preview as the drag image (`src/drag.rs`, shared
with mini previews; the file comes from the shared `PreparePreviewDrag` export). A right-click
(or Shift+F10 / the Menu key on a focused card) opens the card's native menu
from the shared `history_view` list, as GTK and macOS do: Edit, Restore, Copy
image (PNG and `CF_DIB`), Save image (the Captures folder in the screenshot
format, like macOS) and Show in Folder for screenshots; Edit and Save file or Show
in Folder for recordings.

Recordings open from History's Edit or thumbnail or `--scene recording --artifact ID`.
Tauri's editor-wide keys apply: Ctrl+S saves from anywhere; with no control
focused and no menu open, Space plays or pauses, I and O set the trim start or
end to the playhead, Home/End seek to the trim edges and ←/→ step one 30 fps
frame (Shift+←/→ one second). Save and Play keep their buttons' rules.
Play/Pause, the trim handles, Maximum file size's Est. size, the save folder,
Save as new file and Save show Tauri's `title` text after the system hover
delay through the shared glass tooltip (`src/recording/tips.rs`; longer copy
wraps at 180 px, Save's sits above the footer); a press or key hides it until
the pointer leaves. `--select tip-play|tip-trim-start|tip-trim-end|tip-estimate|tip-destination|tip-copy|tip-save`
renders one at rest.
When an editor closes in the resident app and the take is still in History,
shipping's "Recording ready" notice (`src/recording_ready.rs`) appears 18 px
from the primary display's right edge and 30 px below its top on the fixed
glass palette: Save file keeps a copy in the Captures folder through the
shared `SaveRecording` and turns it into "Recording saved" with Show in
Folder (an already-saved take opens that way; Show in Folder dismisses it
once Explorer opens), errors replace its line, and × or a new capture closes
it. It never takes focus, is excluded from captures, exposes a status, the
action and Dismiss to UI Automation, and plays `recording-saved-lifecycle`
over its 15.2 s life (held still under reduced motion). `--scene capture
--tool recording-ready --select saved|saving|error|hover|dismiss-hover|arriving`
renders it. Checked live on Windows 11 at 150% through UIA and posted
messages (placement, capture exclusion, Save file, Dismiss, the × click and
the timeout); Show in Folder and the motion were not watched.
The first slice (`src/recording.rs`) contains the header, dropped-frame warning
and save footer: filename editing, Change save location, Save as new file with
the `-edited` suggestion, replacement, progress, Cancel and Show in Folder.
The shared recording session and media tools run on a worker; closing cancels
work and waits for replacement/history cleanup before destroying the window.
The preview displays retained decoded frames, plays/pauses video with source
audio and Loop preview, and autoplays GIFs without video-only controls.
Fit contains the media; 100% maps source pixels to DIPs and scrolls with the
wheel (Shift+wheel horizontally). The media worker retains only its newest
frame while the UI is busy; pause/restart generations discard stale frames.
The scrollable page has a source-relative thumbnail timeline with draggable
trim handles, keyboard steps and UIA RangeValue. Output cards use the shared
recording-editor menus: original/1080p/720p/custom dimensions, GIF frame rate,
maximum width and quality presets (64/96/128/256 palette colors), plus separate
0–200% source-audio volumes, mute and mono. Custom dimensions export as even
pixels. Valid edits debounce for 250 ms; invalid drafts stay local until blur.
Save waits for the accepted preview settings, and failed previews restore those
settings. Numeric fields expose UIA Value, menus ExpandCollapse/SelectionItem,
volumes RangeValue and audio checkboxes Toggle. Output controls passed physical
pass 25; pass 26 accepted source-sized preview and playback independent of export
dimensions, including stable custom-field steppers and retained audio edits.
Graphical crop has eight handles and interior movement; numeric source-pixel
bounds, aspect lock, keyboard movement and UIA ranges share the crop geometry.
Crop passed its separate hardware check with observations; comparison still awaits
physical acceptance. Wine tests and offscreen renders are not that gate.
MP4 Preserve/Compress presets and GIF compression use
the shared encoded comparison, with a 6–94% split, keyboard/UIA range, Hide/Show,
processing/error feedback and estimated size badges. New edits cancel old work;
only matching generations, accepted revisions and source positions can display.
Comparison hides during MP4 playback; crop controls remain above it.
GIF comparison is a static sample; Hide restores source autoplay.
Maximum file size uses the shared MP4/GIF hard-budget exporter, decimal KB/MB/GB,
a 10 MB default and a 100 KB minimum. Size drafts stay untouched on blur; Save
reports an invalid draft instead of exporting the last accepted cap. Unit changes
preserve exact whole bytes (unlike Tauri's eight-significant-digit formatter).
The comparison displays the cap rather than an estimate. Replacement resets to
source defaults and clears the cap. Physical pointer, UIA and DirectComposition
acceptance for these controls remains open. The filename's Format menu switches
between MP4 and GIF, preserving trim, crop, audio settings and size limits.
Changing formats forces a new file instead of overwriting the source; returning
to the source format allows replacement again. GIF-only frame rate/palette
settings do not leak into MP4 exports, and source playback stays independent of
the export format. Videos honor the saved output-format preference; GIF sources
start as GIF. WebM remains selectable to match Tauri, but saving it reports
unsupported and writes no file. The menu shares the existing keyboard and UIA
ExpandCollapse/SelectionItem behavior; physical interaction acceptance remains open.
The filename exposes UIA Value, the copy switch
and preview modes expose Toggle, and actions Invoke. Timeline/playback passed
physical pass 24; Narrator/Text-pattern and recording-editor parity remain open.

```powershell
node apps/native/prepare.mjs --output apps/native/windows/resources
cargo +1.95.0 run --manifest-path apps/native/windows/Cargo.toml -- `
  --profile $env:TEMP\captures-windows
```

Run it against a disposable profile, never your real settings or folders:
`--profile DIR` (or `CAPTURES_NATIVE_PROFILE=DIR`) keeps settings in
`DIR\settings.json`, history in `DIR\history` and a new profile's saves in
`DIR\Captures`, and never writes the launch-at-login Run key. It replaces
`--settings-file` and `--history-root`, which remain for CI and fixtures.

`--history-root DIR` enables Start Captures on login for that development
profile. `--appearance light|dark`, `--size WxH` and `--screenshot out.png`
render one Preferences, History, screenshot-editor or recording-shell frame offscreen
(Direct2D bitmap, WIC PNG) and exit, for parity
comparisons; `--find QUERY` renders it with Find open and the query typed, and
`--tool KEY` starts the editor with a rail tool or shape, `--select LAYER`
with that layer selected (`--select drop` or `drop-stack` with image files
dragged over the canvas: the default guide, or the pointer over the image;
`--select export` with Export settings open, `export-custom`, `export-compress`
or `export-maximum` with that size or quality, `export-menu` with the Save
quality list open), and
`--crop X,Y,W,H` with that crop staged. On other platforms the binary is a stub, so `cargo check` and
`clippy` still run; cross-check with `--target x86_64-pc-windows-gnu`.

`--live` runs the resident app (`src/live.rs`): the first launch for a profile
wins the shared instance election and later launches forward to it and exit;
`--captures-autostart` makes it quiet. The notification-area icon
(`src/tray.rs`, the `tray-icon`/`muda` Win32 menus shipping uses) has the
shipping menu and accelerators, left-click opens Preferences, and closing the
last window keeps the app resident. New Capture and Screenshot Display open
the Screenshot capture bar (`src/capture.rs`) over a frozen snapshot of the
display under the pointer, on Region or Full screen. Drag a region, or switch
to Window and click the window under the pointer (the desktop picks the entire
display), then press Enter or Capture (Esc or Close cancels); the countdown
applies, the result is copied to the clipboard and History reloads. One shared
`WindowSession` snapshot serves all three targets. A screenshot delay shows
shipping's countdown scrim ("SCREENSHOT IN", the number and "Press Esc to
cancel") on the selected display with the shared entry and exit motions; Esc
cancels it and the capture reads fresh pixels once it closes. Like shipping,
a drawn region shows its W × H label and four corner handles: dragging the frame
moves it, a handle resizes it, and a press elsewhere draws a new one; Shift
squares a drag (pressing or releasing it mid-drag re-runs it) and the Aspect
select (Free, 1 : 1, 4 : 3, 3 : 2, 16 : 9, 9 : 16) locks drags and refits a
drawn region around its center (--select aspect|aspect-top renders it). On
Full screen the Display select lists every display (name, then W × H and
Primary; disabled with one display) and reopens the bar on the chosen one,
keeping its target, mode, aspect and Record options while the primary reads
"Switching…" (`--tool display --select display|display-switching` renders
it). Like shipping, the bar shows the capture guidance chip 16% down the
display (Region's "Drag to select a region", hidden while drawing; Window's
"Select a window to continue" or "Click to capture this display" until a
window is picked) and fades it away from a nearby pointer; Record's options
row slides in, Start recording's dot pings while it is enabled, and lists pop
in, all on the shared motion specs and at rest under reduced motion (rendered
offscreen and unit-tested; the live motion was not watched). Like shipping,
Window is disabled (for shortcuts too) when the display lists no windows, and
it and unavailable Record switches show shipping's `title` copy in the host's
glass tooltip (`--select window-unavailable` renders it; hover not exercised
live). A physical Windows 11 check (4K, 150%) confirmed pixel-exact
Region and Full screen captures, the clipboard and History; a second New
Capture now refocuses the open bar instead of an error, and the snapshot waits
for the tray menu and overflow flyout to close (a recheck passed both; the
wait adds about 0.5 s before the overlay appears). Like shipping, Screenshot
Region and Window open a bar-less selector with the guidance chip: Region
commits on release (a click without a drag nudges the chip, which reads
"Click and drag to select a region" with an accent border for 1.8 s) and
Window on click (`--select direct|direct-feedback` renders them; unit-tested,
not physically clicked). After setup, `--live` registers the saved capture shortcuts with the
shared `CaptureShortcuts` owner, which takes over Print Screen and Win+Shift+S
from Snipping Tool like shipping: New Capture and the screenshot shortcuts open
the bar on their target, the recording shortcuts open it in Record, and while
the bar is up every chord and tray row routes to it through the shared
`busy_route`: another mode or target switches it in place, and the same
screenshot target again (or any chord over a bar-less selector) freezes the
screen with the open UI in it and opens the bar or selector the route names
on that snapshot (no countdown; Full screen saves that snapshot). Unit-tested;
no chord was pressed physically while the bar was up.
An active Preferences shortcut recorder releases the registrations. A key
press skips the tray menu's 200 ms fade wait. A physical Windows 11 check
(4K, 150%, injected input) passed every registered chord, the tray rows, the
recorder's release and Quit; key press to bar took about 380 ms in a release
build (260 ms without Freeze screen). The Win+Shift+S hook ignores injected
input and the installed Captures held Print Screen, so those two takeovers
still need a real keyboard check. The tray's Send Feedback and Preferences'
About › Send feedback › Open open shipping's form (`src/feedback.rs`, 640 × 700,
minimum 460 × 460): the automatically included app and system details, the
Bug/Idea/Other categories with their placeholders, a multi-line message, the
optional contact and Send with its sent and error notes, using the shared
`captures_app::feedback` copy and limits. Only the resident app sends, through
the shared `captures-feedback` client on a worker; other launches are a fixture
whose Send stays off. Tab moves through the categories, fields and Send.
`--scene feedback [--select typed|live|sent|error|idea]` renders it offscreen
(compared with the Tauri harness; Wine's DirectWrite stubs caret hit-testing,
so caret placement needs a real PC). Not ported: the textarea resize grip and line-based Home/End.

Crash diagnostics: the resident app starts the shared
`captures_app::crash::Session` (session marker, panic hook and the hidden
window that marks a `WM_QUERYENDSESSION`/`WM_ENDSESSION` logoff clean and
restores the marker when shutdown is cancelled) and marks a clean exit on Quit
before releasing the profile. A visible launch after an unclean exit opens Send
Feedback with the native hosts' previous-session review card (as GTK: title,
explanation, a 128 px mono summary that scrolls, Copy summary, Add to message
and Dismiss diagnostics); nothing is sent or added unasked. `--scene feedback
--select crash` renders it.

Record (`recorder.rs`) switches the bar to Record ("Start recording"; its
note follows Include recording controls in captures), and the Record Region,
Window and Display tray rows and shortcuts open it there. Its options row
(shipping `.recording-options-row`: FPS and Max resolution selects, Show
cursor, Show clicks and Desktop audio switches, and the Microphone select,
enumerated off the UI thread when Record first shows) starts from the saved
recording settings and applies only to this take, like shipping; the open
lists take the pointer and arrow keys, Enter and Esc. Keyboard focus through
the bar's controls and UI Automation are not ported. Confirming hands the capture flow
to the shared recording worker, which verifies FFmpeg, prepares the session,
then shows the "RECORDING STARTS IN" countdown (Esc cancels the take) and starts.
The controls (`hud.rs`, shipping `.recording-hud` at 430×102) float centered
20 px above the bottom of the display: a topmost, non-activating window,
excluded from captures with `WDA_EXCLUDEFROMCAPTURE` unless Preferences
includes it, dragged by the card. They show the status dot, elapsed time and
state, and Stop, Pause/Resume, Restart, Mute with the microphone level, Delete
and Hide, with state and copy from `captures_app::recording_hud`. Restart and
Delete confirm with a system message box (OK/Cancel). Stop saves to History,
adds a mini preview and opens the recording editor when Preferences asks;
Quit saves an accepted take. New Capture or the tray brings hidden controls
back. The HUD's Screenshot opens the capture bar on Region with Record
disabled while the take runs; included controls are kept out of its snapshot
(`WDA_EXCLUDEFROMCAPTURE` while it prepares). Hovering any button, enabled
or not, shows its shipping tooltip under it (the last three right-aligned),
fading in and out over `--dur-1` through the host's shared glass tooltip
(`src/tooltip.rs`, instant under reduced motion). UI Automation exposes the controls in shipping order with
their `aria-label` names (privacy line, time, state, each button with
Invoke, microphone level, the error line as an assertive alert) and
announces name flips such as Pause/Resume. Hide shows shipping's
"Recording controls hidden" notice centered where the controls were, with
the New Capture keys as chips: topmost, never activated, click-through
(layered + transparent), excluded from captures, closed after 6.2 s or when
the controls return, with shipping's 6 s fade-in, hold and fade-out (still under
reduced motion). By the
owner's decision the controls never take keyboard focus, so a click never
pulls typing away from the recorded app; unlike shipping's Windows webview
they have no Tab order (UI Automation still invokes every button). A take interrupted by a
crash is listed in History's Interrupted recordings section (shared
`RecordingRecovery`, shipping copy and two-step Discard; Recover and Discard
run off the UI thread and are reachable by Tab and UIA; offscreen render with
`--scene history --select recovery`). The HUD
and Record bar match the Tauri harness offscreen
(`--scene capture --tool hud [--select paused|failed|hover] --size 430x102`,
`--tool region --select record`). A physical Windows 11 check (4K, 150%,
injected input) passed Region, Window and Full screen MP4s without the
controls, the countdown and Esc, Pause/Resume, Restart/Delete confirmation,
microphone mute, focus and drag, included controls and save-on-Quit. It found
that the New Capture chord didn't restore hidden controls (hidden controls
now make it a restore chord, like GTK) and the missing "Use Hide controls to
keep them out" hint; both are fixed. Frame rates were far below the 60 fps
setting (about 4 fps at 4K, 7 for a large window, 16–30 for regions) at about
5% CPU, likely in the shared capture pipeline; compare with Tauri before
treating it as a native regression.

When Show mini previews is on,
each saved screenshot joins shipping's floating preview stack (`previews.rs`):
a topmost, non-activating window placed with the shared `thumbnail_geometry`
on the display under the pointer, hidden while New Capture is open. Cards show
the size chip and Copied to clipboard; hover blurs and dims the image under
Close and Delete (Delete alone dismisses an unsaved capture; a saved file goes
to the Recycle Bin), Edit, Copy and Save file / Show in Folder, plus Clear all.
A stopped recording joins too, as on GTK (the Tauri app showed no recording
cards): its card has no Edit or Copy, Save file writes the video and Show in
Folder reveals it, and automation names it "Recording preview". A failed
action shows its error on the card (shipping's `.thumbnail-message`) until
the next action.
History's Restore brings a screenshot back as a card and reads "✓ Restored"
for 2.5 s. UI Automation lists every card's buttons (Invoke, whether or not
the card is hovered), its size line and clipboard status, and Clear all
previews, through the shared `panel_uia.rs` (unit-tested, not yet checked with
Narrator). With two or more cards the toolbar shows Clear all and Minimize
(shipping hides it for one card). Minimize collapses the stack into
shipping's pile: the newest card in front and older cards peeking from the
anchored edge in the shared `collapsed_card_pose`, under their depth shade,
fanned while hovered; the pile is one "Expand N previews" button. Minimize and
Expand fly the cards between the list and the pile, and hovering fans it
with the per-card stagger, on the shared `preview_stack_fly` /
`preview_stack_fan` tweens (instant under Windows' reduced motion); the frame
keeps its expanded height until a collapse lands. The pose is drawn as its
flattened affine (no keystone or rear-card blur). Top placements peek down,
bottom ones up. Past 8 DIPs a press on the pile carries it instead of
expanding: the pile follows the pointer on its display, re-anchors to the
nearest corner through the shared `move_collapsed_stack` (expanding toward
it) and its rear cards lean with the shared `DragSway`; the carried spot
lasts until the stack empties. A stack taller than its window keeps the
newest card in view and scrolls with the wheel (100 DIPs a notch) or
shipping's overflow cues, glass chevron tabs at the window edges that ease
one card slot (380 ms, instant under reduced motion) and are named "Show
older captures" / "Show newer captures"; a new capture or Expand returns to
the newest card. Dragging an expanded card's image past the system drag
threshold drags its file into other apps through the shared `drag.rs` (the
shared `PreparePreviewDrag` export, a copy with the 284 × 160 drag image);
the card goes hollow while it runs (`.thumbnail-file-dragging`). An accepted
drop outside Captures dismisses the card, a drop back on the stack shakes it
(`thumbnail-drop-reject`, skipped under reduced motion), and any other end,
including a drop on another Captures window, keeps it; a failed export shows
on the card.
`--tool previews --select pile|pile-hover|pile-top|overflow|file-drag`
renders it (compared with the Tauri harness). Scrolling, the cues, Minimize
and Expand were checked live through UI Automation with History's Restore on
Windows 11 at 150%; the carry, the wheel and file drags are unit- and
render-checked only (not physically dragged). Close, Delete, Edit and Clear
all show shipping's preview tooltips (a glass chip 6 DIPs above the control
in bottom stacks, below it in top ones, fading in 120 ms, Clear all over
`--dur-1`) on the host's shared `tooltip::Fade` (`--select
tooltip|clear-tip`; unit- and render-checked). Clicks never take focus
(shipping's window was `focusable(false)`), but the buttons are keyboard
focusable for assistive tech: UI Automation SetFocus (Narrator) activates the
stack with that button focused, Tab and Shift+Tab walk the buttons in
automation order, Enter or Space presses one, and Escape collapses an
expanded stack as on GTK. The focused control shows shipping's
`:focus-visible` ring (70 % accent on card controls, a 2 px accent outline
2 px out on the toolbar, cues and pile), its card shows its controls and its
tooltip shows (`--select focus|pile-focus`; unit- and render-checked, and
UIA reports the buttons focusable; SetFocus and keys were not exercised live
because they take focus from the desktop's user). Cards arrive and leave with
the shared preview motion, and a new card holds shipping's capture highlight
(a 1 px accent ring with an 18 px accent glow) for 1 s, then fades it over
3.5 s, skipped under reduced motion (`--select highlight`). Hovering eases
the card's look like shipping's transitions: the image's blur and dim over
0.18 s and its zoom over 0.22 s, the controls fading in over 0.16 s with the
top row sliding down 3 DIPs, and the size chip fading over 0.15 s, each
reversing from where it is (instant under reduced motion; unit-tested, the
settled states rendered). The hovered pile sparkles like shipping's: two
layers of accent and white dots from the shared `preview_motion` tables rise
and flash on their 1.8 s and 2.2 s loops, stopping while it is carried or
under reduced motion (`--select pile-sparkle`). `--scene capture --screenshot PATH`
renders the bar offscreen (`--tool display` for Full screen, `--tool window`
for Window, `--select x` for a sample selection or hovered window, `--select
desktop` for the hovered display, `--select picked` for a picked window with
the pointer on the desktop, `--tool countdown` for the countdown at 3,
`--tool previews` for the preview stack, `--select x` hovering its first card). While
first-run setup is unfinished, launch, relaunch and New Capture open shipping's
setup window (`src/setup.rs`, 620 × 560, minimum 480 × 440): the mark, copy,
the Screen capture row with its Ready status (Windows has no permission prompt
or microphone row) and Start capturing, with the check and completion running
off the UI thread through the shared `captures_app::onboarding` session. Tab
focuses the button and Enter or Space finishes setup, which closes the window
and enables the capture shortcuts. `--scene setup [--select
checking|error|focus]` renders it offscreen (rendered against the Tauri harness
in light, dark, short and tall sizes; not yet physically checked). Finishing setup
shows shipping's launch notice for 15 s, and a quiet (`--captures-autostart`)
launch shows it for 5 s (`src/startup_notice.rs`): "Captures is ready to use"
with the New Capture keys, a non-activating pill whose caret points at the
notification-area icon through the shared `tray_notice` placement, retrying
while the icon has no rect and then falling back above the taskbar without a
caret. Close or a tray left-click hides it early. `--scene capture --tool
startup-notice [--select fallback|hover]` renders it offscreen (compared with
the Tauri harness; not yet physically checked). It arrives with shipping's
`startup-arrive` (`-from-below` toward a taskbar icon), and setup's ready
Start capturing button pulses its `--surface-active` halo (`--scene setup
--select pulse` renders its peak); both are still under reduced motion.

Setup, Send Feedback and the launch notice share one UI Automation provider
(`src/panel_uia.rs`) built from each window's parts in shipping's DOM order and
roles: headings, the setup row as a polite status, the feedback categories as
radio buttons in a "Feedback category" group (Selection), the message and
contact as Edits (Value, placeholder and hint as help text), Send, Start
capturing and Close as buttons (Invoke), and sent/error notes as status/alert
lines. Focus, selection, value, enabled and name changes and new status or
alert lines are raised as events, and the notice announces itself when it
appears. Its event diffing is unit-tested; nothing has been checked with
Narrator yet. Files forwarded by path are not opened. A physical Windows 11 check (4K, 150%) passed quiet launch, the
menu and its disabled rows, left-click, residency after closing windows,
relaunch forwarding and Quit (before shortcuts were registered). Wine
cannot render Preferences (no DirectComposition) or open the tray menu.

Recordings need FFmpeg and FFprobe. `CAPTURES_FFMPEG` and `CAPTURES_FFPROBE`
override discovery; otherwise the host looks beside its executable, in Tauri's
`apps/native/bundle/binaries` (`*-<arch>-pc-windows-msvc.exe`), then on PATH
as `ffmpeg.exe` and `ffprobe.exe`. No tools are downloaded by the host.

`node scripts/native-preview-build.mjs build x86_64-pc-windows-msvc VERSION
--unsigned` (MSVC, Rust 1.95 and the FFmpeg sidecars from
`scripts/build-ffmpeg-sidecars.sh`) packages this host, built in release in
`apps/native/windows/target`, as the NSIS installer
with the sidecars and `native_preview_update_helper`; see
[releases.md](../../../docs/releases.md#native-packaging-and-recovery).
Add `--side-by-side` for a test installer that installs next to an existing
Captures: it is named "Captures Native Test" with identifier
`io.github.joswayski.captures.nativetest` (its own install folder, uninstall
entry and shortcuts), runs as `captures-native-test.exe` so the installer never
closes the real `captures.exe`, registers no file associations and makes no
updater artifacts. It is compiled with `CAPTURES_NATIVE_PREVIEW_TEST`, so it
never checks the public update feed, downloads or installs an update, or runs the
startup-recovery repair (an earlier test build's recovery companion tried to
download the real release after a failed first launch). Its first launch still imports the installed Captures
profile (quit Captures first). It never enrolls in launch at login on that
import, and its Preferences toggle uses its own `CapturesNativeTest` login
item (Run value, LaunchAgent or autostart entry), so the installed Captures
login item is left alone. Its tray icon is named "Captures Native Test".
The installed executable is `captures.exe`, which answers
`--preview-build-info` with renderer `win32`. Such a build starts `--live`
without flags on the imported native profile (unless `--profile` or explicit
paths are given) and holds the package-use lock. Before profile import it arms the shared startup
recovery companion (`updater::preview::recovery::Startup`) and acknowledges its
readiness file (or the installer's `--native-update-ready-file`/`-token`) once
settings load, a Direct2D device starts and the bundled FFmpeg/FFprobe verify;
forwarding to a running instance and a normal Quit finish it. An update restart
reopens the previews, editors and Preferences recorded in the shared restart
intent. Plain path and `file:` URI arguments (Open With, file associations),
`--open-media PATH` and everything after `--` open each file through the shared
`OpenMedia` import: screenshots in the screenshot editor, videos and GIFs in the
recording editor, one at a time; a second launch forwards its files to the
resident app. Development builds accept files only through `--open-media` or
`--` with `--live`. The resident app runs the shared `CheckWorker::preview` at
launch and every five minutes; Preferences' Updates card (`src/updates.rs`)
shows shipping's copy and states (Check Now, Update now with progress, Up to
date, “Couldn’t check for updates”, “Update failed” with Try again) and
the tray's Check for Updates checks and scrolls to it. Update now downloads and
verifies, then hands the package to the copied `native_preview_update_helper`
and quits; it refuses while a capture or recording runs. Render the card with
`--scene preferences --select update-idle|current|available|downloading|failed|check-failed`.
The floating update notice (`src/update_notice.rs`) is shipping's solid
`--surface-raised` card in a non-activating, topmost DirectComposition window,
placed by the shared `update_notice::placement` with a caret toward the tray
icon (caretless tray-corner fallback without one). A newly available version
opens it once per session, deferred while a capture or recording runs; an
install started from it reopens it on failure; the tray's Check for Updates
shows it while an update is available. The tray follows the same status
through the shared `update_notice::tray_update_item`/`tray_tooltip`, like
shipping: "Update Available — N changes" pinned with a separator above New
Capture, then "Installing Update…" or "Restarting Captures…" (disabled), and
"Checking for Updates…" in place; the tooltip reads "Captures — Update
available" or "Captures — Installing update". Not now/Close hide it (not while
downloading or restarting); release notes follow the Preferences
`show_update_changelog` switch, with no toggle on the notice. Update now / Try again / Check again run the same steps as the Updates card,
View release, pull request numbers and "download from captur.es" open the
browser, and progress, restart and error states update in place. Its parts are
exposed through `panel_uia.rs` (heading, status/alert lines, invokable
buttons). Render it with `--scene update --update
available|single|closing|manual|downloading|restarting|error|checking|up-to-date
[--tool fallback] [--select notes-hidden] --screenshot PATH`, or without
`--screenshot` as a live notice by the notification area until Not now/Close
(its buttons reach no worker). It pops in with shipping's `ui-pop-in` and,
once restarting, fades up and out after 3 s (a failed restart puts it back);
under reduced motion both are a single step. A Windows 11 recheck at
8ce4447b found caret, placement, announcement and live-fixture defects, fixed
since and not yet rechecked; a real installed update is still untested.

On Linux (and in orbs), the tests also run under Wine 8 with a disposable prefix.
Run `node apps/native/prepare.mjs --output apps/native/windows/resources` first.
The prefix needs DejaVu copies renamed to Segoe UI/Consolas plus Wine's
Tahoma in `drive_c/windows/Fonts`, *registered* under
`HKLM\Software\Microsoft\Windows NT\CurrentVersion\Fonts` (Wine's DirectWrite
ignores unregistered files there, and every layout test then fails with
`0x80004005`), and a MinGW-built `bcryptprimitives.dll` exporting `ProcessPrng`
with `WINEDLLOVERRIDES=bcryptprimitives=n,b`. Build with
`cargo +1.95.0 test --target x86_64-pc-windows-gnu --no-run`, then run the test
executable under `wine` with `--test-threads=1`, an Xvfb `DISPLAY`, and
`CAPTURES_FFMPEG`/`CAPTURES_FFPROBE` pointing at Windows FFmpeg from a release
installer (`7z x`). In a 4 GB orb, run clippy with `-j1` and not alongside other
builds. These shims are test-only; never package them.

Each scene node (`src/scene.rs`) owns one composition surface. Hover and
toggle transitions animate visual opacity and offsets on the compositor, and
scrolling moves the card column's visual, so a frame redraws only nodes whose
content changed (in Preferences, on hover, at most one 32×19 switch track).
The recording shell retains each footer control separately; progress repaints
only the small progress/status surfaces and the caret blinks on the compositor.
