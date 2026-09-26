//! Native hosts borrow the shipping placement and capture-visibility policy.
use captures_app::preview::{
    self, ThumbnailMonitorBounds, ThumbnailStackAnchor, ThumbnailStackOrigin, ThumbnailVisibility,
};
use captures_app::preview_chrome::{self, CardHoverLock, EditorPhase, EditorPresence};
use captures_app::preview_motion;
use captures_app::tray_notice::LogicalRect;
use captures_settings::MiniPreviewPlacement;
use std::ffi::{CStr, CString, c_char};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapturesPreviewMonitor {
    pub work_x: i32,
    pub work_y: i32,
    pub work_width: u32,
    pub work_height: u32,
    pub full_x: i32,
    pub full_y: i32,
    pub full_width: u32,
    pub full_height: u32,
    pub scale_factor: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapturesPreviewOrigin {
    pub x: f64,
    pub edge: f64,
    pub anchor: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesPreviewGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub card_height: f64,
    pub padding: f64,
    pub control_gutter: f64,
    pub anchor: u32,
}

/// Allocation-free physical-monitor to logical-window placement. See the header
/// for placement/anchor values and the top-left desktop coordinate convention.
///
/// # Safety
/// Non-null origin/output point to aligned readable/writable storage respectively.
/// Pointers are borrowed only during this call. False leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_geometry_v1(
    monitor: CapturesPreviewMonitor,
    count: usize,
    collapsed: bool,
    origin: *const CapturesPreviewOrigin,
    placement: u32,
    output: *mut CapturesPreviewGeometry,
) -> bool {
    if output.is_null()
        || !monitor.scale_factor.is_finite()
        || monitor.scale_factor <= 0.
        || monitor.work_width == 0
        || monitor.work_height == 0
        || monitor.full_width == 0
        || monitor.full_height == 0
    {
        return false;
    }
    let placement = match placement {
        0 => MiniPreviewPlacement::BottomLeft,
        1 => MiniPreviewPlacement::BottomRight,
        2 => MiniPreviewPlacement::TopLeft,
        3 => MiniPreviewPlacement::TopRight,
        _ => return false,
    };
    // SAFETY: The caller guarantees a readable aligned origin or null.
    let origin = match unsafe { origin.as_ref() } {
        None => None,
        Some(origin) => {
            if !origin.x.is_finite() || !origin.edge.is_finite() {
                return false;
            }
            let anchor = match origin.anchor {
                0 => ThumbnailStackAnchor::Bottom,
                1 => ThumbnailStackAnchor::Top,
                _ => return false,
            };
            Some(ThumbnailStackOrigin {
                x: origin.x,
                edge: origin.edge,
                anchor,
            })
        }
    };
    let geometry = preview::thumbnail_geometry(
        ThumbnailMonitorBounds {
            work_x: monitor.work_x,
            work_y: monitor.work_y,
            work_width: monitor.work_width,
            work_height: monitor.work_height,
            full_x: monitor.full_x,
            full_y: monitor.full_y,
            full_width: monitor.full_width,
            full_height: monitor.full_height,
            scale_factor: monitor.scale_factor,
        },
        count,
        collapsed,
        origin,
        placement,
    );
    // SAFETY: Validated non-null output is writable for one value.
    unsafe {
        output.write(CapturesPreviewGeometry {
            x: geometry.x,
            y: geometry.y,
            width: preview::THUMBNAIL_WIDTH,
            height: geometry.height,
            card_height: preview::THUMBNAIL_CARD_HEIGHT,
            padding: preview::THUMBNAIL_PADDING,
            control_gutter: preview::THUMBNAIL_CONTROL_GUTTER,
            anchor: u32::from(geometry.anchor.is_top()),
        });
    }
    true
}

/// Opaque, single-owner policy. Calls on one handle must never overlap.
pub struct CapturesPreviewVisibility(ThumbnailVisibility);

#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_visibility_new_v1() -> *mut CapturesPreviewVisibility {
    Box::into_raw(Box::new(CapturesPreviewVisibility(
        ThumbnailVisibility::default(),
    )))
}

/// # Safety
/// A non-null handle must be live, uniquely owned and freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_visibility_free_v1(
    handle: *mut CapturesPreviewVisibility,
) {
    if !handle.is_null() {
        // SAFETY: The caller transfers the Box allocated by new, once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Begin suppression. Refuses a second capture until the first waits for pixels.
/// # Safety
/// Handle is live/exclusive or null. Output is writable/aligned or null.
/// False leaves output unchanged, including when output is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_begin_v1(
    handle: *mut CapturesPreviewVisibility,
    output: *mut u64,
) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: A non-null handle is live and exclusively borrowed for this call.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(generation) = handle.0.begin_capture() else {
        return false;
    };
    // SAFETY: Validated non-null writable output.
    unsafe {
        output.write(generation);
    }
    true
}

/// Wait for the named artifact's decoded pixels; stale generations do nothing.
/// # Safety
/// Handle is live/exclusive or null. Artifact is null or a readable UTF-8,
/// NUL-terminated string for this call; Rust copies it, never retains the pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_wait_v1(
    handle: *mut CapturesPreviewVisibility,
    generation: u64,
    artifact: *const c_char,
) -> bool {
    if artifact.is_null() {
        return false;
    }
    // SAFETY: Non-null arguments obey the documented handle/string contracts.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Ok(artifact) = (unsafe { CStr::from_ptr(artifact) }).to_str() else {
        return false;
    };
    handle.0.wait_for_artifact(generation, artifact.to_owned())
}

