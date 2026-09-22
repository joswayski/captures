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

| Area | Implemented in this tree | Work still open |
| --- | --- | --- |
| Shared core | Settings/migrations, history/artifact lifecycle, capture coordination, recording engines/runtime, screenshot draft storage and document geometry/undo | Remaining editor actions and host bindings; installed-data migration/rollback |
| Capture and History | Region/window/display screenshots, countdown/cancel, seven configurable launch shortcuts, copy/save, counted media filters, clear all, original-recording export/reveal | Full input/coordinate/permission acceptance; large histories and editor reopen/restore |
| Recording workflow | Pause/resume/restart/mute/stop/discard, Hide/Show, passive region guide, screenshots during recording, ready/saved notices; both hosts provide frame scrubbing, graphical/numeric trim, numeric crop, preset/custom output size, track volume/mute/mono and MP4/GIF save-new-copy | Playback, thumbnail timeline/graphical crop, audio meter/device-change parity and physical recording/audio acceptance |
| Supporting UI | Appearance/preferences, resident tray/menu bar, retained preview stacks, explicit optional feedback | Onboarding, remaining Preferences parity, preview drag/fan/effects, single-instance/relaunch/login items, Open With, crash reporting |
| Editors | Shared draft storage, geometry/undo, image/annotation rendering, hit-testing and encoding; both hosts connect layers, canvas selection/move/rotation/resize, move/resize snapping, basic pan/zoom, canvas fill/transparency/trim, import, image transforms, annotation styles, Rectangle/Ellipse/Triangle/Diamond/Star/Line/Arrow/Pen/Wand/Erase/Restore, basic Text with bundled fonts, copy and save-new-copy | Broader text/font controls, live pixel brush feedback, remaining viewport/output controls and Tauri design parity; recording playback/graphical timeline and remaining controls |
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

