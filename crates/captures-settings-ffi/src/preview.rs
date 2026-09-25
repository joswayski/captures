//! Native hosts borrow the shipping placement and capture-visibility policy.
use captures_app::preview::{
    self, ThumbnailMonitorBounds, ThumbnailStackAnchor, ThumbnailStackOrigin, ThumbnailVisibility,
};
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};

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
