#ifndef CAPTURES_SETTINGS_H
#define CAPTURES_SETTINGS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Worker-only feedback. context returns {app_version, os, os_version, arch}
 * without network access. submit {draft: {message, contact: string|null, category}}
 * sends only those fields and the displayed context, after explicit user consent.
 * No capture, log, or diagnostic attachments. Success/error envelopes and owned
 * UTF-8 pointer rules match captures_app_request_v1; free with
 * captures_settings_free_v1. A process-wide client enforces submission cooldown.
 * HTTP has a 20-second timeout; callers may also wait for serialized submissions.
 * Never call on the UI thread or serialize behind capture/recording work. */
char *captures_feedback_request_v1(const char *request_json);

/* Pure, allocation-returning update notice helpers; safe on the UI thread.
 * No network, download or install happens here (no signed updater yet).
 * present {status: UpdateStatus|null, view?: {show_changelog, action_error,
 * installing}} returns the shared copy/presentation both hosts render.
 * fixture {name} returns {status} for workbench fixtures; stub {status, event:
 * "install"|"check"|"tick"} returns {status: UpdateStatus|null, tick_ms} from
 * the deterministic simulated source (null status closes the notice).
 * placement {monitor, work_area, tray|null, menu_bar_at_top, card_width,
 * card_height} takes top-left logical rectangles {x, y, width, height} and
 * returns {placement: {x, y, width, height, caret: "none"|"top"|"bottom",
 * caret_x}, card: rect relative to the window}. Envelopes follow
 * captures_app_request_v1; free with captures_settings_free_v1. */
char *captures_update_notice_request_v1(const char *request_json);

/* Event-loop-thread-only native capture-launch shortcuts. One owner per process.
 * JSON requests: configure {settings: AppSettings}, enabled {enabled: bool},
 * next, close. Envelopes follow captures_app_request_v1. next returns
 * {action: "new_capture"|"region"|"window"|"display"|"record_region"|
 * "record_window"|"record_display"|null}; consumes one launch.
 * Configure copies settings; conflicts retain the prior registered mapping.
 * wake is required on first configure, must remain callable for process lifetime,
 * may run on an OS worker thread, and must ONLY schedule host work (no synchronous
 * reentry). Drain next on the native thread after waking. No timer is required.
 * Disabling/reconfiguring/closing discards queued and held-key launch intent.
 * Close before app teardown. Do not configure synthetic fixture scenes.
 * Pure recorder requests also work without a configured owner or wake callback:
 * record {event: {code, ctrlKey, shiftKey, altKey, metaKey}, platform} returns
 * {kind: "cancel"|"waiting"|"invalid"|"complete", keys?, message?, shortcut?}.
 * display {shortcut, platform} returns {keys: string[]}. Platform is
 * "macos"|"windows"|"linux". Neither operation registers or saves anything.
 * request_json is readable NUL-terminated UTF-8 during the call. Free returned
 * owned JSON with captures_settings_free_v1 exactly once. */
typedef void (*CapturesShortcutWake)(void);
char *captures_shortcuts_request_v1(const char *request_json, CapturesShortcutWake wake);

/* Native-thread-only, live-mode single-instance owner. Call start before UI or
 * capture initialization: {operation:"start", history_root:string|null, paths:[]}
 * returns {primary:bool}; false means the running owner's queue acknowledged the
 * request and this process must exit without UI. Canonical History root defines
 * identity, independent of settings. Paths resolve against this sender's CWD.
 * next returns {request:null|{paths:[absolute paths]}}; an empty paths array means
 * relaunch/focus. stop joins the worker but retains election while host work
 * drains; close releases election. Do neither when quit is cancelled.
 * Startup paths stay with the primary's existing host queue.
 * wake must remain callable for process lifetime and ONLY schedule host-thread
 * work. Drain once after startup too; a wake may precede UI initialization.
 * Ack is queue acceptance, not successful decoding or durable delivery on quit.
 * No retries after sending if acknowledgement fails (outcome may be unknown).
 * request_json and returned JSON ownership follow the shortcut ABI above. */
char *captures_instance_request_v1(const char *request_json, CapturesShortcutWake wake);

/* Allocation-free region geometry in display-local logical coordinates. These
 * field layouts are versioned alongside the function names. No pointers are
 * retained. False leaves output unchanged (including null output, invalid mode,
 * non-finite values, invalid bounds/initial rectangle or negative aspect).
 * A non-null output must point to writable, aligned storage for one rectangle.
 * Aspect 0 means freeform; Shift forces square. Resize minimum is 16 units.
 * Keep the original drag origin/initial rectangle when modifiers change. */
typedef struct { double x, y; } CapturesSelectionPoint;
typedef struct { double width, height; } CapturesSelectionBounds;
typedef struct { double x, y, width, height; } CapturesSelectionRect;
/* mode: 0=create, 1=move, 2=NW, 3=NE, 4=SW, 5=SE. */
bool captures_selection_drag_v1(uint32_t mode, CapturesSelectionPoint origin,
    CapturesSelectionPoint current, CapturesSelectionRect initial,
    CapturesSelectionBounds bounds, double aspect, bool force_square,
    CapturesSelectionRect *output);
bool captures_selection_constrain_v1(CapturesSelectionRect rect,
    CapturesSelectionBounds bounds, double aspect, CapturesSelectionRect *output);

/* UI-thread-only screenshot crop geometry, independent of editor workers.
 * Coordinates are top-left canvas pixels; bounds must be at least 1x1.
 * Aspect 0 is freeform; positive is a preset. Shift latches the live crop ratio
 * (square when starting with Shift), unlike the capture selector's force-square.
 * begin returns NULL on invalid inputs. Updates allocate nothing and reject
 * nonfinite/negative-aspect/null input without changing the owner or output.
 * Output must be writable aligned storage disjoint from the live drag.
 * Do not call concurrently; free each owner once after its last update. */
typedef struct CapturesEditorCropDrag CapturesEditorCropDrag;
CapturesEditorCropDrag *captures_editor_crop_begin_v1(CapturesSelectionPoint origin,
    CapturesSelectionBounds canvas, double aspect, bool shift);
bool captures_editor_crop_update_v1(CapturesEditorCropDrag *drag,
    CapturesSelectionPoint current, double aspect, bool shift, CapturesSelectionRect *output);
void captures_editor_crop_free_v1(CapturesEditorCropDrag *drag);

/* Ephemeral screenshot viewport, never persisted in a document or draft.
 * All coordinates are top-left logical points. fit is the host layout's fitted
 * image rect; canvas is image-pixel size. zoom_percent=0 means Fit; pan remains
 * applicable in Fit. Set all fields to zero for Fit+recenter, or just pan_x/y for
 * recenter at the current zoom. Zoom anchors a document point, clamps to 5–800%
 * and rounds to 0.1%. Inputs are copied; false leaves output untouched. Output
 * must be null or writable aligned storage. No allocation, I/O or worker access. */
