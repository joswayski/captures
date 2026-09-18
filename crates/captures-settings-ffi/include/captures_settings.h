#ifndef CAPTURES_SETTINGS_H
#define CAPTURES_SETTINGS_H

#include <stdbool.h>
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
 * begin returns {generation}; poll returns {current,remaining}. Escape is global
 * only for this guard. Always finish, including on cancellation/quit. The guard
 * owns native handles on this thread; never dispatch these calls to a worker.
 * Uses the same {ok,result}/{ok,error} envelope and response ownership as above. */
char *captures_flow_request_v1(const char *request_json);
void captures_settings_free_v1(char *response);

#endif