/// Release capture suppression only for the pending artifact.
/// # Safety
/// Same handle/string contracts as captures_preview_wait_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_ready_v1(
    handle: *mut CapturesPreviewVisibility,
    artifact: *const c_char,
) -> bool {
    if artifact.is_null() {
        return false;
    }
    // SAFETY: Non-null arguments obey the documented handle/string contracts.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Ok(artifact) = (unsafe { CStr::from_ptr(artifact) }).to_str() else {
        return false;
    };
    handle.0.mark_artifact_ready(artifact)
}

/// Restore after cancellation/error; stale generations cannot restore newer work.
/// # Safety
/// Handle is live and exclusively borrowed for the call, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_restore_v1(
    handle: *mut CapturesPreviewVisibility,
    generation: u64,
) -> bool {
    // SAFETY: Non-null handle is live and exclusively borrowed.
    unsafe { handle.as_mut() }.is_some_and(|handle| handle.0.restore_capture(generation))
}

/// Clear a pending artifact wait after decode failure/dismissal, not an active capture.
/// # Safety
/// Handle is live and exclusively borrowed for the call, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stop_waiting_v1(
    handle: *mut CapturesPreviewVisibility,
) -> bool {
    // SAFETY: Non-null handle is live and exclusively borrowed.
    unsafe { handle.as_mut() }.is_some_and(|handle| handle.0.stop_waiting_for_artifact())
}

/// Independently suppress while capture UI is present; false for a null handle.
/// # Safety
/// Handle is live and exclusively borrowed for the call, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_capture_ui_v1(
    handle: *mut CapturesPreviewVisibility,
    suppressed: bool,
) -> bool {
    // SAFETY: Non-null handle is live and exclusively borrowed.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    if suppressed {
        handle.0.suppress_for_capture_ui();
    } else {
        handle.0.restore_capture_ui();
    }
    true
}

/// Query shared visibility policy. A null handle is never visible.
/// # Safety
/// Handle is live and not concurrently mutated/freed, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_visible_v1(
    handle: *const CapturesPreviewVisibility,
    count: usize,
    enabled: bool,
    include_in_captures: bool,
) -> bool {
    // SAFETY: Non-null handle is live and immutable for this call.
    unsafe { handle.as_ref() }.is_some_and(|handle| {
        preview::stack_should_be_visible(
            count,
            handle.0.is_suppressed(),
            enabled,
            include_in_captures,
        )
    })
}

/// Owned chronological membership and compact/expanded presentation policy.
pub struct CapturesPreviewStack(preview::PreviewStack);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesPreviewCardLayout {
    pub y: f64,
    pub depth: usize,
    pub interactive: bool,
}

#[repr(C)]
pub struct CapturesPreviewID {
    pub data: *const u8,
    pub length: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_stack_new_v1() -> *mut CapturesPreviewStack {
    Box::into_raw(Box::new(CapturesPreviewStack(
        preview::PreviewStack::default(),
    )))
}

/// # Safety
/// Handle is null or a live, uniquely owned handle freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_free_v1(handle: *mut CapturesPreviewStack) {
    if !handle.is_null() {
        // SAFETY: Caller transfers exclusive ownership of this live allocation.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Append a nonempty ID; duplicates do not reorder or expand the pile.
/// # Safety
/// Handle is null or live/exclusive. ID is null or readable NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_insert_v1(
    handle: *mut CapturesPreviewStack,
    id: *const c_char,
) -> bool {
    if id.is_null() {
        return false;
    }
    // SAFETY: Non-null pointers satisfy the documented handle/string contract.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Ok(id) = (unsafe { CStr::from_ptr(id) }).to_str() else {
        return false;
    };
    handle.0.insert(id.to_owned())
}

/// Remove membership only, never files/history. False for an absent/invalid ID.
/// # Safety
/// Same handle/string contract as captures_preview_stack_insert_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_remove_v1(
    handle: *mut CapturesPreviewStack,
    id: *const c_char,
) -> bool {
    if id.is_null() {
        return false;
    }
    // SAFETY: Non-null pointers satisfy the documented handle/string contract.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Ok(id) = (unsafe { CStr::from_ptr(id) }).to_str() else {
        return false;
    };
    handle.0.remove(id)
}

/// # Safety
/// Handle is null or live and not concurrently mutated/freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_count_v1(
    handle: *const CapturesPreviewStack,
) -> usize {
    // SAFETY: Caller guarantees shared access to a live handle or null.
    unsafe { handle.as_ref() }.map_or(0, |handle| handle.0.ids().len())
}

/// Unclamped logical content height, including the control gutter; zero if empty.
/// # Safety
/// Handle is null or live and not concurrently mutated/freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_height_v1(
    handle: *const CapturesPreviewStack,
) -> f64 {
    // SAFETY: Caller guarantees shared access to a live handle or null.
    unsafe { handle.as_ref() }.map_or(0., |handle| handle.0.content_height())
}

/// Borrow an ID until the next mutation/free. Output is unchanged on failure.
/// # Safety
/// Handle is null or live without concurrent mutation/free. Output is null or
/// aligned writable storage. Never free the returned bytes; copy before mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_id_v1(
    handle: *const CapturesPreviewStack,
    index: usize,
    output: *mut CapturesPreviewID,
) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees shared access to live handle or null.
    let Some(id) = (unsafe { handle.as_ref() }).and_then(|handle| handle.0.ids().get(index)) else {
        return false;
    };
    // SAFETY: Validated writable output; bytes stay owned by the live handle.
    unsafe {
        output.write(CapturesPreviewID {
            data: id.as_ptr(),
            length: id.len(),
        });
    }
    true
}

