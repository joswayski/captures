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

/* Shared recording editor prerequisite. Open on one serialized worker with
 * {history_root,artifact_id,ffmpeg,ffprobe}; the artifact must be a real History
 * recording. Success output is owned {ok:true,result:snapshot}; error output is
 * {ok:false,error}. Snapshot is {artifact_id,source,edit,position_ms,revision,
 * has_system_audio,has_microphone_audio}. Source audio flags are trusted shared
 * values and caller values in EditSpec are ignored. Requests are tagged snake_case:
 * {operation:"snapshot"}, {operation:"seek",position_ms}, and
 * {operation:"update_edit",edit}. Seek positions are source-relative. Accepted
 * requests atomically replace snapshot/frame; failures retain the last good pair.
 * Free every JSON response with captures_settings_free_v1. No playback, audio
 * output, persistent draft, replacement, account, or release behavior exists. */
typedef struct CapturesRecordingEditorSession CapturesRecordingEditorSession;
typedef struct CapturesRecordingEditorFrame CapturesRecordingEditorFrame;
CapturesRecordingEditorSession *captures_recording_editor_open_v1(
    const char *request_json, char **output);
char *captures_recording_editor_request_v1(CapturesRecordingEditorSession *session,
    const char *request_json);
void captures_recording_editor_free_v1(CapturesRecordingEditorSession *session);
/* Retained preview RGBA follows CapturesRegionPixels and may outlive edits/session.
 * Borrow only while frame is live; never mutate/free data. NULL free is allowed. */
CapturesRecordingEditorFrame *captures_recording_editor_frame_v1(
    const CapturesRecordingEditorSession *session);
bool captures_recording_editor_frame_pixels_v1(const CapturesRecordingEditorFrame *frame,
    CapturesRegionPixels *output);
void captures_recording_editor_frame_free_v1(CapturesRecordingEditorFrame *frame);

/* Save new copy is blocking on the serialized worker. JSON is
 * {destination,export}, where export is captures-media ExportSpec. It never
 * overwrites a destination or source and returns saved {path,artifact},
 * saved_without_history {path,warning} after post-publication History failure,
 * or an error before publication. Progress receives borrowed NUL-terminated
 * ExportProgress JSON only for the callback and must copy retained data.
 * Cancellation is an independent thread-safe owner: cancel from another thread,
 * but free only after save_new returns. NULL callback is allowed; NULL cancel is
 * an owned error. No call may concurrently access/free the session. */
typedef struct CapturesRecordingEditorCancel CapturesRecordingEditorCancel;
typedef void (*CapturesRecordingEditorProgress)(void *context, const char *progress_json);
CapturesRecordingEditorCancel *captures_recording_editor_cancel_create_v1(void);
void captures_recording_editor_cancel_v1(const CapturesRecordingEditorCancel *cancel);
void captures_recording_editor_cancel_free_v1(CapturesRecordingEditorCancel *cancel);
char *captures_recording_editor_save_new_v1(const CapturesRecordingEditorSession *session,
    const char *request_json, const CapturesRecordingEditorCancel *cancel,
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
 * operations are snapshot, start, pause, set_microphone_muted, restart, stop,
 * finish and discard. Start and set_microphone_muted accept
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