The screenshot-editor stacks from
[#633](https://github.com/joswayski/captures/pull/633) (AppKit) and
[#634](https://github.com/joswayski/captures/pull/634) (wgpu and shared prerequisites)
are combined in this tree. Both hosts connect layers, import, lossless transforms,
output previews, save-new-copy and rectangle/ellipse drawing. AppKit now also
connects annotation styles ([#637](https://github.com/joswayski/captures/pull/637))
and Line/Arrow/Pen ([#638](https://github.com/joswayski/captures/pull/638)), matching
the existing wgpu command boundary. Both hosts connect crop gestures and clipboard
output. AppKit's Draw crop retains shared Rust `CropDrag` geometry through an
independent UI-thread C owner; no worker session, JSON or file access occurs during
pointer feedback. Free/preset ratios and Shift latching use the same shared rules
as wgpu. The preview and numeric fields track one candidate; Apply sends one crop
transaction, while Cancel/Escape, focus loss, leaving Geometry or closing restores
the pre-mode fields. Viewport changes cancel an active pointer gesture without
committing the candidate. Dragging does not dirty drafts or invalidate encoded output.
Physical AppKit pointer/mixed-DPI acceptance remains open.
The AppKit drawing slice passed 136 Swift tests in macOS CI;
its light/dark transient, committed, dot and minimum-size error fixtures were inspected.
These are development implementations, not completed platform acceptance gates.

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
canvas. Text resize still requires native font layout and is unsupported.
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
Clipboard-layer paste remains open, as does physical keyboard/IME/accessibility
acceptance on macOS, Windows, X11 and Wayland.
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
Both hosts connect canvas fill/transparency in Geometry. Apply background submits
one `set_background` worker transaction; the shared renderer validates hex colors
and composites beneath existing layers. Invalid colors preserve pixels, document
and undo/redo; hosts restore fields from the accepted state. Reset fields is not an
edit. A transparent canvas remembers the session's last accepted solid color for
switching back. Color changes participate in undo/redo and draft reopen; copy/export
use the newly rendered pixels. These are canvas fills, not image-background removal,
text backgrounds or the shipping color-picker layout. AppKit Geometry scrolls to keep
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
and serialized worker. Tolerance defaults to 36, matching Tauri, and accepts the
engine's full 0–255 range; contiguous removal defaults on. Pan and off-image clicks do not submit edits.
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
check exact pixels. Both native Draw panels now connect Erase/Restore with diameter
28 (4–120) and softness 18 (0–100). They sample press/movement/release into one
worker command, including stationary release stamps that affect soft-edge alpha.
The interim preview is a clipped path and brush-size ring, not live raster pixels;
release applies the stroke. Escape, focus loss, close, viewport or tool/section changes
cancel without editing. Pan and clipped/off-image initial presses never paint.
X11 tests cover cancellation, actual feathered alpha, erase/restore, undo/redo, drafts
and clipboard; AppKit has input/bridge tests and minimum light/dark/error fixtures.
Windows/Wayland presentation and physical AppKit input remain unverified; sampling
cadence, live pixel feedback and the Tauri brush cursor/layout remain parity work.
Both hosts connect Geometry → Trim edges through a shared `trim_canvas` command.
It fits visible layer geometry, including locked/zero-opacity and off-canvas layers,
rounds bounds outward, and translates every layer including hidden siblings. Empty,
hidden-only and already-tight documents are no-ops that preserve redo and pixels.
It includes rotated image/shape/path bounds and annotation shadows, not an alpha scan.
Changed trims are one render-before-publish undo step; draft and output use the new
dimensions. Shipping TypeScript vectors cover fractional/rotated/shadowed geometry;
host tests cover controls, undo/redo, output invalidation, drafts and clipboard.
Trim hover-margin feedback and the shipping toolbar layout remain unimplemented.
Windows/Wayland presentation and physical macOS input remain unverified.
Both hosts connect the shipping Compress presets: Tiny (55), Smaller (70), Balanced
(85), High (92) and Highest (98). Presets clear explicit PNG palette overrides and
reuse shared encoding: Tiny–High try 32/64/128/256 colors (retaining lossless pixels
when that is smaller), Highest keeps exact pixels with lossless packing.
JPEG/WebP retain their lossy quality mapping. Custom
numeric values/palettes remain available and labeled Custom when active. Preset
changes invalidate encoded previews without editing documents, drafts or undo.
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
Both hosts now expose confirmed **Replace original…** for the opened screenshot's
existing saved path and matching output format. The session pins that path; shared
Rust rechecks current History identity/path/type and file availability before encoding.
Sibling-temp publication replaces only that destination. Once-resized export, private
History PNG, thumbnail and dimensions update the same artifact ID/date, rather than
adding a copy. A post-publication History failure reports the saved path and warning.
Hosts dismiss that artifact's stale mini preview and reload History with fresh decode
generations after file publication, including partial success. Cancel/Escape and stale
confirmations submit no write; accepted writes drain on quit. Document, pixels, draft,
undo/redo and encoded preview are retained. Undo does not revert the saved file;
discard reloads the current History image. Unlike Tauri's full post-save flow, a new
copy is not adopted as this editor's source and drafts are not flattened/deleted.
Validation at write start is not a cross-process compare-and-swap or a two-store
transaction: an external change during encoding is not locked out. The file and
History publication are separate, and a History failure cannot roll back a saved file.
Physical macOS/Windows/Wayland and full output acceptance remain open.
The shared text prerequisite uses `cosmic-text` advanced shaping and CPU Swash
rasterization for a single line from caller-supplied fonts, with fixed locale and
no system-font scan. It returns logical advance, baseline, painted bounds and
straight-alpha pixels, including ligatures, combining marks, bidi ordering and
negative bearings. Glyph images are scoped to one operation, with line/size/pixel
limits and explicit missing-font/glyph errors. Original generated fonts give
independent metrics for tests rather than depending on installed fonts.
Color-outline and embedded-bitmap glyphs use different alpha representations;
the primitive normalizes outlines before compositing and retains bitmap RGB.
Swash's color-outline flattening has integer alpha-rounding loss. Font bytes must
come from a trusted source; output budgets are not a font-parser sandbox.
The single-line primitive also offers centered contour strokes with round joins,
preserving shaped advances, baseline, explicit faces and fractional glyph placement.
This uses scalable glyph paths, not bitmap dilation; colored and bitmap glyphs return
explicit outline errors. Filled and outlined masks cannot leak between operations
or stroke widths, and the existing raster bounds/pixel budgets still apply.
Both hosts now stage Outline with Text Apply/Cancel. Document rendering uses
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
in addition to individual line budgets. Missing fonts/glyphs, invalid styles and
budget failures return errors without changing the document or assets. The lower-level bitmap compositor
now supports a shadow/source pass using transformed pixel alpha, layer opacity,
canvas-space offsets, blur and blend mode. Shadow work is clipped to output plus
blur support; existing vector shadow/crisp passes are unchanged. Font-backed text
uses the shipping paragraph paint order: all glyph shadow/source passes precede
all crisp glyph passes. With a plate, only the plate receives a shadow. Both hosts
stage a Drop shadow toggle and custom color, opacity, blur and X/Y offsets with
Apply/Cancel. Both hosts use Rust-resolved defaults and submit only changed enabled
fields; saved precision and unknown style metadata survive toggling. Shadow-only
edits do not refit text. Failed transactions retain staged input, and disabled
shadow fields do not block Apply or closing. Legacy low-level `Shape::Text`
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
Missing families/glyphs and invalid requests preserve frames, redo and saved drafts;
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
2.1.5 static faces (4,359,164 bytes, shared across workers), with complete OFL 1.1 notices in
native resources, `--font-license` output and text-bearing saved drafts. Image-only
drafts do not persist the worker's unused font set. No OS fonts are copied
and no network fallback occurs. This Latin/Greek/Cyrillic-oriented default is not
universal Unicode or Tauri system-font equivalence; missing glyphs are errors.
Both hosts stage family changes with the other Text Apply/Cancel fields. Their
family picker reads the session's actual pinned map, not host defaults. Older
Sans-only drafts remain Sans-only; explicit font migration is still unimplemented.
Both selected-text inspectors offer a Style menu staged with Apply/Cancel.
Rust supplies the shipping seven-style catalog filtered by the session's pinned
font families: the bundle offers Standard, Outlined, Mono, Box and Mono box;
Sans-only drafts offer Standard, Outlined and Box. Rounded/Rounded box require an
actual pinned `rounded` face and are not substituted with Sans. Presets change only
family, plate/outline flags and (when no plate existed) the default plate color.
They preserve content, size, alignment, traits, text color, custom plate colors,
shadows and unknown metadata; the usual worker edit/refit rules still apply.
Shipping-TypeScript fixtures check the catalog; staged failure/cancellation and
font filtering are covered separately in host/session tests. This is not the
shipping style-picker layout, immediate editing or a new-text-default picker.
Reopening prefers the saved font set over host
defaults; missing/corrupt fonts return errors without silently substituting or
deleting the draft. Font cleanup follows successful manifest publication; the
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
per slice, not inferred from shared tests. Additional font import and OS acquisition,
inline input and physical input/IME/accessibility remain open.
Both hosts now offer new-text style, size (8–512) and color before placement.
Choices are per-editor UI state, not document/draft/undo; accepted responses and
failed creation retain them. New editors start at Standard when available and
annotation red. Shared Rust supplies Tauri's initial size: 5.5% of the original
capture's shorter side, rounded and clamped to 24–72. It uses History dimensions,
not the resized/cropped canvas of a restored draft; later user choices remain
unchanged across editing responses. Plain retains the explicit saved-family path for custom-font
drafts. Presets come only from pinned fonts; Rounded is not substituted. Shared
Rust validates the chosen preset and creates boxed text centered at the click using
the eight-em composing width, retaining the anchor when content later refits.
Placement is one render-before-publish transaction with fresh selection and normal
output invalidation; invalid/unavailable styles preserve pixels, redo and drafts.
The shipping Rounded-box default and inline composition remain different;
these controls do not reproduce the Tauri layout.
AppKit now starts an on-canvas native multiline responder when Text places a new
layer or hits an existing visible, unlocked text layer; double-clicking such a
layer from Select starts the same transaction. The responder retains local typing,
selection, clipboard and marked-text ownership while shared preview rendering is in
flight, coalescing replacements to the newest buffer. Return inserts a newline;
Done, Escape or focus loss commits one undo step, while Cancel restores the complete
pre-input document. Blank new input is discarded and blank existing input removes
the layer. Save, copy, import and unrelated document actions remain blocked until
the transaction resolves. Begin/update/finish failures keep retryable input, and
close/quit drain accepted work, preserving either the latest commit buffer or a
pending cancellation before draft handling. Shared pinned-font
layout and pixels remain authoritative: the AppKit composing field intentionally
uses the UI font and an axis-aligned clipped box, so exact family glyphs, text
effects, blending and rotated composing-field geometry remain parity work. Existing
inspector styling remains staged outside active composition. Automated macOS
fixtures cover light/dark normal, 760×540 and failure states, but physical macOS
IME, VoiceOver, keyboard layout and mixed-scale acceptance remain unverified.
No host text parity gate is closed.
The Windows/Linux candidate now connects a multiline on-canvas composing field
to the shared transient text transaction. New placement and existing Text-tool hits
retain a local typing buffer while one worker update fits/renders at a time.
Done/Escape/outside-click/close finish one undoable edit; Cancel restores the prior
document, selection and encoded output. Empty new text creates nothing; empty
existing text removes that layer. Quit drains the latest buffer before draft saving,
and failed updates retain it for retry/cancellation. Output actions cannot publish
unfinished pixels. The bounded composing field is unrotated and uses the UI font,
not exact document typography; shared pinned-font pixels remain authoritative.
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
while the accepted frame remains unchanged until the existing Apply/Seek actions.
Unapplied trim continues to gate seek, estimation and save. The wgpu track now
displays the shared 12-frame full-source thumbnail strip, center-cropped vertically
to the compact row. Excluded ranges are dimmed and grips retain the same hit regions.
Generation runs once on the serialized worker after open, with independent cancel
and retry; failure leaves editing available. Edits/seek retain the source strip and
never regenerate it or change the accepted preview. Close/quit waits for generation.
AppKit integration is a separate slice; playback remains unimplemented.
Raw-input tests exercise multi-pass delivery, keyboard focus,
thresholds, cancellation and busy gates; private-X11 tests cover staged values,
thumbnail loading/cancel/failure/retry and temporal pixels, exported duration/colors
and immutable source. Windows presentation and physical macOS/Windows/X11/Wayland
input/accessibility remain unverified; no parity gate closes.
Crop uses source-pixel coordinates. Numeric crop dimensions start
aspect-locked, follow the current crop ratio and fit the remaining source bounds;
unlocking permits independent dimensions, and relocking uses the adjusted ratio.
Typed dimensions commit on Enter/focus loss so partial input does not change the
ratio. The lock is an input preference, not an export edit. Custom output width/height
remain independent (no output aspect lock). Original, 1080p maximum and 720p maximum presets
reuse shared `MaxResolution::constrain`: cap height without upscaling, preserve
the current crop's aspect ratio and round to even pixels. Presets stay selected
after Apply/seek so later crop changes recompute the dimensions; Custom overrides
the preset and disabling Custom restores it. Apply edits stages these
values together with format/quality and previews the accepted export configuration.
Shared GIF encoding honors both explicit dimensions, including square-pixel aspect and proportional
size-budget retries, instead of silently ignoring output height. Re-encoded MP4 on
Windows/Linux fits within 3840 × 2160 (portrait: 2160 × 3840); format-aware preview
now reflects that cap without applying it to Preserve copy/remux or GIF paths.
The accepted format/quality and frame dimensions appear beside the preview.
Available system/microphone tracks have 0–200% volume, independent mute and mono
output controls. Availability comes from the accepted session's trusted audio
identity, not caller-provided track flags. Audio stages with geometry/format and
uses the same Apply/save/dirty guards; failed updates preserve staged controls
and accepted output state. The frame preview stays silent. GIF disables audio
controls while keeping settings for a later MP4 export. No-track recordings show
an explicit explanation rather than editable controls. Private X11 smoke uses
distinct stereo tones in a retained playback mix plus separate system/mic tracks,
then measures decoded export frequencies/amplitudes, mono channel count, mute,
GIF silence, restored MP4 settings and History audio identity.
Unapplied format/quality gates save and seek alongside geometric edits; failed
updates retain all accepted state and preserve staged values for correction.
Save uses the accepted configuration, and format/quality-only changes require
save or explicit discard. Size-budget previews are rejected rather than promising
the dimensions of a later encoding retry; this host does not expose size budgets.
**Estimate size** explicitly runs the shared Tauri estimator on the accepted
edit/export configuration through the same serialized worker. Copied bytes and
fully encoded short ranges report exact byte counts; longer sampled ranges and
audio-only Preserve changes are marked approximate. Staged edits hide the previous
result and gate estimation until Apply. A successful changed preview invalidates
the result; a seek retains it. Estimation has independent cancellation and error/retry,
creates no History entry, and never marks unsaved edits as saved. No estimate promises
a byte budget. Close/quit waits for accepted work, as with export.
The preview/timeline/save hierarchy follows the shipping recording editor, but
the UI is not a visual match and has no playback or thumbnail timeline yet.
One worker serializes media operations; failed seek/edit preserves the accepted
frame, and unapplied values gate scrubbing/export. Failed edits keep
the staged values available for correction. MP4/GIF Save new copy uses
shared encoding, reports progress and accepts independent cancellation. Existing
files and the original History artifact are never replaced. Post-publication
History failure reports the successfully saved path rather than inviting re-export.
Close blocks accepted work; unsaved edits require explicit discard, and normal quit
is refused until they are saved or closed. Recording drafts are not implemented.

Platform status: shared Rust/C ABI is connected to both hosts. The first AppKit
slice opens recordings from History in a separate native window with retained
decoded frames, source-relative seek, numeric trim, MP4/GIF format and quality,
size estimation, progress/cancel, collision-safe Save new copy and dirty close/quit
guards. AppKit's graphical trim handles use the shared allocation-free geometry and
only stage the existing numeric values; pointer movement never seeks or decodes, and
Apply/estimate/save gating is unchanged. AppKit now also stages independent volume
and mute for trusted system/microphone tracks plus mono output in that same atomic
Apply flow. GIF disables audio controls while retaining MP4 values, and the decoded
frame preview remains explicitly silent. AppKit also stages source-relative numeric
crop and Original/1080p/720p or independent custom output dimensions through the
same atomic Apply flow. Aspect-locked crop dimensions and resolution presets use the
shared allocation-free geometry; the lock remains UI-only, and Original omits explicit
output dimensions. Windows/X11 already implement the same controls through wgpu;
private X11/software-GL exercises provide implementation evidence only. Windows and
Wayland presentation, physical macOS input, accessibility, playback, thumbnails,
physical audio output, graphical crop handles, draft restoration and original
replacement remain open. No recording-editor or cross-platform parity gate closes.

All **19 end-to-end acceptance gates remain open**. The large remaining workstreams
are screenshot editing, recording editing, Tauri visual/interaction parity, OS/workflow
integration, physical cross-platform acceptance, and renderer/distribution/cutover.
This is not a near-release checklist or a percentage-complete claim: implemented
features still need acceptance, and the native editor still has a workbench layout.
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
The recording editor is a separate History action, so the trigger remains finalization, not the
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
are described below. Image layers expose a **Transform image** menu for lossless
left/right rotation and horizontal/vertical flips through the shared worker commands.
Hidden and locked images can transform, matching shipping policy; full-canvas
photos rotate their canvas, and undo/draft restore retain the orientation.
Merge/flatten remains unconnected.

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
Resize/curve grips and other tools remain separate work.
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

The wgpu Import image action now picks one PNG/JPEG/WebP/TIFF file independently
of the session worker. The worker bounds encoded input and decoded dimensions,
normalizes EXIF orientation and supplies owned RGBA to the shared import command.
RGB/grayscale ICC profiles convert to sRGB before publication, preserving straight
alpha; untagged files assume sRGB. Unsupported or malformed ICC profiles, CMYK
profiles, and PNG gamma/chromaticity-only or CICP descriptions fail recoverably
instead of silently relabeling samples. Those color formats and HDR/wide-gamut
editing remain open; imports normalize to RGBA8. Analytic linear-to-sRGB fixtures
exercise profile transport through PNG, JPEG, WebP and TIFF plus grayscale alpha.
The returned stable ID selects the new layer. Cancellation, decode failures and
late results after close preserve the editor; a completed selection waits for
already accepted edits before importing. Imports do not write a draft or History
until explicitly saved, and saved assets survive deleting the external source.
Private-X11 checks exercise the actual rfd D-Bus transport with a disposable file
chooser fixture, asymmetric rendered pixels, cancellation/retry, undo/redo,
reopen and stale-close handling in both appearances. That fixture does not verify
physical file dialogs, input, accessibility or IME. Windows and Wayland share the
implementation but remain presentation-unverified; AppKit import is described below.
Batch import and drag-and-drop remain separate slices. Shipping Tauri import is unchanged.

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
The wgpu host also connects an output-folder picker and edited-image clipboard
output. AppKit export and clipboard controls are described below;
post-save source adoption and physical-platform acceptance remain open.

The AppKit editor host now enables **Edit screenshot** only for screenshot History
entries. Its dedicated serialized worker owns the shared Rust session and publishes
independently retained RGBA frames to a fit preview. The window exposes crop geometry,
canvas sizing, Undo/Redo, explicit draft save and confirmed draft discard; shared Rust
remains the only geometry/render authority. Geometry and Layers views retain the fit
preview; the front-to-back layer panel exposes visibility, lock, opacity, absolute
X/Y movement through shared deltas, image rename, duplicate, delete and adjacent
ordering. Stable IDs preserve selection across replies, and shared Rust remains the
authority for locked barriers and duplicate behavior. An Output view runs shared
PNG/JPEG/WebP encoding on that worker, reports exact bytes and switches the fit preview
between the edited canvas and decoded output. Option or document changes invalidate
stale output; previewing has no draft, undo, clipboard or file side effects. **Save new
copy** chooses a directory independently of the worker, then serializes publication on
that worker. It never replaces a file or mutates the draft; successful publication adds
a distinct History entry, and partial History failure preserves the saved path.
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
color/width, and shadow color/opacity/blur/offset controls. Apply style submits one
minimal shared patch through the existing serialized worker and invalidates encoded
output; Reset fields, selection changes and worker failures restore published values.
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
Copy image and Save new copy are pinned below the scrolling inspector on both
hosts, including at 760×540. They remain available in Geometry, Layers and Draw;
Output retains format/quality/size, destination, encoding preview and confirmed
original replacement. Copy still uses full-resolution edited PNG pixels, while
Save uses the selected export options without saving the draft or replacing a file.
Both actions retain the serialized worker and accepted-work lifecycle. AppKit
keeps its full status area above the actions; wgpu shows a compact status with the
complete message on hover. Native fixtures exercise section/resize visibility,
pending-work gates and unchanged canvas geometry; X11 export tests save from all
four sections. Windows/Wayland presentation and physical AppKit acceptance remain
open, rather than being inferred from shared code or rendered CI fixtures.
The tool rail and toolbar/export-bar organization still differ from Tauri.
Remaining viewport controls and other drawing tools are not connected.
Recording editing remains open on both hosts; the
screenshot-editor parity gate stays open.

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