/// # Safety
/// Handle is null or live and exclusively borrowed for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_set_collapsed_v1(
    handle: *mut CapturesPreviewStack,
    collapsed: bool,
) -> bool {
    // SAFETY: Caller guarantees exclusive access to live handle or null.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    handle.0.set_collapsed(collapsed);
    true
}

/// # Safety
/// Handle is null or live without concurrent mutation/free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_collapsed_v1(
    handle: *const CapturesPreviewStack,
) -> bool {
    // SAFETY: Caller guarantees shared access to live handle or null.
    unsafe { handle.as_ref() }.is_some_and(|handle| handle.0.is_collapsed())
}

/// Pure compact-card shade policy. Depth zero is the undimmed front image.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_dim_opacity_v1(depth: usize) -> f64 {
    preview::collapsed_dim_opacity(depth)
}

/// Allocation-free card pose for a chronological index, in unscrolled content.
/// # Safety
/// Handle is null or live without concurrent mutation/free. Output is null or
/// aligned writable storage. False leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_card_v1(
    handle: *const CapturesPreviewStack,
    index: usize,
    top_anchor: bool,
    output: *mut CapturesPreviewCardLayout,
) -> bool {
    // SAFETY: v2 has identical ownership/output requirements; v1 is the rest pose.
    unsafe { captures_preview_stack_card_v2(handle, index, top_anchor, false, output) }
}

/// Hover-aware card pose. This extends, rather than changes, the v1 ABI.
/// # Safety
/// The requirements are identical to `captures_preview_stack_card_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_stack_card_v2(
    handle: *const CapturesPreviewStack,
    index: usize,
    top_anchor: bool,
    hovered: bool,
    output: *mut CapturesPreviewCardLayout,
) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees shared access to live handle or null.
    let Some(card) = (unsafe { handle.as_ref() })
        .and_then(|handle| handle.0.card_layout_hovered(index, top_anchor, hovered))
    else {
        return false;
    };
    // SAFETY: Validated writable output.
    unsafe {
        output.write(CapturesPreviewCardLayout {
            y: card.y,
            depth: card.depth,
            interactive: card.interactive,
        });
    }
    true
}

/// Idle mini-preview metadata ("W × H · size"). Returns owned UTF-8; free
/// with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_card_metadata_v1(
    width: u32,
    height: u32,
    size_bytes: u64,
) -> *mut c_char {
    CString::new(preview::card_metadata(width, height, size_bytes))
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Overflow-cue edges for an expanded stack: bit 0 = cards hidden above the
/// viewport, bit 1 = cards hidden below. Uses the shipping 1 px tolerance.
pub const CAPTURES_PREVIEW_OVERFLOW_ABOVE: u32 = 1;
pub const CAPTURES_PREVIEW_OVERFLOW_BELOW: u32 = 2;

#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_overflow_v1(
    scroll_top: f64,
    content_height: f64,
    viewport_height: f64,
) -> u32 {
    let overflow = preview::stack_overflow(scroll_top, content_height, viewport_height);
    u32::from(overflow.above) * CAPTURES_PREVIEW_OVERFLOW_ABOVE
        + u32::from(overflow.below) * CAPTURES_PREVIEW_OVERFLOW_BELOW
}

/// Scroll offset after an overflow cue moves `slots` whole cards (negative is
/// up), clamped to the scrollable range.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_scroll_target_v1(
    scroll_top: f64,
    content_height: f64,
    viewport_height: f64,
    slots: i32,
) -> f64 {
    preview::stack_scroll_target(scroll_top, content_height, viewport_height, slots)
}

/// Shipping overflow-cue name ("Show older captures"/"Show newer captures").
/// Returns static UTF-8; never free it.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_overflow_label_v1(
    above: bool,
    top_anchor: bool,
) -> *const c_char {
    match preview::overflow_cue_label(above, top_anchor) {
        "Show older captures" => c"Show older captures".as_ptr(),
        _ => c"Show newer captures".as_ptr(),
    }
}

/// Editor presence phases (`captures_app::preview_chrome::EditorPhase`).
pub const CAPTURES_EDITOR_PHASE_IDLE: u32 = 0;
pub const CAPTURES_EDITOR_PHASE_PRESENT: u32 = 1;
pub const CAPTURES_EDITOR_PHASE_LEAVING: u32 = 2;
pub const CAPTURES_EDITOR_PHASE_LINGERING: u32 = 3;

/// Plain-field editor presence a host stores per card.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesEditorPresence {
    pub active: bool,
    pub phase: u32,
    pub since_ms: f64,
}

fn editor_phase(raw: u32) -> EditorPhase {
    match raw {
        CAPTURES_EDITOR_PHASE_PRESENT => EditorPhase::Present,
        CAPTURES_EDITOR_PHASE_LEAVING => EditorPhase::Leaving,
        CAPTURES_EDITOR_PHASE_LINGERING => EditorPhase::Lingering,
        _ => EditorPhase::Idle,
    }
}

fn raw_editor_phase(phase: EditorPhase) -> u32 {
    match phase {
        EditorPhase::Idle => CAPTURES_EDITOR_PHASE_IDLE,
        EditorPhase::Present => CAPTURES_EDITOR_PHASE_PRESENT,
        EditorPhase::Leaving => CAPTURES_EDITOR_PHASE_LEAVING,
        EditorPhase::Lingering => CAPTURES_EDITOR_PHASE_LINGERING,
    }
}

