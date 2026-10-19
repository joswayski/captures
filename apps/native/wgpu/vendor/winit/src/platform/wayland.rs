//! # Wayland
//!
//! **Note:** Windows don't appear on Wayland until you draw/present to them.
//!
//! By default, Winit loads system libraries using `dlopen`. This can be
//! disabled by disabling the `"wayland-dlopen"` cargo feature.
//!
//! ## Client-side decorations
//!
//! Winit provides client-side decorations by default, but the behaviour can
//! be controlled with the following feature flags:
//!
//! * `wayland-csd-adwaita` (default).
//! * `wayland-csd-adwaita-crossfont`.
//! * `wayland-csd-adwaita-notitle`.

use std::ffi::c_void;
use std::ptr::NonNull;

use crate::event_loop::{ActiveEventLoop, EventLoop, EventLoopBuilder};
use crate::monitor::MonitorHandle;
use crate::window::{Window, WindowAttributes};

pub use crate::window::Theme;

/// A complete COPY drop with the position frozen when the button was released.
#[derive(Debug)]
pub struct FileDropBatch {
    /// Local paths in source order.
    pub paths: Vec<std::path::PathBuf>,
    /// Surface coordinates, independent of output-scale changes during transfer.
    pub position: crate::dpi::LogicalPosition<f64>,
}

/// Image/file formats advertised by the focused seat's clipboard selection.
#[derive(Debug, Default)]
pub struct ClipboardOffer {
    /// Whether a PNG is advertised; reading its pixels still uses data-control.
    pub has_png: bool,
    /// Advertised local-file transfer, to be read with bounds off the event loop.
    pub uri_list: Option<std::os::fd::OwnedFd>,
}

/// Additional methods on [`ActiveEventLoop`] that are specific to Wayland.
pub trait ActiveEventLoopExtWayland {
    /// True if the [`ActiveEventLoop`] uses Wayland.
    fn is_wayland(&self) -> bool;

    /// Offer one private clipboard MIME from the focused seat's last input.
    /// `owned` starts false and is set only by the compositor's matching
    /// selection offer; replacement/cancellation invalidates it. Keep this
    /// flag alive while the retained application payload remains available.
    fn set_clipboard_marker(
        &self,
        mime: String,
        owned: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), &'static str>;

    /// Inspect advertised image/file formats and request an offered URI list.
    /// The caller owns the pipe and must bound reads off the native event loop.
    fn clipboard_offer(&self) -> Result<ClipboardOffer, &'static str>;

    /// Start a COPY-only URI-list drag from a currently pressed source window.
    /// Completion reports COPY acceptance, whether the destination belongs to
    /// this client, and whether it is exactly the source window. The source file
    /// must remain available afterward.
    fn start_file_drag(
        &self,
        source: crate::window::WindowId,
        path: std::path::PathBuf,
        finished: Box<dyn FnOnce(bool, bool, bool) + Send>,
    ) -> Result<(), &'static str>;
}

impl ActiveEventLoopExtWayland for ActiveEventLoop {
    #[inline]
    fn is_wayland(&self) -> bool {
        self.p.is_wayland()
    }

    fn set_clipboard_marker(
        &self,
        mime: String,
        owned: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), &'static str> {
        self.p.set_wayland_clipboard_marker(mime, owned)
    }

    fn clipboard_offer(&self) -> Result<ClipboardOffer, &'static str> {
        self.p.wayland_clipboard_offer()
    }

    fn start_file_drag(
        &self,
        source: crate::window::WindowId,
        path: std::path::PathBuf,
        finished: Box<dyn FnOnce(bool, bool, bool) + Send>,
    ) -> Result<(), &'static str> {
        self.p.start_wayland_file_drag(source, path, finished)
    }
}

/// Additional methods on [`EventLoop`] that are specific to Wayland.
pub trait EventLoopExtWayland {
    /// True if the [`EventLoop`] uses Wayland.
    fn is_wayland(&self) -> bool;
}

impl<T: 'static> EventLoopExtWayland for EventLoop<T> {
    #[inline]
    fn is_wayland(&self) -> bool {
        self.event_loop.is_wayland()
    }
}