typedef struct { double zoom_percent, pan_x, pan_y; } CapturesEditorViewport;
bool captures_editor_viewport_rect_v1(CapturesEditorViewport viewport,
    CapturesSelectionRect fit, CapturesSelectionBounds canvas, CapturesSelectionRect *output);
bool captures_editor_viewport_zoom_v1(CapturesEditorViewport viewport,
    CapturesSelectionRect fit, CapturesSelectionBounds canvas, double percent,
    CapturesSelectionPoint anchor, CapturesEditorViewport *output);
/* Native hosts normalize wheel delta to pixels; zero result means invalid input. */
double captures_editor_viewport_wheel_factor_v1(double delta_pixels);
/* Shared logarithmic slider: 0–1 position ↔ 5–800 percent. Finite inputs clamp;
 * percent rounds to 0.1%. Fit supplies actual displayed percent, not zero.
 * Non-finite input returns NaN. No allocation or worker/session access. */
double captures_editor_viewport_slider_position_v1(double percent);
double captures_editor_viewport_slider_zoom_v1(double position);

/* Shared preview placement. Monitor bounds are PHYSICAL pixels in desktop
 * top-left coordinates (negative origins allowed), including the actual usable
 * work area. Output/origin are LOGICAL coordinates in that same orientation.
 * AppKit must convert its bottom-left points at this boundary. Scale must be
 * finite and positive; shipping policy clamps values below 1 to 1.
 * Placement: 0 bottom-left, 1 bottom-right, 2 top-left, 3 top-right.
 * Anchor: 0 bottom, 1 top. NULL origin uses the configured corner.
 * No allocation or OS access. Borrow pointers only for this call. False leaves
 * output unchanged (null output, bad enum/nonfinite origin/scale/empty bounds).
 * Non-null origin/output require aligned readable/writable storage. */
typedef struct {
    int32_t work_x, work_y;
    uint32_t work_width, work_height;
    int32_t full_x, full_y;
    uint32_t full_width, full_height;
    double scale_factor;
} CapturesPreviewMonitor;
typedef struct { double x, edge; uint32_t anchor; } CapturesPreviewOrigin;
typedef struct {
    double x, y, width, height, card_height, padding, control_gutter;
    uint32_t anchor;
} CapturesPreviewGeometry;
bool captures_preview_geometry_v1(CapturesPreviewMonitor monitor, size_t count,
    bool collapsed, const CapturesPreviewOrigin *origin, uint32_t placement,
    CapturesPreviewGeometry *output);

/* Owned shared visibility state, not a native window. Serialize all calls on
 * one handle (normally the UI thread). Free exactly once after callers stop;
 * NULL is permitted by free and returns false from every other handle call.
 * Generation tokens below are preview tokens, NOT capture-flow generations.
 * Host must apply native visibility and wait for it to settle before capture;
 * this policy cannot hide a window itself. Include-in-captures intentionally
 * overrides suppression, but never zero count or disabled previews. */
typedef struct CapturesPreviewVisibility CapturesPreviewVisibility;
CapturesPreviewVisibility *captures_preview_visibility_new_v1(void);
void captures_preview_visibility_free_v1(CapturesPreviewVisibility *handle);
/* False leaves output untouched; NULL output refuses begin without mutation. */
bool captures_preview_begin_v1(CapturesPreviewVisibility *handle, uint64_t *output);
/* artifact strings are readable NUL-terminated UTF-8; NULL/invalid UTF-8 fails.
 * Wait copies the ID. Ready only accepts the current pending ID. */
bool captures_preview_wait_v1(CapturesPreviewVisibility *handle, uint64_t generation,
    const char *artifact);
bool captures_preview_ready_v1(CapturesPreviewVisibility *handle, const char *artifact);
bool captures_preview_restore_v1(CapturesPreviewVisibility *handle, uint64_t generation);
/* Clears a pending decode wait, never an active capture without a pending ID. */
bool captures_preview_stop_waiting_v1(CapturesPreviewVisibility *handle);
/* UI suppression is independent of the capture generation/decoded artifact. */
bool captures_preview_capture_ui_v1(CapturesPreviewVisibility *handle, bool suppressed);
bool captures_preview_visible_v1(const CapturesPreviewVisibility *handle, size_t count,
    bool enabled, bool include_in_captures);

/* Session-only chronological membership, initially expanded; no history/file
 * operations. Serialize all calls and free exactly once. Null handles return
 * false/zero. Insert copies nonempty UTF-8 IDs; duplicates do not reorder.
 * Clear all by removing a SNAPSHOT of IDs, never by draining a changing list.
 * An empty stack resets collapsed state; new captures preserve nonempty state. */
typedef struct CapturesPreviewStack CapturesPreviewStack;
typedef struct { double y; size_t depth; bool interactive; } CapturesPreviewCardLayout;
typedef struct { const uint8_t *data; size_t length; } CapturesPreviewID;
CapturesPreviewStack *captures_preview_stack_new_v1(void);
void captures_preview_stack_free_v1(CapturesPreviewStack *handle);
bool captures_preview_stack_insert_v1(CapturesPreviewStack *handle, const char *id);
bool captures_preview_stack_remove_v1(CapturesPreviewStack *handle, const char *id);
size_t captures_preview_stack_count_v1(const CapturesPreviewStack *handle);
/* Unclamped logical document height, including the control gutter; zero empty. */
double captures_preview_stack_height_v1(const CapturesPreviewStack *handle);
/* Borrowed UTF-8 bytes, not NUL terminated; copy before next mutation/free.
 * Outputs require aligned writable storage. Null/invalid index leaves outputs
 * unchanged. Layout index is chronological. y is in logical unscrolled content;
 * top_anchor mirrors expanded order and compact peeks. Paint oldest first so
 * newest (depth zero) is on top. Scroll expanded overflow instead of truncating. */
bool captures_preview_stack_id_v1(const CapturesPreviewStack *handle, size_t index,
    CapturesPreviewID *output);
bool captures_preview_stack_set_collapsed_v1(CapturesPreviewStack *handle, bool collapsed);
bool captures_preview_stack_collapsed_v1(const CapturesPreviewStack *handle);
bool captures_preview_stack_card_v1(const CapturesPreviewStack *handle, size_t index,
    bool top_anchor, CapturesPreviewCardLayout *output);
/* Hover fan pose; v1 remains the rest pose. Front-card position and interaction
 * are unchanged. Expanded stacks ignore hovered. */