/// Report whether an editor window shows this card's capture at `now_ms`
/// (any monotonic millisecond clock) and advance the leave/linger timers.
/// Returns whether the phase changed. Null is ignored.
///
/// # Safety
/// Non-null `presence` points to aligned, writable storage for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_editor_presence_update_v1(
    presence: *mut CapturesEditorPresence,
    active: bool,
    now_ms: f64,
    reduced_motion: bool,
) -> bool {
    // SAFETY: The caller guarantees a valid, exclusive pointer when non-null.
    let Some(raw) = (unsafe { presence.as_mut() }) else {
        return false;
    };
    let mut state = EditorPresence::from_parts(raw.active, editor_phase(raw.phase), raw.since_ms);
    let changed = state.set_active(active, now_ms, reduced_motion);
    *raw = CapturesEditorPresence {
        active: state.active(),
        phase: raw_editor_phase(state.phase()),
        since_ms: state.since_ms(),
    };
    changed
}

/// Milliseconds until the presence phase next changes, or -1 when nothing is
/// pending (idle or present).
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_editor_presence_next_v1(
    presence: CapturesEditorPresence,
    now_ms: f64,
    reduced_motion: bool,
) -> f64 {
    EditorPresence::from_parts(
        presence.active,
        editor_phase(presence.phase),
        presence.since_ms,
    )
    .next_change_in_ms(now_ms, reduced_motion)
    .unwrap_or(-1.)
}

/// Editor control copy. A present control returns its pill label ("In editor",
/// or "Show in editor" on hover/focus unless just opened); other phases return
/// "Edit". Returns static UTF-8; never free it.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_editor_label_v1(
    phase: u32,
    hovered_or_focused: bool,
    just_opened: bool,
) -> *const c_char {
    let label = if editor_phase(phase).present() {
        preview_chrome::editor_pill_label(hovered_or_focused, just_opened)
    } else {
        preview_chrome::EDIT_LABEL
    };
    match label {
        preview_chrome::EDITOR_PRESENT_LABEL => c"In editor".as_ptr(),
        preview_chrome::EDITOR_SHOW_LABEL => c"Show in editor".as_ptr(),
        _ => c"Edit".as_ptr(),
    }
}

/// Width of the present pill for measured label widths (both labels share one
/// cell, so it never jumps on hover).
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_editor_pill_width_v1(
    rest_label_width: f64,
    hover_label_width: f64,
) -> f64 {
    preview_chrome::editor_pill_width(rest_label_width, hover_label_width)
}

/// Stale-pointer hover lock a host stores for its stack.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesCardHoverLock {
    pub locked: bool,
    pub has_origin: bool,
    pub origin_x: f64,
    pub origin_y: f64,
}

fn hover_lock(raw: &CapturesCardHoverLock) -> CardHoverLock {
    CardHoverLock::from_parts(
        raw.locked,
        raw.has_origin.then_some((raw.origin_x, raw.origin_y)),
    )
}

fn store_hover_lock(raw: &mut CapturesCardHoverLock, lock: CardHoverLock) {
    let origin = lock.origin();
    *raw = CapturesCardHoverLock {
        locked: lock.locked(),
        has_origin: origin.is_some(),
        origin_x: origin.map_or(0., |(x, _)| x),
        origin_y: origin.map_or(0., |(_, y)| y),
    };
}

/// Hover-lock commands for `captures_preview_hover_lock_v1`.
pub const CAPTURES_HOVER_LOCK_LOCK: u32 = 0;
pub const CAPTURES_HOVER_LOCK_POINTER: u32 = 1;
pub const CAPTURES_HOVER_LOCK_POINTER_OUTSIDE: u32 = 2;
pub const CAPTURES_HOVER_LOCK_RESAMPLE: u32 = 3;
pub const CAPTURES_HOVER_LOCK_UNLOCK: u32 = 4;

/// Apply one command to a stack's hover lock: lock after an expand or a new
/// capture, feed a pointer sample at window point (`x`, `y`) or outside the
/// window, re-sample after the window moved, or unlock. Returns whether hover
/// stays locked. Null returns false.
///
/// # Safety
/// Non-null `lock` points to aligned, writable storage for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_hover_lock_v1(
    lock: *mut CapturesCardHoverLock,
    command: u32,
    x: f64,
    y: f64,
) -> bool {
    // SAFETY: The caller guarantees a valid, exclusive pointer when non-null.
    let Some(raw) = (unsafe { lock.as_mut() }) else {
        return false;
    };
    let mut state = hover_lock(raw);
    match command {
        CAPTURES_HOVER_LOCK_LOCK => state.lock(),
        CAPTURES_HOVER_LOCK_POINTER => {
            state.pointer(Some((x, y)));
        }
        CAPTURES_HOVER_LOCK_POINTER_OUTSIDE => {
            state.pointer(None);
        }
        CAPTURES_HOVER_LOCK_RESAMPLE => state.resample_origin(),
        CAPTURES_HOVER_LOCK_UNLOCK => state.unlock(),
        _ => {}
    }
    store_hover_lock(raw, state);
    state.locked()
}

