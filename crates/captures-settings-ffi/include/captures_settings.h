#ifndef CAPTURES_SETTINGS_H
#define CAPTURES_SETTINGS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

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
/* Event-loop-thread ONLY: begin {seconds}, poll {generation}, finish {generation}.
 * For selection, begin with seconds=0; start_countdown {generation,seconds} after
 * confirmation starts the delay without dropping Escape or changing generation.
 * begin returns {generation}; poll returns {current,remaining}. Escape is global
 * only for this guard. Always finish, including on cancellation/quit. The guard
 * owns native handles on this thread; never dispatch these calls to a worker.
 * Uses the same {ok,result}/{ok,error} envelope and response ownership as above. */
char *captures_flow_request_v1(const char *request_json);
void captures_settings_free_v1(char *response);

#endif