bool captures_preview_stack_card_v2(const CapturesPreviewStack *handle, size_t index,
    bool top_anchor, bool hovered, CapturesPreviewCardLayout *output);

/* Pure compact-card shade policy. Paint glass-strong-solid at this opacity.
 * Depth zero is undimmed. Expanded cards never use this overlay. */
double captures_preview_dim_opacity_v1(size_t depth);

/* Owned immutable region session. Prepare/capture may block; use a worker after
 * hiding capture windows. Begin/retain a capture-flow guard on the event-loop
 * thread first. Freeze and cursor settings are fixed at prepare. No pixel data
 * crosses JSON or temporary files. No permissions prompt occurs implicitly.
 * Prepare returns NULL on failure; non-null output must be writable char-pointer
 * storage and receives owned {ok,result:{display}} or {ok,error} JSON. It does
 * not free a previous output value. NULL output refuses preparation entirely.
 * Free all JSON results with captures_settings_free_v1. */
typedef struct CapturesRegionSession CapturesRegionSession;
CapturesRegionSession *captures_region_prepare_v1(const char *display_id,
    uint64_t generation, bool freeze, bool include_cursor, char **output);
/* Borrowed straight-alpha RGBA8/sRGB, top-to-bottom, tight rows. Keep the session
 * alive for every image-provider/worker borrow, including asynchronous draws.
 * Never mutate/free data. False (nulls or live-desktop mode) leaves output intact.
 * Non-null output must point to writable, aligned CapturesRegionPixels storage. */
typedef struct {
    const uint8_t *data;
    size_t length;
    uint32_t width, height;
    size_t bytes_per_row;
} CapturesRegionPixels;
bool captures_region_pixels_v1(const CapturesRegionSession *session, CapturesRegionPixels *output);
/* Capture after the selector/countdown has closed and settled. A countdown uses
 * fresh pixels/cursor, even when the selection used frozen pixels. Rect is in
 * display-local logical coordinates. Returns app-request {ok,result}/{ok,error}.
 * The session and UTF-8/NUL-terminated root must remain valid through this call. */
char *captures_region_capture_v1(const CapturesRegionSession *session,
    const char *root, CapturesSelectionRect rect, bool after_countdown);
/* Exactly once after all workers/providers stop borrowing; NULL is permitted.
 * Separately finish the main-thread flow guard on success, failure and quit. */
void captures_region_free_v1(CapturesRegionSession *session);

/* Worker-owned screenshot editor. Open JSON: {history_root,drafts_root,artifact_id}.
 * Use isolated native roots, not installed-app storage. Serialize open/request/
 * frame/free calls on one worker; decode, render and draft I/O may block.
 * NULL output refuses open; otherwise output receives owned {ok,result}/{ok,error}
 * JSON, freed with captures_settings_free_v1. Failed open returns NULL.
 * Requests: snapshot, crop {rect:{x,y,width,height}}, resize_canvas {width,height},
 * create_closed_shape/create_open_shape {shape,start:{x,y},end:{x,y}},
 * create_freehand_path {points:[{x,y},...]}, layer {id,edit},
 * commit {document}, undo, redo, save_draft {updated_at_ms}, discard_draft.
 * Snapshots contain artifact_id, document, can_undo, can_redo, unsaved_changes,
 * has_draft. Only owned image sources may be committed; unsupported visible
 * annotation layers return an error without changing document/history/frame.
 * No pixels/base64 in JSON; no implicit draft write when freeing the session.
 * Input strings remain readable UTF-8/NUL-terminated during each call. */
typedef struct CapturesEditorSession CapturesEditorSession;
typedef struct CapturesEditorFrame CapturesEditorFrame;
CapturesEditorSession *captures_editor_open_v1(const char *request_json, char **output);
char *captures_editor_request_v1(CapturesEditorSession *session, const char *request_json);
/* Stateless picking from a published document JSON copy, not a session handle.
 * Call once on pointer press, never per movement/frame. No render/decode/I/O.
 * Coordinates and nonnegative tolerance are finite document pixels.
 * Returns owned {ok:true,result:{hit:string|null}} or {ok:false,error:string};
 * free with captures_settings_free_v1. Null/malformed input is an error.
 * Visible/unlocked unsupported geometry is an error, not silent fall-through.
 * Snapshots also provide selection_outlines: {layerId:[{x,y},...]} with four
 * document-space corners; unsupported layers omit their outline. */
char *captures_editor_hit_test_document_v1(const char *document_json,
    double x, double y, double tolerance);
/* New-annotation shadow defaults resolved by Rust for finite stroke width 2–40.
 * No session/render/I/O. Returns owned success/error JSON; free with
 * captures_settings_free_v1. */
char *captures_editor_default_shadow_v1(double stroke_width);
/* Allocation-free rotation chrome/preview. Outline is four original published
 * world-space corners in local TL,TR,BR,BL order; angles are radians. All pointer
 * inputs are aligned/readable for four points, outputs writable for one struct.
 * No pointers retained, JSON, sessions, rendering or I/O. False leaves output
 * unchanged for null/nonfinite/invalid input or a grip that cannot fit the canvas.
 * Hosts hide handles on hidden/locked layers. Hit radius is in document pixels.
 * Keep original outline, angle and press point when modifiers change. Shift
 * snaps to the shipping default 15-degree stops, including negative half-ties. */
typedef struct {
    CapturesSelectionPoint anchor, handle;
    double hit_radius;
} CapturesEditorRotationHandle;
typedef struct {
    double radians;
    CapturesSelectionPoint outline[4];
} CapturesEditorRotationPreview;
bool captures_editor_rotation_handle_v1(const CapturesSelectionPoint *outline,
    double radians, double display_scale, CapturesSelectionBounds canvas,
    CapturesEditorRotationHandle *output);
bool captures_editor_rotation_preview_v1(const CapturesSelectionPoint *outline,
    double initial, CapturesSelectionPoint start, CapturesSelectionPoint current,
    bool snap, CapturesEditorRotationPreview *output);
/* Same ownership as v1. When snap is true, finite snap_degrees clamp to [1,180];
 * nonfinite values use 15. v1 retains its fixed 15-degree default. */
bool captures_editor_rotation_preview_v2(const CapturesSelectionPoint *outline,
    double initial, CapturesSelectionPoint start, CapturesSelectionPoint current,
    bool snap, double snap_degrees, CapturesEditorRotationPreview *output);
