//! Real AppKit geometry regression, not GPUI's simulated test platform.
//! Run in a logged-in macOS graphical session; backing scale may be 1× or 2×.
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macos_popup_geometry: skipped (requires macOS AppKit)");
}

#[cfg(target_os = "macos")]
fn main() {
    use gpui::*;
    use std::{cell::Cell, rc::Rc, time::Duration};

    struct Surface;
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0x20242b))
        }
    }

    let completed = Rc::new(Cell::new(false));
    let result = completed.clone();
    Application::new().run(move |cx| {
        let requested = size(px(640.), px(720.));
        let handle = cx
            .open_window(
                WindowOptions {
                    // Odd display dimensions can center the adapter at half points.
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(100.5), px(198.5)),
                        requested,
                    ))),
                    titlebar: None,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Opaque,
                    ..Default::default()
                },
                |window, cx| {
                    check_geometry(window, requested);
                    cx.new(|_| Surface)
                },
            )
            .expect("open native popup geometry regression window");
        cx.spawn(async move |cx| {
            // Check after AppKit layout, then exercise the same content-size
            // contract through resize and restoration. No one-point tolerance.
            for (index, expected) in [requested, size(px(641.), px(719.)), requested]
                .into_iter()
                .enumerate()
            {
                if index > 0 {
                    handle
                        .update(cx, |_, window, _| window.resize(expected))
                        .unwrap();
                }
                Timer::after(Duration::from_millis(100)).await;
                handle
                    .update(cx, |_, window, _| check_geometry(window, expected))
                    .unwrap();
            }
            result.set(true);
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    assert!(completed.get(), "native geometry checks did not complete");
    println!("macos_popup_geometry: creation, AppKit layout, resize and restore passed");
}

#[cfg(target_os = "macos")]
fn check_geometry(window: &gpui::Window, expected: gpui::Size<gpui::Pixels>) {
    use objc2_app_kit::{NSView, NSWindowStyleMask};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    assert_eq!(window.viewport_size(), expected, "GPUI viewport");
    let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
        panic!("expected AppKit window");
    };
    // The live GPUI window owns this NSView; checks run on AppKit's main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let native = view.window().expect("NSView has a window");
    assert_eq!(native.styleMask(), NSWindowStyleMask::NonactivatingPanel);
    let content = native.contentView().expect("NSWindow has a content view");
    let width = f64::from(f32::from(expected.width));
    let height = f64::from(f32::from(expected.height));
    for (name, actual) in [
        ("NSWindow frame", native.frame().size),
        ("content view", content.frame().size),
        ("GPUI NSView", view.frame().size),
    ] {
        assert_eq!((actual.width, actual.height), (width, height), "{name}");
    }
    let backing = view.convertRectToBacking(view.bounds()).size;
    let scale = f64::from(window.scale_factor());
    assert_eq!(
        (backing.width, backing.height),
        (width * scale, height * scale),
        "native backing size"
    );
}
