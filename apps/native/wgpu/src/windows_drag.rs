//! Bounded Windows outbound file dragging.
//!
//! This adapter intentionally uses exactly `drag` 2.1.1 rather than owning
//! unsafe OLE code. In that release the Windows backend only checks that its
//! argument has a Win32 window handle; it does not otherwise use the HWND.
//! Consequently the caller may pass the retained root `Arc<Window>`'s
//! `&Window` while a child preview initiated the drag, provided this function
//! is called on the GUI/STA thread after eframe has released its window borrow.

use std::{path::PathBuf, sync::Mutex};

use winit::{dpi::PhysicalPosition, window::Window};

/// The one-shot completion delivered when the native drag loop exits.
///
/// The boolean is true only when a target accepted the offered COPY. The
/// position is the final physical screen cursor position reported by Win32.
/// Because `drag::start_drag` runs a nested synchronous event loop and invokes
/// this callback before returning, the callback must only enqueue this value;
/// it must not directly mutate application UI state.
pub type Completion = Box<dyn FnOnce(bool, PhysicalPosition<i32>) + Send + 'static>;

/// Starts a synchronous, COPY-only Windows file drag.
///
/// `drag_image_png` must contain an already-loaded, encoded PNG suitable for a
/// small product drag icon. It is only the drag image: `original_file` is the
/// sole transferred file. Supplying bytes keeps filesystem access and image
/// preparation out of this GUI-thread adapter.
pub fn start(
    window: &Window,
    original_file: PathBuf,
    drag_image_png: Vec<u8>,
    completion: Completion,
) -> Result<(), String> {
    if drag_image_png.is_empty() {
        return Err("Windows drag image PNG is empty".to_owned());
    }

    // drag 2.1.1 requires `Fn`, although its Windows backend calls it once.
    // Interior ownership adapts that API without weakening our one-shot contract.
    let completion = Mutex::new(Some(completion));
    drag::start_drag(
        window,
        drag::DragItem::Files(vec![original_file]),
        drag::Image::Raw(drag_image_png),
        move |result, cursor| {
            let Some(completion) = completion
                .lock()
                .expect("drag completion mutex poisoned")
                .take()
            else {
                return;
            };
            completion(
                matches!(result, drag::DragResult::Dropped),
                PhysicalPosition::new(cursor.x, cursor.y),
            );
        },
        drag::Options {
            mode: drag::DragMode::Copy,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())
}