/* Independent immutable resize gesture. Begin parses the published document
 * once and hit-tests the selected layer at 8 view points of tolerance. Output
 * is set to NULL on miss/error; success transfers a drag with copied geometry
 * and snap lines. Returns owned JSON {ok:true,result:{handle:0..7|null}} or
 * {ok:false,error:string}, freed with captures_settings_free_v1. Handle order:
 * NW,N,NE,E,SE,S,SW,W. Strings and output pointer storage are borrowed for begin.
 * Preview has no JSON/session/render/I/O; false leaves output untouched. Guides
 * are at most four, orientation 0 vertical / 1 horizontal, in document pixels.
 * Keep original drag through Shift changes; release/free it on cancellation,
 * window resize, snapshot replacement or completion, after all calls finish. */
typedef struct CapturesEditorResizeDrag CapturesEditorResizeDrag;
typedef struct {
    uint32_t orientation;
    double position;
} CapturesEditorAlignmentGuide;
typedef struct {
    CapturesSelectionPoint outline[4];
    CapturesEditorAlignmentGuide guides[4];
    size_t guide_count;
} CapturesEditorResizePreview;
char *captures_editor_resize_begin_v1(const char *document_json, const char *layer_id,
    CapturesSelectionPoint point, double display_scale, CapturesEditorResizeDrag **output);
bool captures_editor_resize_preview_v1(const CapturesEditorResizeDrag *drag,
    CapturesSelectionPoint current, bool lock_aspect, CapturesEditorResizePreview *output);
void captures_editor_resize_free_v1(CapturesEditorResizeDrag *drag);
/* Independent immutable drag-move, after body picking. Begin copies geometry
 * and snap lines; rejects hidden/locked/unsupported layers. Returns owned usual
 * {ok:true,result:{}} or {ok:false,error:string} JSON; output is NULL on error.
 * Preview reuses the resize outline/guide descriptor. Delta is document-space
 * displacement from the original press, not from the previous preview. There is
 * no per-event JSON/session/render/I/O. False leaves output untouched. Strings
 * are borrowed for begin; outputs are writable. Free response with settings_free,
 * and drag exactly once after all preview calls/cancellation/completion. */
typedef struct CapturesEditorMoveDrag CapturesEditorMoveDrag;
char *captures_editor_move_begin_v1(const char *document_json, const char *layer_id,
    double display_scale, CapturesEditorMoveDrag **output);
bool captures_editor_move_preview_v1(const CapturesEditorMoveDrag *drag,
    CapturesSelectionPoint delta, CapturesEditorResizePreview *output);
void captures_editor_move_free_v1(CapturesEditorMoveDrag *drag);
/* Stateless shared preview geometry; no session access or per-event JSON.
 * kind 0: arrow outline, exactly two signed document-space endpoints.
 * kind 1: smoothed Pen centerline, one or more accepted samples; one is a dot,
 * two also represent a straight Line. Hosts paint round caps/joins.
 * kinds 2/3/4: Triangle/Diamond/Star, exactly two signed document-space endpoints;
 * returns normalized vertices (close the path to fill), including degenerate previews.
 * Input is aligned/readable for length initialized points during the call.
 * Non-null output points to writable aligned descriptor storage. Success owns
 * an independent immutable buffer and returns the default shared stroke width.
 * Too-short arrows succeed with zero points. Invalid inputs/panic return NULL
 * and leave output unchanged. Borrow output.data only while the handle lives;
 * release exactly once after all borrows. NULL free is allowed. */
typedef struct CapturesEditorDrawGeometry CapturesEditorDrawGeometry;
typedef struct {
    const CapturesSelectionPoint *data;
    size_t length;
    double stroke_width;
} CapturesEditorDrawPoints;
CapturesEditorDrawGeometry *captures_editor_draw_geometry_v1(uint32_t kind,
    const CapturesSelectionPoint *input, size_t length, CapturesEditorDrawPoints *output);
/* Same ownership/validation contract; stroke_width must be finite and positive.
 * Shapes and the returned width use this explicit value instead of the v1 default. */
CapturesEditorDrawGeometry *captures_editor_draw_geometry_v2(uint32_t kind,
    const CapturesSelectionPoint *input, size_t length, double stroke_width,
    CapturesEditorDrawPoints *output);
void captures_editor_draw_geometry_free_v1(CapturesEditorDrawGeometry *handle);
/* Import one host-decoded image on the serialized session worker. request_json is
 * {name,selected_id?,point?:{x,y}} and never contains pixels or asset URLs.
 * pixels describes borrowed top-down straight-alpha sRGB RGBA8; padded rows are
 * accepted. The descriptor, JSON and actual RGBA bytes in each row must remain
 * readable, initialized and live for the call; padding need not be initialized and
 * is never read. The function validates dimensions, stride, length and pointer
 * arithmetic before reading/copying, then owns an independent image.
 * Success returns owned {ok:true,result:{layer_id,snapshot}}; failure returns
 * {ok:false,error} without changing document/frame/history/assets or writing files.
 * Free the response with captures_settings_free_v1. Never access/free the session
 * concurrently. Hosts retain decoding, picker, clipboard and batch policy. */
char *captures_editor_import_image_v1(CapturesEditorSession *session,
    const CapturesRegionPixels *pixels, const char *request_json);
void captures_editor_free_v1(CapturesEditorSession *session);
/* Retain on the worker without copying pixels. The frame may move to the UI and
 * outlive subsequent edits or session free. Release exactly once after all image
 * providers/draws stop reading. NULL session returns NULL; NULL free is allowed.
 * pixels returns borrowed straight-alpha sRGB RGBA8 in tight top-down rows; false
 * leaves output unchanged. Never mutate/free data. Retain frame for every read. */
CapturesEditorFrame *captures_editor_frame_v1(const CapturesEditorSession *session);
bool captures_editor_frame_pixels_v1(const CapturesEditorFrame *frame, CapturesRegionPixels *output);
void captures_editor_frame_free_v1(CapturesEditorFrame *frame);

/* Worker-only uncommitted drawing render. Accepts create_closed_shape,
 * create_open_shape, create_freehand_path or paint_image_background requests.
 * Brush requests contain the complete gesture, rendered from published assets.
 * Never changes document,
 * undo/redo, assets, published frame or drafts; no I/O. Retains independent
 * pixels freed with frame_free_v1. Session access must remain serialized.
 * NULL output refuses the operation; otherwise writes owned success/error JSON
 * freed with captures_settings_free_v1. Failure returns NULL. */
CapturesEditorFrame *captures_editor_preview_drawing_v1(CapturesEditorSession *session,
    const char *request_json, char **output);

