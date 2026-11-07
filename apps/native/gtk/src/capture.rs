//! Display screenshots in the live app, through the shared capture flow:
//! `CaptureFlow` and `Request::CaptureDisplay` on X11 (the display under
//! the pointer, Escape cancels), and the desktop portal's Screenshot on
//! Wayland. As shipping, the preview stack leaves the screen first unless
//! it is included in captures, the configured countdown runs, and a
//! saved capture joins History, the preview stack and, when enabled, the
//! clipboard.

use std::rc::Rc;
use std::time::{Duration, Instant};

use captures_app::capture_flow::{CaptureFlow, Countdown};
use captures_capture::CaptureMode;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{Host, emit};

/// Shipping `CAPTURE_HUD_HIDE_SETTLE_MS`: a hidden surface's last frame
/// leaves the compositor before the screen is read.
pub(crate) const SETTLE: Duration = Duration::from_millis(40);

/// Owns the one capture in flight; dropping it ends the flow.
pub enum Flow {
    X11(CaptureFlow),
    Portal(captures_app::capture_flow::PortalCapture),
}

impl Flow {
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Flow::X11(flow) => flow.generation(),
            Flow::Portal(flow) => flow.generation(),
        }
    }

    pub(crate) fn cancel(&self) {
        match self {
            Flow::X11(flow) => flow.cancel(),
            Flow::Portal(flow) => flow.cancel(),
        }
    }
}

pub fn wayland(host: &Host) -> bool {
    gdk::Display::default().is_some_and(|display| display.type_().name() == "GdkWaylandDisplay")
        || host
            .app
            .windows()
            .first()
            .is_some_and(|window| WidgetExt::display(window).type_().name() == "GdkWaylandDisplay")
}

impl Host {
    /// Display on X11 or through the Screenshot portal; on Wayland also
    /// Window, through a window-only ScreenCast grant (the portal's own
    /// picker replaces the window overlay there).
    pub(crate) fn capture_now(self: &Rc<Self>, mode: CaptureMode) {
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let seconds = settings.screenshot_countdown_seconds;
        let portal = wayland(self);
        let flow = if portal {
            captures_app::capture_flow::PortalCapture::begin().map(Flow::Portal)
        } else {
            CaptureFlow::begin(seconds).map(Flow::X11)
        };
        let flow = match flow {
            Ok(flow) => flow,
            Err(error) => return self.capture_failed(&error),
        };
        let generation = flow.generation();
        *self.capture.borrow_mut() = Some(flow);
        // Hide the stack unless it belongs in captures; the portal path
        // unmaps every Captures window, as the wgpu host, so consent and
        // the still never include them.
        let hidden: Vec<gtk::Window> = self
            .app
            .windows()
            .into_iter()
            .filter(|window| window.is_visible())
            .filter(|window| {
                portal
                    || (!settings.include_mini_previews_in_captures
                        && self.previews.borrow().as_ref().is_some_and(|previews| {
                            previews.window.upcast_ref::<gtk::Window>() == window
                        }))
            })
            .collect();
        for window in &hidden {
            window.set_visible(false);
        }
        let countdown = Countdown::new(Instant::now(), seconds);
        let started = Instant::now();
        let host = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            // The countdown is a monotonic deadline (no GTK overlay yet).
            while countdown.remaining(Instant::now()) > 0 {
                glib::timeout_future(Duration::from_millis(50)).await;
                if !captures_app::capture_flow::is_current(generation) {
                    break;
                }
            }
            if !hidden.is_empty() {
                glib::timeout_future(SETTLE).await;
            }
            let Some(root) = host.upgrade().map(|host| host.history_root.clone()) else {
                return;
            };
            let include_cursor = settings.show_cursor_in_screenshots;
            let result = gio::spawn_blocking(move || {
                capture(root, generation, portal, mode, include_cursor)
            })
            .await
            .unwrap_or_else(|_| Err("the capture worker stopped".into()));
            for window in &hidden {
                window.set_visible(true);
            }
            let Some(host) = host.upgrade() else { return };
            host.capture.borrow_mut().take();
            match result {
                Ok(Some(artifact)) => {
                    host.captured(artifact, &settings, portal, started);
                }
                Ok(None) => emit("capture-cancelled", serde_json::json!({"portal": portal})),
                Err(error) => host.capture_failed(&error),
            }
        });
    }

    /// A saved capture: History reloads, the newest card joins the preview
    /// stack, and auto-copy puts it on the clipboard.
    pub(crate) fn captured(
        self: &Rc<Self>,
        artifact: captures_app::Artifact,
        settings: &captures_settings::AppSettings,
        portal: bool,
        started: Instant,
    ) {
        emit(
            "captured",
            serde_json::json!({"id": artifact.entry.id, "mode": artifact.entry.mode,
            "portal": portal, "ms": started.elapsed().as_secs_f64() * 1000.}),
        );
        if let Some(history) = self.history.borrow().as_ref() {
            history.reload();
        }
        let id = artifact.entry.id.clone();
        let previews = settings
            .show_mini_previews
            .then(|| self.show_preview(artifact.entry));
        if settings.auto_copy_to_clipboard {
            let path = artifact.image_path;
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || copy_image(&path))
                    .await
                    .unwrap_or_else(|_| Err("the copy worker stopped".into()));
                emit("auto-copy", serde_json::json!({"ok": result.is_ok()}));
                if let Some(previews) = previews {
                    previews.copied(&id, result);
                }
            });
        }
    }

    pub(crate) fn capture_failed(&self, error: &str) {
        emit("capture-failed", serde_json::json!({"error": error}));
        self.show_error(error);
    }
}

/// Auto-copy goes through arboard, as shipping and the wgpu host: on
/// Wayland its wlr-data-control path needs no input serial, which a capture
/// started from a shortcut or the tray never gives GDK. The clipboard stays
/// alive so the selection keeps being served.
fn copy_image(path: &std::path::Path) -> Result<(), String> {
    static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgba8();
    let mut clipboard = CLIPBOARD
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    clipboard
        .as_mut()
        .expect("clipboard initialized")
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Borrowed(image.as_raw()),
        })
        .map_err(|e| format!("Could not copy image: {e}"))
}

fn capture(
    root: std::path::PathBuf,
    generation: u64,
    portal: bool,
    mode: CaptureMode,
    include_cursor: bool,
) -> Result<Option<captures_app::Artifact>, String> {
    if portal {
        return captures_app::capture_portal_screenshot(&root, generation, mode, include_cursor)
            .map_err(|error| error.to_string());
    }
    let display_id = pointer_display()?;
    match captures_app::execute(captures_app::Request::CaptureDisplay {
        root,
        display_id,
        generation,
        include_cursor,
    }) {
        Ok(captures_app::Response::Captured { artifact }) => Ok(Some(artifact)),
        Ok(_) => unreachable!("CaptureDisplay returns Captured"),
        Err(captures_app::Error::Cancelled) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Shipping starts on the display under the pointer, looked up fresh, else
/// the primary display.
pub(crate) fn pointer_display() -> Result<String, String> {
    let backend = captures_capture::XcapBackend;
    captures_capture::pointer_position()
        .and_then(|point| backend.display_id_at_point(point))
        .or_else(|| {
            let displays = backend.displays().ok()?;
            displays
                .iter()
                .find(|display| display.is_primary)
                .or(displays.first())
                .map(|display| display.id.clone())
        })
        .ok_or_else(|| "No display is available for capture.".into())
}