/// Card and stack icon tooltip frame (y-down): centered on `anchor`, opening
/// above or below it and nudging while `progress` runs 0→1.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_icon_tooltip_frame_v1(
    anchor: crate::tray_notice::CapturesTrayNoticeRect,
    text_width: f64,
    text_height: f64,
    above: bool,
    progress: f64,
) -> crate::tray_notice::CapturesTrayNoticeRect {
    let frame = preview_chrome::icon_tooltip_frame(
        LogicalRect::new(anchor.x, anchor.y, anchor.width, anchor.height),
        text_width,
        text_height,
        above,
        progress,
    );
    crate::tray_notice::CapturesTrayNoticeRect {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
    }
}

/// Hover media treatment (`blur(2px) brightness(.5) scale(1.015)`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesPreviewHoverMedia {
    pub blur: f64,
    pub brightness: f64,
    pub scale: f64,
}

#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_hover_media_v1() -> CapturesPreviewHoverMedia {
    CapturesPreviewHoverMedia {
        blur: preview_chrome::HOVER_MEDIA_BLUR,
        brightness: preview_chrome::HOVER_MEDIA_BRIGHTNESS,
        scale: preview_chrome::HOVER_MEDIA_SCALE,
    }
}

/// Shipping card warning beside the metadata ("Not in History", then
/// "Clipboard unavailable"), or null. Static UTF-8; never free it.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_card_warning_v1(
    clipboard_current: bool,
    history_saved: bool,
    copy_failed: bool,
) -> *const c_char {
    match preview_chrome::card_warning(clipboard_current, history_saved, copy_failed) {
        Some(preview_chrome::WARNING_NOT_IN_HISTORY) => c"Not in History".as_ptr(),
        Some(_) => c"Clipboard unavailable".as_ptr(),
        None => std::ptr::null(),
    }
}