/* Encode the current edited frame on the serialized session worker. No file or
 * clipboard I/O, no draft save, and no change to document/undo/redo/dirty state.
 * Options JSON: {format:"png"|"jpeg"|"webp", quality:"preserve"|"compress"|"maximum",
 * quality_value:0..255, max_size_bytes:unsigned|null, png:{max_colors:unsigned|null}}.
 * max_size_bytes and max_colors may be omitted. Other fields are required.
 * Uses the shared shipping format/quality/clamping and hard-byte-budget policy.
 * NULL output refuses encoding; otherwise it receives owned {ok,result:{length}}
 * or {ok:false,error} JSON, freed with captures_settings_free_v1. Failure returns
 * NULL. Inputs remain live/readable for the call; never call/free the session
 * concurrently. Hosts choose destinations, publish files, and update clipboard.
 * The export owns immutable encoded bytes independently of edits/session close;
 * it can move to another thread. No pixels or encoded bytes are placed in JSON. */
typedef struct CapturesEditorExport CapturesEditorExport;
typedef struct {
    const uint8_t *data;
    size_t length;
} CapturesEditorBytes;
CapturesEditorExport *captures_editor_encode_v1(const CapturesEditorSession *session,
    const char *options_json, char **output);
/* Borrow while export is live; false for NULL input/output leaves output unchanged.
 * Never mutate/free data. Release export exactly once after all borrows end. */
bool captures_editor_export_bytes_v1(const CapturesEditorExport *exported, CapturesEditorBytes *output);
void captures_editor_export_free_v1(CapturesEditorExport *exported); /* NULL allowed */

/* Save NEW COPY on the serialized session worker. Never replaces an existing file
 * or changes session/document/undo/redo/draft state. Hosts choose the destination
 * and pass their isolated History root and capture mode; drain accepted writes
 * before closing the worker. JSON request: {history_root,destination,options,mode}.
 * options uses the encode schema above; mode is "region", "window" or "display".
 * Owned response: {ok:true,result:{status:"saved",path,artifact}} or
 * {ok:true,result:{status:"saved_without_history",path,warning}} after file success
 * but History failure. Present that path for recovery, not as a total failure.
 * {ok:false,error} means no export was published. Free every response with
 * captures_settings_free_v1. Inputs remain live/readable UTF-8 during the call;
 * never access/free session concurrently. NULL session/input returns an error. */
char *captures_editor_save_new_v1(const CapturesEditorSession *session, const char *request_json);

/* OVERWRITE ORIGINAL on the serialized session worker. Only the saved_path
 * captured when this screenshot session opened may be replaced. The shared
 * implementation revalidates History and the source file before publication.
 * JSON request: {destination,options}; result schema and ownership match
 * captures_editor_save_new_v1. Does not mutate document/undo/redo/draft state.
 * NULL session/input returns an owned error response. */
char *captures_editor_save_original_v1(const CapturesEditorSession *session,
    const char *request_json);

/* Allocation-free recording crop/output-size geometry. Crop resize preserves
 * the crop's current aspect ratio and fits at its existing origin; staged width
 * or height may exceed the remaining source and is repaired. Axis 0 changes
 * width and axis 1 changes height. Crop/source extents must leave at least 2px.
 * Max-resolution presets 0/1/2 are Original/1080p/720p and call the shared
 * recording model directly for positive source dimensions, including 1px;
 * Original still normalizes dimensions to an even minimum of 2px.
 * Crop drag handles 0..8 are move/N/NE/E/SE/S/SW/W/NW. Drag deltas use source
 * pixels from the immutable pointer-down rectangle. Unlocked handles move their
 * edges; locked corners anchor opposite edges and locked edges center the coupled
 * dimension while fitting the current ratio in bounds. Final integer geometry
 * is rounded and re-bounded with a 2px minimum.
 * False means null/nonfinite/invalid input and leaves output untouched.
 * Inputs/outputs are copied; functions allocate nothing and access no session
 * or media I/O. */
#define CAPTURES_RECORDING_CROP_RESIZE_WIDTH 0
#define CAPTURES_RECORDING_CROP_RESIZE_HEIGHT 1
#define CAPTURES_RECORDING_CROP_HANDLE_MOVE 0
#define CAPTURES_RECORDING_CROP_HANDLE_N 1
#define CAPTURES_RECORDING_CROP_HANDLE_NE 2
#define CAPTURES_RECORDING_CROP_HANDLE_E 3
#define CAPTURES_RECORDING_CROP_HANDLE_SE 4
#define CAPTURES_RECORDING_CROP_HANDLE_S 5
#define CAPTURES_RECORDING_CROP_HANDLE_SW 6
#define CAPTURES_RECORDING_CROP_HANDLE_W 7
#define CAPTURES_RECORDING_CROP_HANDLE_NW 8
#define CAPTURES_RECORDING_MAX_RESOLUTION_ORIGINAL 0
#define CAPTURES_RECORDING_MAX_RESOLUTION_1080P 1
#define CAPTURES_RECORDING_MAX_RESOLUTION_720P 2
typedef struct {
    uint32_t x, y, width, height;
} CapturesRecordingCropRect;
typedef struct {
    uint32_t width, height;
} CapturesRecordingDimensions;
bool captures_recording_crop_resize_locked_v1(CapturesRecordingCropRect crop,
    CapturesRecordingDimensions source, uint8_t axis, uint32_t value,
    CapturesRecordingCropRect *output);
bool captures_recording_crop_after_drag_v1(CapturesRecordingCropRect initial,
    CapturesRecordingDimensions source, uint8_t handle, double delta_x,
    double delta_y, bool lock_aspect, CapturesRecordingCropRect *output);
bool captures_recording_max_resolution_constrain_v1(uint8_t preset,
    CapturesRecordingDimensions input, CapturesRecordingDimensions *output);

/* Allocation-free recording trim-handle geometry in host logical points and
 * source-relative fractional milliseconds. Edge 0 is start; edge 1 is end.
 * begin validates a staged range of at least 1ms and captures immutable origin.
 * update applies the shipping 3px threshold (exactly 3 starts), permits fast
 * in-track movement, and ignores samples farther than one track width outside
 * either edge without advancing last_x. The returned drag is the next gesture
 * state and time_ms is the staged trim/playhead value. Pointer up/cancel/lost
 * capture requires no call: discard drag without rolling back host-staged time.
 * Finite duration/width values below 1 normalize to 1 like shipping Tauri.
 * False means null/nonfinite/invalid input and leaves output untouched. Inputs
 * and outputs are copied; functions allocate nothing and access no session/I/O. */
#define CAPTURES_RECORDING_TIMELINE_TRIM_START 0
#define CAPTURES_RECORDING_TIMELINE_TRIM_END 1
typedef struct {
    double start_time_ms, start_x, last_x, min_time_ms, max_time_ms, duration_ms;
    bool dragging;
} CapturesRecordingTimelineTrimDrag;
typedef struct {
    CapturesRecordingTimelineTrimDrag drag;
    double time_ms;
} CapturesRecordingTimelineTrimUpdate;
bool captures_recording_timeline_ratio_v1(double time_ms, double duration_ms,
    double *output);