/// Additional methods on [`EventLoopBuilder`] that are specific to Wayland.
pub trait EventLoopBuilderExtWayland {
    /// Force using Wayland.
    fn with_wayland(&mut self) -> &mut Self;

    /// Whether to allow the event loop to be created off of the main thread.
    ///
    /// By default, the window is only allowed to be created on the main
    /// thread, to make platform compatibility easier.
    fn with_any_thread(&mut self, any_thread: bool) -> &mut Self;
}

impl<T> EventLoopBuilderExtWayland for EventLoopBuilder<T> {
    #[inline]
    fn with_wayland(&mut self) -> &mut Self {
        self.platform_specific.forced_backend = Some(crate::platform_impl::Backend::Wayland);
        self
    }

    #[inline]
    fn with_any_thread(&mut self, any_thread: bool) -> &mut Self {
        self.platform_specific.any_thread = any_thread;
        self
    }
}

/// Additional methods on [`Window`] that are specific to Wayland.
///
/// [`Window`]: crate::window::Window
pub trait WindowExtWayland {
    /// Returns `xdg_toplevel` of the window or [`None`] if the window is X11 window.
    fn xdg_toplevel(&self) -> Option<NonNull<c_void>>;

    /// Whether a Wayland surface may accept rendered buffers. False as soon as
    /// hiding is requested, while `is_visible` waits for acknowledged unmapping.
    /// Returns None for an X11 window.
    fn is_surface_ready(&self) -> Option<bool>;

    /// Drain complete drops instead of grouping individual `DroppedFile` events.
    /// Returns None on X11. Consume on `DroppedFile`; all per-file events still fire.
    fn take_file_drop_batches(&self) -> Option<Vec<FileDropBatch>>;
}

impl WindowExtWayland for Window {
    #[inline]
    fn xdg_toplevel(&self) -> Option<NonNull<c_void>> {
        #[allow(clippy::single_match)]
        match &self.window {
            #[cfg(x11_platform)]
            crate::platform_impl::Window::X(_) => None,
            #[cfg(wayland_platform)]
            crate::platform_impl::Window::Wayland(window) => window.xdg_toplevel(),
        }
    }

    fn is_surface_ready(&self) -> Option<bool> {
        match &self.window {
            #[cfg(x11_platform)]
            crate::platform_impl::Window::X(_) => None,
            #[cfg(wayland_platform)]
            crate::platform_impl::Window::Wayland(window) => Some(window.is_surface_ready()),
        }
    }

    fn take_file_drop_batches(&self) -> Option<Vec<FileDropBatch>> {
        match &self.window {
            #[cfg(x11_platform)]
            crate::platform_impl::Window::X(_) => None,
            #[cfg(wayland_platform)]
            crate::platform_impl::Window::Wayland(window) => Some(window.take_file_drop_batches()),
        }
    }
}

/// Additional methods on [`WindowAttributes`] that are specific to Wayland.
pub trait WindowAttributesExtWayland {
    /// Build window with the given name.
    ///
    /// The `general` name sets an application ID, which should match the `.desktop`
    /// file distributed with your program. The `instance` is a `no-op`.
    ///
    /// For details about application ID conventions, see the
    /// [Desktop Entry Spec](https://specifications.freedesktop.org/desktop-entry-spec/desktop-entry-spec-latest.html#desktop-file-id)
    fn with_name(self, general: impl Into<String>, instance: impl Into<String>) -> Self;
}

impl WindowAttributesExtWayland for WindowAttributes {
    #[inline]
    fn with_name(mut self, general: impl Into<String>, instance: impl Into<String>) -> Self {
        self.platform_specific.name = Some(crate::platform_impl::ApplicationName::new(
            general.into(),
            instance.into(),
        ));
        self
    }
}

/// Additional methods on `MonitorHandle` that are specific to Wayland.
pub trait MonitorHandleExtWayland {
    /// Returns the inner identifier of the monitor.
    fn native_id(&self) -> u32;
}

impl MonitorHandleExtWayland for MonitorHandle {
    #[inline]
    fn native_id(&self) -> u32 {
        self.inner.native_identifier()
    }
}
