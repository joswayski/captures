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
 * Requests block for at most the client's 20-second HTTP timeout; never call on
 * the UI thread or serialize behind capture/recording work. */
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
/* Event-loop-thread ONLY: begin {seconds}, poll {generation},
 * disarm_escape {generation}, restart_countdown {generation,seconds},
 * finish {generation}.
 * For selection, begin with seconds=0; start_countdown {generation,seconds} after
 * confirmation starts the delay without dropping Escape or changing generation.
 * begin returns {generation}; poll returns {current,remaining}. Escape is global
 * only until disarm_escape hands an accepted recording to its session owner.
 * Always finish, including on cancellation/quit. The guard
 * owns native handles on this thread; never dispatch these calls to a worker.
 * Uses the same {ok,result}/{ok,error} envelope and response ownership as above. */
char *captures_flow_request_v1(const char *request_json);
void captures_settings_free_v1(char *response);

#endif