bool captures_recording_timeline_time_at_x_v1(double client_x, double track_left,
    double track_width, double duration_ms, double *output);
bool captures_recording_timeline_trim_begin_v1(uint8_t edge, double pointer_x,
    double trim_start_ms, double trim_end_ms, double duration_ms,
    CapturesRecordingTimelineTrimDrag *output);
bool captures_recording_timeline_trim_update_v1(CapturesRecordingTimelineTrimDrag drag,
    double client_x, double track_left, double track_width,
    CapturesRecordingTimelineTrimUpdate *output);

/* Shared recording editor prerequisite. Open on one serialized worker with
 * {history_root,artifact_id,ffmpeg,ffprobe}; the artifact must be a real History
 * recording. Success output is owned {ok:true,result:snapshot}; error output is
 * {ok:false,error}. Snapshot is {artifact_id,source,edit,preview_export,position_ms,
 * revision,has_system_audio,has_microphone_audio}. Source audio flags are trusted
 * shared values and caller values in EditSpec are ignored. Requests are tagged
 * snake_case: {operation:"snapshot"}, {operation:"seek",position_ms},
 * {operation:"update_edit",edit}, and
 * {operation:"update_preview",edit,export}. Seek positions are source-relative.
 * preview_export identifies the first export attempt represented by the retained
 * frame. WebM and size-budget previews are unavailable. Accepted requests atomically
 * replace snapshot/frame; failures retain the last good snapshot, frame and revision.
 * Free every JSON response with captures_settings_free_v1. Playback v1 remains
 * silent; playback v2 can explicitly use accepted audio. No persistent draft,
 * replacement, account, or release behavior exists. */
typedef struct CapturesRecordingEditorSession CapturesRecordingEditorSession;
typedef struct CapturesRecordingEditorFrame CapturesRecordingEditorFrame;
typedef struct CapturesRecordingEditorCancel CapturesRecordingEditorCancel;
CapturesRecordingEditorSession *captures_recording_editor_open_v1(
    const char *request_json, char **output);
/* Read-only accepted-session saved_path for a host confirmation hint. Owned
 * {ok:true,result:{path:<string|null>}} or {ok:false,error}; free with
 * captures_settings_free_v1. No filesystem work or replace eligibility promise.
 * Do not substitute a possibly stale History-list artifact path. */
char *captures_recording_editor_original_save_path_v1(
    const CapturesRecordingEditorSession *session);
char *captures_recording_editor_request_v1(CapturesRecordingEditorSession *session,
    const char *request_json);
/* Additive accepted-save-export contract. Requests retain the v1 operation
 * spellings. update_preview accepts MP4/GIF export with an optional hard byte
 * budget; a budget requires Preserve quality and at least 100000 bytes. Success
 * returns the v1 snapshot fields plus save_export. preview_export is the visual
 * first-attempt derivative with max_size_bytes:null, while save_export retains
 * the accepted budget. v1 request/snapshot behavior and budget rejection remain
 * unchanged, including after v2 use. */
char *captures_recording_editor_request_v2(CapturesRecordingEditorSession *session,
    const char *request_json);
void captures_recording_editor_free_v1(CapturesRecordingEditorSession *session);
/* Retained preview RGBA follows CapturesRegionPixels and may outlive edits/session.
 * Borrow only while frame is live; never mutate/free data. NULL free is allowed. */
CapturesRecordingEditorFrame *captures_recording_editor_frame_v1(
    const CapturesRecordingEditorSession *session);
bool captures_recording_editor_frame_pixels_v1(const CapturesRecordingEditorFrame *frame,
    CapturesRegionPixels *output);
void captures_recording_editor_frame_free_v1(CapturesRecordingEditorFrame *frame);

/* Blocking full-source still at the accepted source-relative position. It
 * ignores accepted trim/crop/output/export effects. Success returns an existing
 * retained frame owner plus owned {ok:true,result:{position_ms,width,height}};
 * failure returns NULL plus owned {ok:false,error}. output_json must be non-NULL
 * and is freed with captures_settings_free_v1. Cancel stays live through the
 * call. Frame pixels may outlive session/cancel through the existing frame
 * pixels/free functions. No accepted session or History state is changed. */
CapturesRecordingEditorFrame *captures_recording_editor_source_frame_v1(
    const CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel, char **output_json);

/* Blocking read-only encoded comparison at the accepted source-relative
 * position. Positions outside [trim_start,trim_end) fail rather than clamp.
 * It encodes a short sample with accepted preview_export, not save_export:
 * Maximum mode therefore shows budget-free first-attempt fidelity, never a
 * promise of final capped-save pixels. Success returns a retained owner and
 * owned {ok:true,result:{basis:"accepted_preview_first_attempt",revision,
 * position_ms,after_seek_position_ms,sample_start_ms,sample_duration_ms,
 * attempts,export,width,height}}. position_ms is the selected source time;
 * after_seek_position_ms reports an edge adjustment to the encoded seek, and
 * actual decoded frames may fall on adjacent output cadence timestamps.
 * Error returns NULL plus owned {ok:false,error}. Non-NULL output_json is
 * required before work; free JSON with captures_settings_free_v1. The two
 * frame accessors return independent retained owners using the existing
 * frame_pixels_v1/frame_free_v1 contract; frames outlive comparison/session/
 * cancel. Free the comparison once. Calls are serialized on the owning worker;
 * hosts also guard item switches with their own generation alongside the
 * session-scoped revision/position/export identity. No accepted state or
 * History is changed. Native host UI is not yet wired to this prerequisite. */
typedef struct CapturesRecordingEditorComparison CapturesRecordingEditorComparison;
CapturesRecordingEditorComparison *captures_recording_editor_comparison_v1(
    const CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel, char **output_json);
CapturesRecordingEditorFrame *captures_recording_editor_comparison_before_frame_v1(
    const CapturesRecordingEditorComparison *comparison);
CapturesRecordingEditorFrame *captures_recording_editor_comparison_after_frame_v1(
    const CapturesRecordingEditorComparison *comparison);
void captures_recording_editor_comparison_free_v1(
    CapturesRecordingEditorComparison *comparison);