/// Dust chips for a Delete as a JSON array of `DustParticle` objects
/// (camelCase keys). Owned UTF-8; free with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_dust_particles_v1(
    card_width: f64,
    card_height: f64,
    image_width: f64,
    image_height: f64,
    origin_x: f64,
    origin_y: f64,
    seed: u32,
) -> *mut c_char {
    let finite = [
        card_width,
        card_height,
        image_width,
        image_height,
        origin_x,
        origin_y,
    ]
    .iter()
    .all(|value| value.is_finite());
    if !finite {
        return std::ptr::null_mut();
    }
    let particles = preview_motion::dust_particles(
        card_width,
        card_height,
        (image_width, image_height),
        (origin_x, origin_y),
        seed,
    );
    serde_json::to_string(&particles)
        .ok()
        .and_then(|json| CString::new(json).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Exit holds and settle delays, the Clear all stagger and the pile sparkle
/// tables: `{exits:{dismiss:{hold_ms,settle_delay_ms},dust,delete_fallback,
/// clear_stagger_ms,clear_stagger_max_ms,dust_pad,delete_origin:{first_x,
/// after_close_x,y}},sparkles:{reach,side,near,
/// early:[{x,y,core,fade,accent,alpha}],late}}`. Owned UTF-8; free with
/// captures_settings_free_v1.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_motion_tables_v1() -> *mut c_char {
    let tables = serde_json::json!({
        "exits": preview_motion::exit_catalog(),
        "sparkles": preview_motion::sparkle_catalog(),
    });
    CString::new(tables.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Clear all's start delay for chronological `index` of `count` cards.
#[unsafe(no_mangle)]
pub extern "C" fn captures_preview_clear_delay_ms_v1(
    count: usize,
    index: usize,
    top_anchor: bool,
) -> f64 {
    preview_motion::clear_delay_ms(count, index, top_anchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};

    #[test]
    fn warnings_dust_and_motion_tables_cross_the_abi() {
        // SAFETY: Static and owned NUL-terminated strings returned above.
        unsafe {
            assert!(captures_preview_card_warning_v1(false, true, false).is_null());
            assert_eq!(
                CStr::from_ptr(captures_preview_card_warning_v1(false, false, true)).to_str(),
                Ok("Not in History")
            );
            assert_eq!(
                CStr::from_ptr(captures_preview_card_warning_v1(false, true, true)).to_str(),
                Ok("Clipboard unavailable")
            );
            assert!(captures_preview_card_warning_v1(true, false, true).is_null());
            assert!(
                captures_preview_dust_particles_v1(f64::NAN, 160., 1., 1., 0., 0., 1).is_null()
            );
            let dust = captures_preview_dust_particles_v1(284., 160., 800., 600., 22.5, 22.5, 3);
            let parsed: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(dust).to_str().unwrap()).unwrap();
            crate::captures_settings_free_v1(dust);
            assert_eq!(parsed.as_array().unwrap().len(), 198);
            assert!(parsed[0]["sourceLeft"].is_number() && parsed[0]["delayMs"].is_number());
            let tables = captures_preview_motion_tables_v1();
            let parsed: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(tables).to_str().unwrap()).unwrap();
            crate::captures_settings_free_v1(tables);
            assert_eq!(parsed["exits"]["dismiss"]["hold_ms"], 1_030.);
            assert_eq!(parsed["sparkles"]["early"][0]["accent"], true);
        }
        assert_eq!(captures_preview_clear_delay_ms_v1(3, 0, false), 72.);
    }

    #[test]
    fn overflow_cues_cross_the_abi() {
        assert_eq!(captures_preview_overflow_v1(0., 500., 600.), 0);
        assert_eq!(
            captures_preview_overflow_v1(0., 1_000., 600.),
            CAPTURES_PREVIEW_OVERFLOW_BELOW
        );
        assert_eq!(
            captures_preview_overflow_v1(200., 1_000., 600.),
            CAPTURES_PREVIEW_OVERFLOW_ABOVE | CAPTURES_PREVIEW_OVERFLOW_BELOW
        );
        assert_eq!(
            captures_preview_scroll_target_v1(400., 1_000., 600., -1),
            216.
        );
        // SAFETY: The export returns static NUL-terminated UTF-8.
        unsafe {
            for (above, top, expected) in [
                (true, false, "Show older captures"),
                (false, false, "Show newer captures"),
                (true, true, "Show newer captures"),
                (false, true, "Show older captures"),
            ] {
                let label = captures_preview_overflow_label_v1(above, top);
                assert_eq!(CStr::from_ptr(label).to_str(), Ok(expected));
            }
        }
    }

    #[test]
    fn editor_presence_and_copy_cross_the_abi() {
        let mut presence = CapturesEditorPresence::default();
        // SAFETY: A local, aligned, exclusive value; null is documented as ignored.
        unsafe {
            assert!(!captures_preview_editor_presence_update_v1(
                std::ptr::null_mut(),
                true,
                0.,
                false
            ));
            assert!(captures_preview_editor_presence_update_v1(
                &mut presence,
                true,
                10.,
                false
            ));
            assert_eq!(presence.phase, CAPTURES_EDITOR_PHASE_PRESENT);
            assert_eq!(
                captures_preview_editor_presence_next_v1(presence, 20., false),
                -1.
            );
            assert!(captures_preview_editor_presence_update_v1(
                &mut presence,
                false,
                100.,
                false
            ));
            assert_eq!(presence.phase, CAPTURES_EDITOR_PHASE_LEAVING);
            assert_eq!(
                captures_preview_editor_presence_next_v1(presence, 200., false),
                450.
            );
            assert!(captures_preview_editor_presence_update_v1(
                &mut presence,
                false,
                650.,
                false
            ));
            assert_eq!(presence.phase, CAPTURES_EDITOR_PHASE_LINGERING);
            assert_eq!(presence.since_ms, 650.);
        }
        // SAFETY: The export returns static NUL-terminated UTF-8.
        unsafe {
            for (phase, hovered, just_opened, expected) in [
                (CAPTURES_EDITOR_PHASE_IDLE, true, false, "Edit"),
                (CAPTURES_EDITOR_PHASE_LINGERING, false, false, "Edit"),
                (CAPTURES_EDITOR_PHASE_PRESENT, false, false, "In editor"),
                (CAPTURES_EDITOR_PHASE_PRESENT, true, false, "Show in editor"),
                (CAPTURES_EDITOR_PHASE_PRESENT, true, true, "In editor"),
            ] {
                let label = captures_preview_editor_label_v1(phase, hovered, just_opened);
                assert_eq!(CStr::from_ptr(label).to_str(), Ok(expected));
            }
        }
        assert_eq!(captures_preview_editor_pill_width_v1(40., 60.), 94.);
    }

    #[test]
    fn hover_lock_tooltips_and_media_cross_the_abi() {
        let mut lock = CapturesCardHoverLock::default();
        // SAFETY: A local, aligned, exclusive value.
        unsafe {
            assert!(captures_preview_hover_lock_v1(
                &mut lock,
                CAPTURES_HOVER_LOCK_LOCK,
                0.,
                0.
            ));
            assert!(captures_preview_hover_lock_v1(
                &mut lock,
                CAPTURES_HOVER_LOCK_POINTER,
                30.,
                40.
            ));
            assert!(lock.has_origin && lock.origin_x == 30. && lock.origin_y == 40.);
            assert!(captures_preview_hover_lock_v1(
                &mut lock,
                CAPTURES_HOVER_LOCK_POINTER,
                32.,
                40.
            ));
            assert!(!captures_preview_hover_lock_v1(
                &mut lock,
                CAPTURES_HOVER_LOCK_POINTER,
                34.,
                40.
            ));
            assert_eq!(lock, CapturesCardHoverLock::default());
            captures_preview_hover_lock_v1(&mut lock, CAPTURES_HOVER_LOCK_LOCK, 0., 0.);
            assert!(!captures_preview_hover_lock_v1(
                &mut lock,
                CAPTURES_HOVER_LOCK_POINTER_OUTSIDE,
                0.,
                0.
            ));
        }
        let frame = captures_preview_icon_tooltip_frame_v1(
            crate::tray_notice::CapturesTrayNoticeRect {
                x: 8.,
                y: 8.,
                width: 28.,
                height: 28.,
            },
            30.,
            12.,
            false,
            1.,
        );
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (0., 42., 44., 20.)
        );
        let media = captures_preview_hover_media_v1();
        assert_eq!(
            (media.blur, media.brightness, media.scale),
            (2., 0.5, 1.015)
        );
    }

    #[test]
    fn card_metadata_crosses_the_abi_as_owned_utf8() {
        let value = captures_preview_card_metadata_v1(1440, 900, 245_760);
        assert!(!value.is_null());
        // SAFETY: The export returns an owned NUL-terminated string that is
        // read, then freed exactly once through the documented free function.
        unsafe {
            assert_eq!(CStr::from_ptr(value).to_str(), Ok("1440 × 900 · 246 KB"));
            crate::captures_settings_free_v1(value);
        }
    }

    #[test]
    fn stack_membership_poses_and_borrowed_utf8_cross_the_abi() {
        let stack = captures_preview_stack_new_v1();
        // SAFETY: Sequential access to one owned handle, valid C strings/local
        // outputs; borrowed bytes are copied before membership is mutated.
        unsafe {
            assert!(!captures_preview_stack_insert_v1(stack, null()));
            assert!(!captures_preview_stack_insert_v1(stack, c"\xff".as_ptr()));
            assert!(!captures_preview_stack_insert_v1(stack, c"".as_ptr()));
            assert!(captures_preview_stack_insert_v1(stack, c"古い".as_ptr()));
            assert!(captures_preview_stack_insert_v1(stack, c"new".as_ptr()));
            assert!(!captures_preview_stack_insert_v1(stack, c"古い".as_ptr()));
            assert_eq!(captures_preview_stack_count_v1(stack), 2);
            assert_eq!(captures_preview_stack_height_v1(stack), 424.);
            let mut id = CapturesPreviewID {
                data: null(),
                length: 0,
            };
            assert!(captures_preview_stack_id_v1(stack, 0, &mut id));
            assert_eq!(
                std::slice::from_raw_parts(id.data, id.length),
                "古い".as_bytes()
            );
            let copied = std::slice::from_raw_parts(id.data, id.length).to_vec();
            let mut card = CapturesPreviewCardLayout::default();
            assert!(captures_preview_stack_card_v1(stack, 0, true, &mut card));
            assert_eq!(
                card,
                CapturesPreviewCardLayout {
                    y: 236.,
                    depth: 1,
                    interactive: true
                }
            );
            assert!(captures_preview_stack_set_collapsed_v1(stack, true));
            assert!(captures_preview_stack_collapsed_v1(stack));
            assert!(captures_preview_stack_card_v1(stack, 0, false, &mut card));
            assert!(!card.interactive);
            let rest_y = card.y;
            assert!(captures_preview_stack_card_v2(
                stack, 0, false, true, &mut card
            ));
            assert!(card.y < rest_y);
            assert!(!card.interactive);
            assert!(captures_preview_stack_insert_v1(
                stack,
                c"incoming".as_ptr()
            ));
            assert!(captures_preview_stack_collapsed_v1(stack));
            assert!(captures_preview_stack_remove_v1(stack, c"古い".as_ptr()));
            assert!(captures_preview_stack_remove_v1(stack, c"new".as_ptr()));
            assert!(!captures_preview_stack_remove_v1(stack, c"new".as_ptr()));
            assert_eq!(copied, "古い".as_bytes());
            assert_eq!(captures_preview_stack_count_v1(stack), 1);
            assert!(captures_preview_stack_remove_v1(
                stack,
                c"incoming".as_ptr()
            ));
            assert!(!captures_preview_stack_collapsed_v1(stack));
            captures_preview_stack_free_v1(stack);
        }
    }

    #[test]
    fn invalid_stack_queries_preserve_outputs() {
        let stack = captures_preview_stack_new_v1();
        let sentinel = CapturesPreviewCardLayout {
            y: 999.,
            depth: 7,
            interactive: true,
        };
        let mut card = sentinel;
        let mut id = CapturesPreviewID {
            data: null(),
            length: 42,
        };
        // SAFETY: Nulls are explicitly permitted; locals and owned handle valid.
        unsafe {
            assert!(!captures_preview_stack_card_v1(stack, 0, false, &mut card));
            assert!(!captures_preview_stack_card_v1(null(), 0, false, &mut card));
            assert!(!captures_preview_stack_card_v1(stack, 0, false, null_mut()));
            assert_eq!(card, sentinel);
            assert!(!captures_preview_stack_id_v1(stack, 0, &mut id));
            assert!(!captures_preview_stack_id_v1(null(), 0, &mut id));
            assert_eq!(id.length, 42);
            assert!(id.data.is_null());
            assert!(!captures_preview_stack_set_collapsed_v1(null_mut(), true));
            assert!(!captures_preview_stack_collapsed_v1(null()));
            assert_eq!(captures_preview_stack_count_v1(null()), 0);
            captures_preview_stack_free_v1(stack);
            captures_preview_stack_free_v1(null_mut());
        }
    }

    fn monitor() -> CapturesPreviewMonitor {
        CapturesPreviewMonitor {
            work_x: -2400,
            work_y: 120,
            work_width: 2400,
            work_height: 1500,
            full_x: -2400,
            full_y: 40,
            full_width: 2400,
            full_height: 1660,
            scale_factor: 2.,
        }
    }

    #[test]
    fn asymmetric_monitor_corners_and_dragged_origin_cross_the_abi() {
        let mut out = CapturesPreviewGeometry::default();
        // Independent expectations: work rect (-1200,60,1200,750), 12 gap, 240 high.
        for (placement, x, y, anchor) in [
            (0, -1200., 558., 0),
            (1, -340., 558., 0),
            (2, -1200., 72., 1),
            (3, -340., 72., 1),
        ] {
            // SAFETY: Output is local aligned writable storage; origin is null.
            assert!(unsafe {
                captures_preview_geometry_v1(monitor(), 1, false, null(), placement, &mut out)
            });
            assert_eq!(
                out,
                CapturesPreviewGeometry {
                    x,
                    y,
                    width: 340.,
                    height: 240.,
                    card_height: 160.,
                    padding: 28.,
                    control_gutter: 52.,
                    anchor
                }
            );
        }
        let origin = CapturesPreviewOrigin {
            x: -700.,
            edge: 480.,
            anchor: 0,
        };
        // SAFETY: Both pointers refer to valid local values.
        assert!(unsafe { captures_preview_geometry_v1(monitor(), 1, false, &origin, 2, &mut out) });
        assert_eq!((out.x, out.y, out.anchor), (-700., 240., 0));
        for (collapsed, height, anchor) in [(false, 424., 1), (true, 264., 0)] {
            // SAFETY: Output is valid, origin is null. Two cards distinguish
            // count forwarding; a collapsed frame has its own bottom anchor.
            assert!(unsafe {
                captures_preview_geometry_v1(monitor(), 2, collapsed, null(), 3, &mut out)
            });
            assert_eq!(
                (out.x, out.y, out.height, out.anchor),
                (-340., 72., height, anchor)
            );
        }
        // Same rect with no reserved bottom triggers the shared auto-hide reserve (48).
        let monitor = CapturesPreviewMonitor {
            full_height: 1580,
            ..monitor()
        };
        // SAFETY: Output is valid, origin is null.
        assert!(unsafe { captures_preview_geometry_v1(monitor, 1, false, null(), 0, &mut out) });
        assert_eq!(out.y, 510.);
    }

    #[test]
    fn invalid_geometry_leaves_output_unchanged() {
        let sentinel = CapturesPreviewGeometry {
            x: 917.,
            ..Default::default()
        };
        let mut out = sentinel;
        for monitor in [
            CapturesPreviewMonitor {
                scale_factor: f64::NAN,
                ..monitor()
            },
            CapturesPreviewMonitor {
                scale_factor: 0.,
                ..monitor()
            },
            CapturesPreviewMonitor {
                work_height: 0,
                ..monitor()
            },
        ] {
            // SAFETY: Local output is writable and origin is null.
            assert!(!unsafe {
                captures_preview_geometry_v1(monitor, 1, false, null(), 0, &mut out)
            });
            assert_eq!(out, sentinel);
        }
        for origin in [
            CapturesPreviewOrigin {
                x: 1.,
                edge: 2.,
                anchor: 2,
            },
            CapturesPreviewOrigin {
                x: f64::INFINITY,
                edge: 2.,
                anchor: 0,
            },
        ] {
            // SAFETY: Both pointers are valid locals.
            assert!(!unsafe {
                captures_preview_geometry_v1(monitor(), 1, false, &origin, 0, &mut out)
            });
            assert_eq!(out, sentinel);
        }
        // SAFETY: Null output is rejected; other output is valid local storage.
        unsafe {
            assert!(!captures_preview_geometry_v1(
                monitor(),
                1,
                false,
                null(),
                0,
                null_mut()
            ));
            assert!(!captures_preview_geometry_v1(
                monitor(),
                1,
                false,
                null(),
                4,
                &mut out
            ));
        }
        assert_eq!(out, sentinel);
    }

    #[test]
    fn stale_artifacts_and_cancellation_cannot_restore_a_newer_capture() {
        let handle = captures_preview_visibility_new_v1();
        // SAFETY: A single owner uses the live handle sequentially and frees once;
        // string literals and output locals remain valid through every call.
        unsafe {
            let mut first = 99;
            assert!(!captures_preview_begin_v1(handle, null_mut()));
            assert!(captures_preview_visible_v1(handle, 1, true, false));
            assert!(captures_preview_begin_v1(handle, &mut first));
            assert_eq!(first, 1);
            let mut second = 123;
            assert!(!captures_preview_begin_v1(handle, &mut second));
            assert_eq!(second, 123);
            assert!(!captures_preview_visible_v1(handle, 1, true, false));
            assert!(captures_preview_visible_v1(handle, 1, true, true));
            assert!(!captures_preview_visible_v1(handle, 1, false, true));
            assert!(!captures_preview_visible_v1(handle, 0, true, true));
            assert!(!captures_preview_wait_v1(handle, first, null()));
            assert!(!captures_preview_wait_v1(handle, first, c"\xff".as_ptr()));
            assert!(captures_preview_wait_v1(handle, first, c"old".as_ptr()));
            assert!(captures_preview_begin_v1(handle, &mut second));
            assert!(!captures_preview_restore_v1(handle, first));
            assert!(!captures_preview_wait_v1(handle, first, c"old".as_ptr()));
            assert!(!captures_preview_ready_v1(handle, c"old".as_ptr()));
            assert!(!captures_preview_stop_waiting_v1(handle));
            assert!(captures_preview_wait_v1(handle, second, c"new".as_ptr()));
            assert!(captures_preview_capture_ui_v1(handle, true));
            assert!(captures_preview_ready_v1(handle, c"new".as_ptr()));
            assert!(!captures_preview_visible_v1(handle, 1, true, false));
            assert!(captures_preview_capture_ui_v1(handle, false));
            assert!(captures_preview_visible_v1(handle, 1, true, false));
            assert!(captures_preview_begin_v1(handle, &mut second));
            assert!(captures_preview_restore_v1(handle, second));
            assert!(!captures_preview_restore_v1(handle, second));
            assert!(captures_preview_begin_v1(handle, &mut second));
            assert!(captures_preview_wait_v1(
                handle,
                second,
                c"bad-decode".as_ptr()
            ));
            assert!(captures_preview_stop_waiting_v1(handle));
            assert!(!captures_preview_ready_v1(handle, c"bad-decode".as_ptr()));
            assert!(captures_preview_visible_v1(handle, 1, true, false));
            captures_preview_visibility_free_v1(handle);
            captures_preview_visibility_free_v1(null_mut());
            assert!(!captures_preview_visible_v1(null(), 1, true, true));
            assert!(!captures_preview_begin_v1(null_mut(), &mut second));
        }
    }
}