/* Persistent silent playback of accepted edit + preview_export. position_ms is
 * source-relative and normalizes outside accepted [trim_start,trim_end) to trim
 * start. Success returns a stream plus owned {ok:true,result:{start_position_ms,
 * width,height,frames_per_second}}. Output is aspect-preserving RGBA8 bounded to
 * 1280x720 and at most 30fps. Each next call blocks for shared monotonic pacing:
 * a frame returns its retained CapturesRecordingEditorFrame plus owned
 * {ok:true,result:{eof:false,position_ms}}; exclusive trim end returns NULL plus
 * {ok:true,result:{eof:true}}; failures return NULL plus {ok:false,error}.
 * output_json must be non-NULL; NULL refuses work and next does not advance.
 * Stream/frame may outlive session and cancel owners. Caller cancellation
 * interrupts pacing/reads; normal EOF/free never marks that token cancelled.
 * Free stream to stop/kill/reap/join its one FFmpeg process. Frames use the
 * existing pixel/free functions and may outlive the stream. NULL free allowed. */
typedef struct CapturesRecordingEditorPlayback CapturesRecordingEditorPlayback;
CapturesRecordingEditorPlayback *captures_recording_editor_playback_open_v1(
    const CapturesRecordingEditorSession *session, uint64_t position_ms,
    const CapturesRecordingEditorCancel *cancel, char **output_json);
/* Explicit accepted-audio playback. It otherwise shares v1 positioning,
 * RGBA frames, next/free ownership, bounds, and cancellation. Success adds
 * audio_enabled to the open result. GIF, no source audio, all accepted tracks
 * muted, or all accepted gains zero reports false and does not open a device.
 * Audible playback opens the default output device and a second persistent
 * FFmpeg decoder; unavailable/broken output fails instead of falling back to
 * silence. Create, drive, and free the stream on one serialized owning worker;
 * backend callbacks are internal and do not allocate, block, or lock. */
CapturesRecordingEditorPlayback *captures_recording_editor_playback_open_v2(
    const CapturesRecordingEditorSession *session, uint64_t position_ms,
    const CapturesRecordingEditorCancel *cancel, char **output_json);
CapturesRecordingEditorFrame *captures_recording_editor_playback_next_v1(
    CapturesRecordingEditorPlayback *playback, char **output_json);
void captures_recording_editor_playback_free_v1(
    CapturesRecordingEditorPlayback *playback);

/* Blocking full-source thumbnail generation on the serialized session worker.
 * It uses the shipping 12-frame 160x90 sampling/filter and ignores accepted
 * trim/crop/audio/export/position. Success returns an independently retained
 * owner and owned JSON {ok:true,result:{frame_count,frame_width,frame_height,
 * sprite_width,sprite_height}}; failure returns NULL plus owned {ok:false,error}.
 * output_json must be non-NULL and is always freed with captures_settings_free_v1.
 * The existing independent cancel owner stays live until this call returns.
 * Borrowed RGBA8 pixels may outlive session/edit changes while the thumbnail
 * owner remains live. Pixel failure leaves output untouched; NULL free is allowed. */
typedef struct CapturesRecordingEditorThumbnails CapturesRecordingEditorThumbnails;
CapturesRecordingEditorThumbnails *captures_recording_editor_thumbnails_v1(
    const CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel, char **output_json);
bool captures_recording_editor_thumbnails_pixels_v1(
    const CapturesRecordingEditorThumbnails *thumbnails, CapturesRegionPixels *output);
void captures_recording_editor_thumbnails_free_v1(
    CapturesRecordingEditorThumbnails *thumbnails);

/* Save new copy is blocking on the serialized worker. JSON is
 * {destination,export}, where export is captures-media ExportSpec. It never
 * overwrites a destination or source and returns saved {path,artifact},
 * saved_without_history {path,warning} after post-publication History failure,
 * or an error before publication. Progress receives borrowed NUL-terminated
 * ExportProgress JSON only for the callback and must copy retained data.
 * Cancellation is an independent thread-safe owner: cancel from another thread,
 * but free only after save_new returns. NULL callback is allowed; NULL cancel is
 * an owned error. No call may concurrently access/free the session. */
typedef void (*CapturesRecordingEditorProgress)(void *context, const char *progress_json);
CapturesRecordingEditorCancel *captures_recording_editor_cancel_create_v1(void);
void captures_recording_editor_cancel_v1(const CapturesRecordingEditorCancel *cancel);
void captures_recording_editor_cancel_free_v1(CapturesRecordingEditorCancel *cancel);
/* Blocking interrupted native recording recovery, scoped to the host's
 * isolated history_root and its sibling recording-recovery directory. Calls
 * belong on the serialized recording worker. A live native session's OS lock
 * makes list/recover/discard fail rather than expose its active bundle.
 * list request: {history_root,ffmpeg?,ffprobe?}; action requests add session_id
 * and expected_identity from an available list row. Unavailable/corrupt rows
 * have no identity and cannot be recovered or discarded. Responses are owned
 * {ok:true,result:{drafts:[...]}} / {ok:true,result:{status,entry,path,warning}}
 * / {ok:true,result:{status:"discarded"}} or {ok:false,error}. Free using
 * captures_settings_free_v1. Reuse the thread-safe editor CancelToken owner;
 * no editor session is involved. NULL cancel is an error. Progress callback
 * receives borrowed {stage:"scanning"|"assembling"|"poster"|"publishing"}
 * and may be NULL. Cancellation before publication retains draft media. */
typedef void (*CapturesRecordingRecoveryProgress)(void *context, const char *progress_json);
char *captures_recording_recovery_list_v1(const char *request_json);
char *captures_recording_recovery_recover_v1(const char *request_json,
    const CapturesRecordingEditorCancel *cancel,
    CapturesRecordingRecoveryProgress progress, void *context);
char *captures_recording_recovery_discard_v1(const char *request_json);
/* Blocking estimate of the accepted edit + preview_export. Returns owned
 * {ok:true,result:{size_bytes,exact}} or {ok:false,error}. It does not mutate
 * session/frame/revision, publish History, or invoke a progress callback. */
char *captures_recording_editor_estimate_v1(const CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel);
/* Blocking estimate of accepted edit + save_export. Maximum-mode hosts should
 * present the accepted hard cap rather than promise this advisory encoded
 * estimate. Ownership, cancellation, and response envelopes match v1. */
char *captures_recording_editor_estimate_v2(const CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel);
char *captures_recording_editor_save_new_v1(const CapturesRecordingEditorSession *session,
    const char *request_json, const CapturesRecordingEditorCancel *cancel,
    CapturesRecordingEditorProgress progress, void *context);
/* Blocking same-format replacement of an existing permanent MP4/GIF with an
 * identical private History recovery copy. Requires exclusive session ownership
 * on its worker; cancel is live until return and may be signaled elsewhere.
 * Progress JSON is borrowed during the callback. Owned response must be freed
 * with captures_settings_free_v1. Success:
 * {ok:true,result:{replacement:{status:"replaced",path,artifact},snapshot:<v2>}}.
 * Error: {ok:false,error,requires_reopen}. False means no permanent change
 * (or completed compensation); true means close/reopen before any more media
 * operations. No request/snapshot v1/v2 shape is changed. Old retained frames
 * survive. This is not a cross-directory crash/power-loss atomic transaction:
 * a process kill may leave permanent new while History recovery is old. */
char *captures_recording_editor_replace_original_v1(CapturesRecordingEditorSession *session,
    const CapturesRecordingEditorCancel *cancel,
    CapturesRecordingEditorProgress progress, void *context);

/* Allocation-free macOS window-radius fallback in points. Pass the current OS
 * major version from ProcessInfo. No OS access or session handle is required. */
double captures_macos_window_corner_radius_v1(int64_t major_version);

/* Window sessions share region-session ownership and pixel layout. Prepare on
 * a worker after hiding capture windows, retaining an event-loop flow guard.
 * fallback_corner_radius is the finite, nonnegative OS radius (0 outside macOS).
 * No implicit permission prompt. NULL output refuses preparation; otherwise it
 * receives owned {ok,result:{display,windows,shell_chrome}} or {ok,error} JSON.
 * Freeze/cursor preferences remain fixed for the session. */
typedef struct CapturesWindowSession CapturesWindowSession;
typedef CapturesRegionPixels CapturesWindowPixels;
CapturesWindowSession *captures_window_prepare_v1(const char *display_id,
    uint64_t generation, bool freeze, bool include_cursor,
    double fallback_corner_radius, char **output);
/* Same borrowed RGBA8 contract as captures_region_pixels_v1. */
bool captures_window_pixels_v1(const CapturesWindowSession *session, CapturesWindowPixels *output);
/* Allocation-free pointer path; point is display-local overlay/DIP coordinates.
 * -1 means display, >=0 indexes prepare.windows. False for nulls/nonfinite points
 * leaves output unchanged. Retain the session through this call; non-null output
 * must be aligned writable int64_t storage. No pointer-event JSON is needed. */
bool captures_window_hit_test_v1(const CapturesWindowSession *session,
    CapturesSelectionPoint point, int64_t *output);
/* target_json: {"kind":"window","id":"..."}, {"kind":"display"}, or
 * {"kind":"region","rect":{"x":N,"y":N,"width":N,"height":N}}.
 * Region coordinates are display-local logical units, using the same prepared
 * desktop and cursor snapshot as the other targets; no second session is needed.
 * Both strings are readable UTF-8/NUL-terminated for the call. Window IDs must
 * belong to the prepared picker. Display captures cover desktop/shell targets.
 * Nonzero countdown requires after_countdown=true: fresh geometry and pixels,
 * even if frozen. A missing target or target moved off this display fails.
 * Hide/settle selector/countdown windows first. Free returned JSON with
 * captures_settings_free_v1. Keep session alive through every worker call. */
char *captures_window_capture_v1(const CapturesWindowSession *session,
    const char *root, const char *target_json, bool after_countdown);
/* Exactly once, after all workers/providers/borrows finish. NULL is permitted.
 * Separately finish the event-loop capture-flow guard on every exit path. */
void captures_window_free_v1(CapturesWindowSession *session);

/* Owned mutable recording lifecycle. Prepare and every request may block: run
 * them on one serialized worker, never AppKit's event thread. Prepare JSON is
 * {recovery_root,options,display}; success returns {snapshot}. Lifecycle request
 * operations are snapshot, microphone_level, start, pause,
 * set_microphone_muted, restart, stop, finish and discard. Microphone_level
 * returns {microphone_peak: number} in [0,1], or zero when not actively
 * recording with an unmuted microphone; polling does not write media/state.
 * Start and set_microphone_muted accept
 * generation/exclude_captures_app and require an is_current callback, invoked
 * around any engine opening together with the shared capture-flow gate;
 * it must only read a thread-safe host cancellation gate. Finish accepts
 * history_root/ffmpeg/ffprobe file paths and
 * returns FinalizedRecording metadata/path, never media JSON/base64. Stop/discard
 * an active handle before free. Info operations are capabilities,
 * microphone_devices, and history {root}; history returns recording metadata and
 * native media/poster paths only. Free may block while platform Drop aborts an
 * active engine, but is not durable Stop/finalization: explicitly stop/discard
 * first on the worker. The generation callback must not reenter this ABI. All
 * responses use the standard owned envelope. */
typedef struct CapturesRecordingSession CapturesRecordingSession;
typedef bool (*CapturesRecordingIsCurrent)(void *context, uint64_t generation);
char *captures_recording_info_v1(const char *request_json);
CapturesRecordingSession *captures_recording_prepare_v1(const char *request_json,
    char **output);
char *captures_recording_request_v1(CapturesRecordingSession *handle,
    const char *request_json, CapturesRecordingIsCurrent is_current, void *context);
void captures_recording_free_v1(CapturesRecordingSession *handle);

/* Versioned JSON ABI. Operations are load, save, default_path, and theme.
 * Theme accepts {"operation":"theme","accent":"#rgb","signal":"#rrggbb",
 * "light":true} and returns {"ok":true,"colors":{...}}.
 * `request_json` must be a non-null, NUL-terminated UTF-8
 * string no larger than 8 MiB. The returned string is always NUL-terminated,
 * owned by Rust, and must be released exactly once with captures_settings_free_v1.
 * Passing NULL to free is permitted. Never use another allocator. */
char *captures_settings_request_v1(const char *request_json);
/* Native application operations from captures-app::Request. This may block on
 * capture/encoding/disk: call off the UI thread. Returns {ok,result} or {ok,error}.
 * File paths, not image bytes, cross this ABI. The same free function owns both.
 * Permission is prompted only by the explicit request_permission operation. */
char *captures_app_request_v1(const char *request_json);
/* Event-loop-thread ONLY: begin {seconds},
 * begin_recording_screenshot {parent_generation,seconds}, poll {generation},
 * disarm_escape {generation}, restart_countdown {generation,seconds},
 * finish {generation}.
 * For selection, begin with seconds=0; start_countdown {generation,seconds} after
 * confirmation starts the delay without dropping Escape or changing generation.
 * begin returns {generation}; poll returns {current,remaining}. A recording
 * screenshot is a temporary child and never replaces or commits its disarmed
 * parent. Escape cancels that child only. Otherwise Escape is global only until
 * disarm_escape hands an accepted recording to its session owner.
 * Always finish, including on cancellation/quit. The guard
 * owns native handles on this thread; never dispatch these calls to a worker.
 * Uses the same {ok,result}/{ok,error} envelope and response ownership as above. */
char *captures_flow_request_v1(const char *request_json);
void captures_settings_free_v1(char *response);

#endif
