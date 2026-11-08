//! Shipping's direct Region and Window overlays (`CaptureOverlay`, the
//! `.capture-surface`, `.selection-box` and `.window-target` rules) over
//! one display.
//!
//! X11: a shared `CaptureFlow` owns Escape, `RegionSession` or
//! `WindowSession` prepares the (optionally frozen) display off the main
//! thread, and the overlay is an unmanaged window covering the display.
//! Wayland: Region takes the Screenshot portal's still and selects over it
//! on a full-output layer surface (or a fullscreen window), then
//! `persist_portal_region` crops it; Window goes straight to the portal's
//! own window picker (`Host::capture_now`).
//!
//! The frozen desktop is one texture uploaded once; pointer motion only
//! re-records the cheap shade, marquee and badge nodes over it. The
//! guidance chip is a real widget, so screen readers hear its copy.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use captures_app::capture_flow::{self, CaptureFlow, PortalCapture};
use captures_app::capture_menu::{self, GuidanceTarget};
use captures_app::region::RegionSession;
use captures_app::selection::{self, Bounds, DragMode, DragOptions, Point, Rect};
use captures_app::window::{Target, WindowSession};
use captures_capture::{CaptureMode, WindowDescriptor};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk, pango};

use crate::capture::{Flow, SETTLE, pointer_display, wayland};
use crate::capture_bar::{self, Bar, Target as MenuTarget};
use crate::tokens::Tokens;
use crate::widgets::SnapshotArea;
use crate::{Host, emit};

/// Shipping's region minimum while dragging (`MINIMUM_SIZE`).
const MINIMUM_SIZE: f64 = 16.;
pub const REGION_TITLE: &str = "Captures Region Selector";
pub const WINDOW_TITLE: &str = "Captures Window Selector";
pub const MENU_TITLE: &str = "Captures New Capture";
/// Shipping `.recording-selector` shades: region and its full veil, the
/// window cutout, and Full screen.
const MENU_SHADE: f32 = 0.38;
const MENU_SHADE_WINDOW: f32 = 0.3;
const MENU_SHADE_DISPLAY: f32 = 0.5;
/// `.recording-selection-frame .handle`: 10 px, centred 1 px outside.
const HANDLE: f32 = 10.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Region,
    Window,
    /// New Capture: the bar picks Region, Window or Full screen.
    Menu,
}

/// What the overlay selects over and later captures from.
enum Source {
    Region(Box<RegionSession>),
    Window(Arc<WindowSession>),
    /// New Capture on X11: both sessions over one display, and the
    /// displays the bar can switch to.
    /// `window` is absent where the session lists no windows (an X11
    /// session without EWMH): Window is then unavailable, as shipping.
    Menu {
        region: Box<RegionSession>,
        window: Option<Arc<WindowSession>>,
        displays: Vec<(String, String)>,
    },
    /// A portal still in its own pixels; selection is in overlay points.
    Portal(Arc<image::RgbaImage>),
}

/// What a confirmed overlay captures.
enum Choice {
    Region(Rect),
    Window(Target),
    Display,
}

/// The overlay's live selection state.
#[derive(Default)]
struct State {
    /// Region: the selection in overlay points.
    rect: Option<Rect>,
    drag: Option<(DragMode, Point, Rect)>,
    /// Region: empty clicks while "Click and drag" feedback lasts.
    feedback: u32,
    /// Window: the hovered target; `None` window index is the display.
    hovered: Option<Option<usize>>,
    /// New Capture: the clicked window, kept until confirmed.
    selected: Option<Option<usize>>,
    /// New Capture: a start or display switch is in flight.
    primary: capture_menu::PrimaryState,
}

struct Overlay {
    window: gtk::ApplicationWindow,
    kind: Kind,
    source: RefCell<Option<Source>>,
    state: RefCell<State>,
    area: SnapshotArea,
    root: gtk::Overlay,
    chip: gtk::Box,
    chip_title: gtk::Label,
    chip_hint: gtk::Label,
    chip_over: Cell<bool>,
    /// Overlay points per display (X11) or still (portal) pixel.
    bounds: Cell<(f64, f64)>,
    done: Cell<bool>,
    portal: bool,
    hidden: Vec<gtk::Window>,
    settings: captures_settings::AppSettings,
    started: Instant,
    texture: Option<gdk::Texture>,
    tokens: Rc<Tokens>,
    host: std::rc::Weak<Host>,
    /// The height the guidance chip's 16% offset was laid out for.
    laid_out: Cell<i32>,
    /// New Capture: the bar, its target and region aspect lock.
    bar: Option<Bar>,
    target: Cell<MenuTarget>,
    aspect: Cell<Option<f64>>,
    display_card: Option<gtk::Box>,
}

impl Host {
    /// Screenshot Region (the direct overlay).
    pub fn capture_region(self: &Rc<Self>) {
        if !self.retarget_new_capture(MenuTarget::Region, Kind::Region) {
            self.select(Kind::Region);
        }
    }

    /// Screenshot Window: the direct overlay on X11, the portal's window
    /// picker on Wayland.
    pub fn capture_window(self: &Rc<Self>) {
        if self.retarget_new_capture(MenuTarget::Window, Kind::Window) {
            return;
        }
        if wayland(self) {
            self.capture_now(CaptureMode::Window);
        } else {
            self.select(Kind::Window);
        }
    }

    /// New Capture (Screenshot mode): the capture bar over the display
    /// under the pointer, or over the portal's still.
    pub fn new_capture(self: &Rc<Self>) {
        if self.restore_recording_controls() {
            return;
        }
        if !self.retarget_new_capture(MenuTarget::Region, Kind::Menu) {
            self.select_on(Kind::Menu, None, MenuTarget::Region);
        }
    }

    /// Shipping's Display entry point opens the bar on Full screen.
    pub fn capture_display(self: &Rc<Self>) {
        if !self.retarget_new_capture(MenuTarget::Display, Kind::Menu) {
            self.select_on(Kind::Menu, None, MenuTarget::Display);
        }
    }

    /// Shipping's screenshot shortcuts while New Capture is open: switch its
    /// target in place (Region and Full screen drop a clicked window).
    /// False when no New Capture is open, so the shortcut runs as usual.
    fn retarget_new_capture(self: &Rc<Self>, target: MenuTarget, repeat: Kind) -> bool {
        let Some(overlay) = self
            .selector
            .borrow()
            .as_ref()
            .map(|handle| handle.0.clone())
        else {
            return false;
        };
        if overlay.kind != Kind::Menu {
            return false;
        }
        let window_available = overlay
            .bar
            .as_ref()
            .is_some_and(|bar| bar.targets[1].is_sensitive());
        if overlay.done.get()
            || overlay.state.borrow().primary.starting
            || (target == MenuTarget::Window && !window_available)
        {
            return true;
        }
        if overlay.target.get() == target {
            overlay.recapture(repeat);
            return true;
        }
        if target != MenuTarget::Window {
            overlay.state.borrow_mut().selected = None;
        }
        overlay.retarget(target);
        true
    }

    fn select(self: &Rc<Self>, kind: Kind) {
        self.select_on(kind, None, MenuTarget::Region);
    }

    /// `display`: the display New Capture switches to.
    fn select_on(self: &Rc<Self>, kind: Kind, display: Option<String>, target: MenuTarget) {
        // An exiting countdown still owns hidden windows and cleanup even
        // after Escape has released its shared generation.
        if self.capture.borrow().is_some() {
            return self.capture_failed("A capture is already in progress");
        }
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let portal = wayland(self);
        let flow = if portal {
            PortalCapture::begin().map(Flow::Portal)
        } else if let Some(recorder) = self.recorder.borrow().as_ref() {
            recorder
                .flow
                .borrow()
                .begin_recording_screenshot(0)
                .map(Flow::X11)
        } else {
            CaptureFlow::begin(0).map(Flow::X11)
        };
        let flow = match flow {
            Ok(flow) => flow,
            Err(error) => return self.capture_failed(&error),
        };
        let generation = flow.generation();
        *self.capture.borrow_mut() = Some(flow);
        // As Screenshot Display: the stack leaves the screen unless it is
        // included in captures; the portal path unmaps every window.
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
        let started = Instant::now();
        let host = Rc::downgrade(self);
        let (freeze, include_cursor) =
            (settings.freeze_screen, settings.show_cursor_in_screenshots);
        glib::spawn_future_local(async move {
            if !hidden.is_empty() {
                glib::timeout_future(SETTLE).await;
            }
            let source = gio::spawn_blocking(move || {
                prepare(kind, display, generation, portal, freeze, include_cursor)
            })
            .await
            .unwrap_or_else(|_| Err("the capture worker stopped".into()));
            let Some(host) = host.upgrade() else { return };
            match source {
                Ok(Some(source)) if capture_flow::is_current(generation) => {
                    overlay(
                        &host, kind, source, portal, hidden, settings, started, generation, target,
                    );
                }
                result => {
                    for window in &hidden {
                        window.set_visible(true);
                    }
                    host.capture.borrow_mut().take();
                    match result {
                        Err(error) => host.capture_failed(&error),
                        Ok(_) => emit("capture-cancelled", serde_json::json!({"portal": portal})),
                    }
                }
            }
        });
    }
}

/// Off the main thread: the display under the pointer, frozen when set, or
/// the portal's still. `Ok(None)` is cancellation.
fn prepare(
    kind: Kind,
    display: Option<String>,
    generation: u64,
    portal: bool,
    freeze: bool,
    include_cursor: bool,
) -> Result<Option<Source>, String> {
    if portal {
        return captures_app::acquire_portal_region_still(generation)
            .map(|still| still.map(|still| Source::Portal(Arc::new(still))))
            .map_err(|error| error.to_string());
    }
    let display = match display {
        Some(display) => display,
        None => pointer_display()?,
    };
    let result = match kind {
        Kind::Region => RegionSession::prepare(&display, generation, freeze, include_cursor)
            .map(|session| Source::Region(Box::new(session))),
        Kind::Window => WindowSession::prepare(&display, generation, freeze, include_cursor, 0.)
            .map(|session| Source::Window(Arc::new(session))),
        Kind::Menu => RegionSession::prepare(&display, generation, freeze, include_cursor)
            .and_then(|region| {
                let window = match WindowSession::prepare(
                    &display,
                    generation,
                    freeze,
                    include_cursor,
                    0.,
                ) {
                    Ok(window) => Some(Arc::new(window)),
                    Err(captures_app::Error::Cancelled) => {
                        return Err(captures_app::Error::Cancelled);
                    }
                    Err(_) => None,
                };
                let displays = captures_capture::XcapBackend
                    .displays()
                    .map(|displays| {
                        displays
                            .into_iter()
                            .map(|display| {
                                let name = capture_menu::display_identity(
                                    &display.name,
                                    display.width,
                                    display.height,
                                    None,
                                )
                                .name;
                                (display.id, name)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Source::Menu {
                    region: Box::new(region),
                    window,
                    displays,
                })
            }),
    };
    match result {
        Ok(source) => Ok(Some(source)),
        Err(captures_app::Error::Cancelled) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn texture(image: &image::RgbaImage) -> gdk::Texture {
    gdk::MemoryTexture::new(
        image.width() as i32,
        image.height() as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from(image.as_raw().as_slice()),
        image.width() as usize * 4,
    )
    .upcast()
}

#[allow(clippy::too_many_arguments)]
fn overlay(
    host: &Rc<Host>,
    kind: Kind,
    source: Source,
    portal: bool,
    hidden: Vec<gtk::Window>,
    settings: captures_settings::AppSettings,
    started: Instant,
    generation: u64,
    target: MenuTarget,
) {
    let (texture, display) = match &source {
        Source::Region(session) => (
            session.frozen_preview().map(|image| texture(&image)),
            Some(session.display().clone()),
        ),
        Source::Window(session) => (
            session.frozen_preview().map(|image| texture(&image)),
            Some(session.display().clone()),
        ),
        Source::Menu { region, .. } => (
            region.frozen_preview().map(|image| texture(&image)),
            Some(region.display().clone()),
        ),
        Source::Portal(still) => (Some(texture(still)), None),
    };
    let window = gtk::ApplicationWindow::builder()
        .application(&host.app)
        .title(match kind {
            Kind::Region => REGION_TITLE,
            Kind::Window => WINDOW_TITLE,
            Kind::Menu => MENU_TITLE,
        })
        .decorated(false)
        .build();
    window.add_css_class("captures");
    window.add_css_class("capture-surface");
    if texture.is_none() {
        window.add_css_class("live");
    }
    let root = gtk::Overlay::builder()
        .accessible_role(gtk::AccessibleRole::Group)
        .build();
    root.update_property(&[gtk::accessible::Property::Label(match kind {
        Kind::Region | Kind::Menu => capture_menu::REGION_SELECTOR_LABEL,
        Kind::Window => capture_menu::WINDOW_SELECTOR_LABEL,
    })]);
    let area = SnapshotArea::new(|_, _, _| {});
    // The focused surface shows the frozen desktop: a named image, as the
    // recording preview (`Application` needs GTK 4.14).
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    root.set_child(Some(&area));
    let chip = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .accessible_role(gtk::AccessibleRole::Status)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .can_target(false)
        .build();
    chip.add_css_class("capture-guidance");
    let chip_title = gtk::Label::new(None);
    chip_title.add_css_class("capture-guidance-title");
    let chip_hint = gtk::Label::new(None);
    chip_hint.add_css_class("capture-guidance-hint");
    chip.append(&chip_title);
    chip.append(&chip_hint);
    root.add_overlay(&chip);
    // New Capture: the Full screen identity card and the bar. The bar's
    // selects reach the overlay through `link` once it exists.
    let link: Rc<RefCell<std::rc::Weak<Overlay>>> = Rc::default();
    let (bar, display_card) = if kind == Kind::Menu {
        let (displays, current) = match &source {
            Source::Menu {
                displays, region, ..
            } => (displays.clone(), region.display().id.clone()),
            _ => (Vec::new(), String::new()),
        };
        let identity = match &display {
            Some(display) => {
                capture_menu::display_identity(&display.name, display.width, display.height, None)
            }
            None => capture_menu::display_identity(
                "",
                texture.as_ref().map_or(0, |t| t.width() as u32),
                texture.as_ref().map_or(0, |t| t.height() as u32),
                None,
            ),
        };
        let displays = if displays.is_empty() {
            vec![(current.clone(), identity.name.clone())]
        } else {
            displays
        };
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("capture-display-identity");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_can_target(false);
        // `translate(-50%, -60%)`: a tenth of the card above centre.
        card.set_margin_bottom(26);
        let icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        icon.add_css_class("capture-display-icon");
        icon.set_halign(gtk::Align::Center);
        let glyph = crate::widgets::Icon::new("target-display", 34).with_stroke(1.4);
        glyph.set_halign(gtk::Align::Center);
        glyph.set_valign(gtk::Align::Center);
        glyph.set_hexpand(true);
        icon.append(&glyph);
        card.append(&icon);
        let name = gtk::Label::new(Some(&identity.name));
        name.add_css_class("capture-display-name");
        card.append(&name);
        let detail = gtk::Label::new(Some(&identity.detail));
        detail.add_css_class("capture-display-detail");
        card.append(&detail);
        card.set_visible(false);
        root.add_overlay(&card);
        let (aspect_link, display_link) = (link.clone(), link.clone());
        let bar = Bar::new(
            displays,
            &current,
            settings.auto_start_on_selection,
            move |id| {
                if let Some(overlay) = aspect_link.borrow().upgrade() {
                    overlay.set_aspect(&id);
                }
            },
            move |id| {
                if let Some(overlay) = display_link.borrow().upgrade() {
                    overlay.switch_display(id);
                }
            },
        );
        bar.root.set_halign(gtk::Align::Center);
        bar.root.set_valign(gtk::Align::End);
        bar.root.set_margin_bottom(26);
        root.add_overlay(&bar.root);
        (Some(bar), Some(card))
    } else {
        (None, None)
    };
    window.set_child(Some(&root));

    let overlay = Rc::new(Overlay {
        window: window.clone(),
        kind,
        source: RefCell::new(Some(source)),
        state: RefCell::default(),
        area: area.clone(),
        root: root.clone(),
        chip,
        chip_title,
        chip_hint,
        chip_over: Cell::new(false),
        bounds: Cell::new(display.as_ref().map_or((1., 1.), |d| d.overlay_size())),
        done: Cell::new(false),
        portal,
        hidden,
        settings,
        started,
        texture,
        tokens: host.tokens.borrow().clone(),
        host: Rc::downgrade(host),
        laid_out: Cell::new(0),
        bar,
        target: Cell::new(target),
        aspect: Cell::new(None),
        display_card,
    });
    *link.borrow_mut() = Rc::downgrade(&overlay);
    overlay.guidance();

    if let Some(display) = &display {
        let (width, height) = display.overlay_size();
        window.set_default_size(width as i32, height as i32);
        // X11 only: on Wayland a fixed size would pin the surface below the
        // fullscreen or layer-shell size the compositor configures.
        window.set_resizable(false);
        #[cfg(feature = "x11")]
        {
            let geometry = (display.x, display.y, display.width, display.height);
            if let Some(placer) = crate::previews::x11::Placer::new(&WidgetExt::display(&window)) {
                window.connect_realize(move |window| {
                    if let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() {
                        placer.cover(&surface, geometry);
                    }
                });
            }
        }
    } else {
        #[cfg(feature = "layer-shell")]
        let layered = crate::previews::layer_shell::cover(&window);
        #[cfg(not(feature = "layer-shell"))]
        let layered = false;
        if !layered {
            window.fullscreen();
        }
    }

    let weak = Rc::downgrade(&overlay);
    area.set_draw(move |snapshot, width, height| {
        if let Some(overlay) = weak.upgrade() {
            overlay.draw(snapshot, width, height);
        }
    });

    match kind {
        Kind::Region => overlay.region_input(),
        Kind::Window => overlay.window_input(),
        Kind::Menu => overlay.menu_input(),
    }
    if kind == Kind::Menu {
        overlay.retarget(target);
    }
    // Escape: CaptureFlow's global key on X11; the focused surface's key
    // on Wayland (the portal flow grabs nothing). Capture phase, so Enter
    // reaches New Capture before a focused segment activates itself.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&overlay);
    let host_ref = Rc::downgrade(host);
    let surface = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let (Some(overlay), Some(host)) = (weak.upgrade(), host_ref.upgrade()) else {
            return glib::Propagation::Proceed;
        };
        if key == gdk::Key::Escape {
            overlay.cancel(&host);
            return glib::Propagation::Stop;
        }
        // Shipping's Enter confirms New Capture's target, unmodified and
        // not from Close or a select, which keep their own activation.
        let focus = surface
            .upgrade()
            .and_then(|window| GtkWindowExt::focus(&window));
        if overlay.kind == Kind::Menu
            && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
            && (modifiers & gtk::accelerator_get_default_mod_mask()).is_empty()
            && enter_confirms(focus.as_ref())
            && overlay.can_start()
        {
            overlay.start();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    let weak = Rc::downgrade(&overlay);
    let host_ref = Rc::downgrade(host);
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let (Some(overlay), Some(host)) = (weak.upgrade(), host_ref.upgrade()) else {
            return glib::ControlFlow::Break;
        };
        if overlay.done.get() {
            return glib::ControlFlow::Break;
        }
        if !capture_flow::is_current(generation) {
            overlay.cancel(&host);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    *host.selector.borrow_mut() = Some(Handle(overlay.clone()));
    if kind == Kind::Menu {
        crate::live::route_shortcuts_to_selector(Some(generation));
    }
    #[cfg(feature = "x11")]
    if kind == Kind::Menu {
        window.connect_map(|window| {
            let window = window.downgrade();
            // GTK finishes mapping the GDK toplevel after the widget's map
            // signal. XSetInputFocus before that can fail with BadMatch.
            glib::idle_add_local_once(move || {
                if let Some(window) = window.upgrade()
                    && let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>()
                    && surface.is_mapped()
                    && let Some(placer) =
                        crate::previews::x11::Placer::new(&WidgetExt::display(&window))
                {
                    placer.focus_selector(&surface);
                }
            });
        });
    }
    window.present();
    area.grab_focus();
    emit(
        "selector",
        serde_json::json!({"kind": match kind {
                Kind::Region => "region",
                Kind::Window => "window",
                Kind::Menu => "new-capture",
            },
            "portal": portal, "frozen": overlay.texture.is_some(),
            "windows": overlay.windows().len()}),
    );
}

/// The open overlay, held by the host until it confirms or cancels.
pub struct Handle(#[allow(dead_code)] Rc<Overlay>);

impl Overlay {
    fn windows(&self) -> Vec<WindowDescriptor> {
        match self.source.borrow().as_ref() {
            Some(Source::Window(session)) => session.windows().to_vec(),
            Some(Source::Menu {
                window: Some(window),
                ..
            }) => window.windows().to_vec(),
            _ => Vec::new(),
        }
    }

    /// What the overlay is selecting now: fixed for the direct overlays,
    /// the bar's target in New Capture.
    fn mode(&self) -> MenuTarget {
        match self.kind {
            Kind::Region => MenuTarget::Region,
            Kind::Window => MenuTarget::Window,
            Kind::Menu => self.target.get(),
        }
    }

    /// Shipping `CaptureGuidance` copy for the current state.
    fn guidance(&self) {
        let state = self.state.borrow();
        let target = match self.mode() {
            MenuTarget::Region => GuidanceTarget::Region,
            MenuTarget::Window if self.kind == Kind::Window && state.hovered == Some(None) => {
                GuidanceTarget::Display
            }
            MenuTarget::Window => GuidanceTarget::Window,
            MenuTarget::Display => GuidanceTarget::Display,
        };
        // Full screen shows the display card instead of the chip.
        self.chip.set_visible(self.mode() != MenuTarget::Display);
        let copy = capture_menu::guidance(target, state.feedback > 0);
        self.chip_title.set_label(copy.title);
        self.chip_hint.set_label(copy.hint);
        self.chip.set_css_classes(&[
            "capture-guidance",
            match state.feedback {
                0 => "steady",
                attempt if attempt % 2 == 1 => "feedback-a",
                _ => "feedback-b",
            },
            // Hidden while dragging out a region and while the pointer is
            // near (shipping's 28 pt approach with 12 pt leave slack).
            if state.drag.is_some() || self.chip_over.get() {
                "faded"
            } else {
                "shown"
            },
        ]);
    }

    /// Duck the chip from the pointer.
    fn pointer(&self, x: f64, y: f64) {
        let Some(bounds) = self.chip.compute_bounds(&self.root) else {
            return;
        };
        let over = capture_menu::pointer_over_guidance(
            x,
            y,
            f64::from(bounds.x()),
            f64::from(bounds.y()),
            f64::from(bounds.x() + bounds.width()),
            f64::from(bounds.y() + bounds.height()),
            self.chip_over.get(),
        );
        if over != self.chip_over.get() {
            self.chip_over.set(over);
            self.guidance();
        }
    }

    /// Widget points to selection points.
    fn point(&self, x: f64, y: f64) -> Point {
        let (width, height) = self.bounds.get();
        let scale_x = width / f64::from(self.area.width().max(1));
        let scale_y = height / f64::from(self.area.height().max(1));
        Point {
            x: x * scale_x,
            y: y * scale_y,
        }
    }

    fn region_input(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, y| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let point = overlay.point(x, y);
            {
                let mut state = overlay.state.borrow_mut();
                let initial = Rect {
                    x: point.x,
                    y: point.y,
                    width: 0.,
                    height: 0.,
                };
                state.rect = Some(initial);
                state.drag = Some((DragMode::Create, point, initial));
            }
            overlay.guidance();
            overlay.area.queue_draw();
        });
        let weak = Rc::downgrade(self);
        let update = move |gesture: &gtk::GestureDrag, dx: f64, dy: f64| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let Some((x, y)) = gesture.start_point() else {
                return;
            };
            let shift = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            let point = overlay.point(x + dx, y + dy);
            let (width, height) = overlay.bounds.get();
            let mut state = overlay.state.borrow_mut();
            if let Some((mode, origin, initial)) = state.drag {
                state.rect = Some(selection::drag(
                    mode,
                    origin,
                    point,
                    initial,
                    Bounds { width, height },
                    DragOptions {
                        minimum_size: MINIMUM_SIZE,
                        aspect_ratio: None,
                        force_square: shift,
                    },
                ));
            }
            drop(state);
            overlay.area.queue_draw();
        };
        let finish = update.clone();
        drag.connect_drag_update(move |gesture, dx, dy| update(gesture, dx, dy));
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |gesture, dx, dy| {
            // Button-up owns the final position and Shift.
            finish(gesture, dx, dy);
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let rect = {
                let mut state = overlay.state.borrow_mut();
                state.drag = None;
                state.rect
            };
            // A completed drag commits at once; a click, or a drag too
            // small to capture, shows the feedback instead.
            match rect.filter(|_| dx.hypot(dy) > 0.) {
                Some(rect) if selection::capturable(Some(rect)) => {
                    overlay.confirm(Choice::Region(rect));
                }
                _ => overlay.feedback(),
            }
        });
        self.area.add_controller(drag);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            if let Some(overlay) = weak.upgrade() {
                overlay.pointer(x, y);
            }
        });
        self.area.add_controller(motion);
        self.area
            .update_property(&[gtk::accessible::Property::Label(
                capture_menu::REGION_SELECTOR_LABEL,
            )]);
    }

    /// Shipping `showSelectionFeedback`: a click without a region says
    /// "Click and drag", nudges the chip, and settles after 1.8 s.
    fn feedback(self: &Rc<Self>) {
        let attempt = {
            let mut state = self.state.borrow_mut();
            state.rect = None;
            state.feedback = state.feedback.saturating_add(1);
            state.feedback
        };
        self.guidance();
        self.area.queue_draw();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(capture_menu::GUIDANCE_FEEDBACK, move || {
            if let Some(overlay) = weak.upgrade()
                && overlay.state.borrow().feedback == attempt
            {
                overlay.state.borrow_mut().feedback = 0;
                overlay.guidance();
            }
        });
    }

    fn window_input(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            if let Some(overlay) = weak.upgrade() {
                overlay.pointer(x, y);
                overlay.hover(x, y);
            }
        });
        self.area.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, x, y| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            // The clicked target, not later motion's.
            overlay.hover(x, y);
            let target = match overlay.state.borrow().hovered.flatten() {
                Some(index) => overlay
                    .windows()
                    .get(index)
                    .map_or(Target::Display, |window| Target::Window {
                        id: window.id.clone(),
                    }),
                None => Target::Display,
            };
            overlay.confirm(Choice::Window(target));
        });
        self.area.add_controller(click);
        self.describe_target();
    }

    /// New Capture: one drag gesture for Region (create, move, corner
    /// resize), and hover plus click to pick a Window or the display.
    fn menu_input(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, y| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            if overlay.mode() != MenuTarget::Region || overlay.state.borrow().primary.starting {
                return;
            }
            let point = overlay.point(x, y);
            {
                let mut state = overlay.state.borrow_mut();
                let existing = state.rect.filter(|rect| selection::capturable(Some(*rect)));
                let mode = existing.map_or(DragMode::Create, |rect| {
                    hit_test(rect, point, overlay.handle_reach())
                });
                let initial = match (mode, existing) {
                    (DragMode::Create, _) | (_, None) => Rect {
                        x: point.x,
                        y: point.y,
                        width: 0.,
                        height: 0.,
                    },
                    (_, Some(rect)) => rect,
                };
                if matches!(mode, DragMode::Create) {
                    state.rect = Some(initial);
                }
                state.drag = Some((mode, point, initial));
            }
            overlay.guidance();
            overlay.sync_bar();
            overlay.area.queue_draw();
        });
        let weak = Rc::downgrade(self);
        let update = move |gesture: &gtk::GestureDrag, dx: f64, dy: f64| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let Some((x, y)) = gesture.start_point() else {
                return;
            };
            let shift = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            let point = overlay.point(x + dx, y + dy);
            let (width, height) = overlay.bounds.get();
            let aspect = overlay.aspect.get();
            let mut state = overlay.state.borrow_mut();
            if let Some((mode, origin, initial)) = state.drag {
                state.rect = Some(selection::drag(
                    mode,
                    origin,
                    point,
                    initial,
                    Bounds { width, height },
                    DragOptions {
                        minimum_size: MINIMUM_SIZE,
                        aspect_ratio: aspect,
                        force_square: shift,
                    },
                ));
            }
            drop(state);
            overlay.area.queue_draw();
        };
        let finish = update.clone();
        drag.connect_drag_update(move |gesture, dx, dy| update(gesture, dx, dy));
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |gesture, dx, dy| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let Some((mode, ..)) = overlay.state.borrow().drag else {
                return;
            };
            finish(gesture, dx, dy);
            let rect = {
                let mut state = overlay.state.borrow_mut();
                state.drag = None;
                state.rect
            };
            let moved = dx.hypot(dy) > 0.;
            if matches!(mode, DragMode::Create) && !(moved && selection::capturable(rect)) {
                // A click, or a region too small to capture.
                overlay.feedback();
            } else if overlay.settings.auto_start_on_selection && matches!(mode, DragMode::Create) {
                overlay.start();
                return;
            }
            overlay.guidance();
            overlay.sync_bar();
            overlay.area.queue_draw();
        });
        self.area.add_controller(drag);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            if let Some(overlay) = weak.upgrade() {
                overlay.pointer(x, y);
                if overlay.mode() == MenuTarget::Window {
                    overlay.hover(x, y);
                }
            }
        });
        self.area.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, x, y| {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            if overlay.mode() != MenuTarget::Window || overlay.portal {
                return;
            }
            overlay.hover(x, y);
            let hovered = overlay.state.borrow().hovered;
            overlay.state.borrow_mut().selected = hovered;
            overlay.sync_bar();
            overlay.area.queue_draw();
            if overlay.settings.auto_start_on_selection && hovered.is_some() {
                overlay.start();
            }
        });
        self.area.add_controller(click);

        let Some(bar) = &self.bar else { return };
        let weak = Rc::downgrade(self);
        bar.close.connect_clicked(move |_| {
            if let Some(overlay) = weak.upgrade()
                && let Some(host) = overlay.host.upgrade()
            {
                overlay.cancel(&host);
            }
        });
        for (button, target) in
            bar.targets
                .iter()
                .zip([MenuTarget::Region, MenuTarget::Window, MenuTarget::Display])
        {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(overlay) = weak.upgrade() {
                    overlay.set_target(target);
                }
            });
        }
        bar.set_window_available(self.portal || !self.windows().is_empty());
        let weak = Rc::downgrade(self);
        bar.primary.connect_clicked(move |_| {
            if let Some(overlay) = weak.upgrade() {
                overlay.start();
            }
        });
        self.sync_bar();
        self.area
            .update_property(&[gtk::accessible::Property::Label(
                capture_menu::REGION_SELECTOR_LABEL,
            )]);
    }

    /// Hit slop around each 10 px corner handle, in selection points.
    fn handle_reach(&self) -> f64 {
        let (width, _) = self.bounds.get();
        f64::from(HANDLE + 4.) * width / f64::from(self.area.width().max(1))
    }

    /// Shipping's target switch: hovers and auto-start reset; a second
    /// Full screen click with auto-start on starts.
    fn set_target(self: &Rc<Self>, target: MenuTarget) {
        let again = self.retarget(target);
        if target == MenuTarget::Display && again && self.settings.auto_start_on_selection {
            self.start();
        }
    }

    /// Switch the target without starting; true when it was already chosen.
    fn retarget(&self, target: MenuTarget) -> bool {
        let again = self.target.replace(target) == target;
        {
            let mut state = self.state.borrow_mut();
            state.hovered = None;
            state.drag = None;
        }
        if let Some(card) = &self.display_card {
            card.set_visible(target == MenuTarget::Display);
        }
        self.area.set_cursor_from_name(Some(match target {
            MenuTarget::Region => "crosshair",
            MenuTarget::Window => "pointer",
            MenuTarget::Display => "default",
        }));
        self.guidance();
        self.sync_bar();
        self.area.queue_draw();
        again
    }

    /// The Aspect select: lock the ratio and snap a settled region to it.
    fn set_aspect(&self, id: &str) {
        let aspect = capture_bar::ASPECT_PRESETS
            .iter()
            .find(|(preset, ..)| *preset == id)
            .and_then(|(.., ratio)| *ratio);
        self.aspect.set(aspect);
        let (width, height) = self.bounds.get();
        let mut state = self.state.borrow_mut();
        if let Some(rect) = state.rect.filter(|rect| selection::capturable(Some(*rect))) {
            state.rect = Some(selection::constrain(
                rect,
                aspect,
                Some(Bounds { width, height }),
                MINIMUM_SIZE,
            ));
        }
        drop(state);
        self.sync_bar();
        self.area.queue_draw();
    }

    /// The Display select: reopen New Capture on another display.
    fn switch_display(self: &Rc<Self>, id: String) {
        let current = match self.source.borrow().as_ref() {
            Some(Source::Menu { region, .. }) => region.display().id.clone(),
            _ => return,
        };
        let Some(host) = self.host.upgrade() else {
            return;
        };
        if id == current || self.done.get() {
            return;
        }
        self.state.borrow_mut().primary.switching_display = true;
        self.sync_bar();
        self.close();
        if let Some(flow) = host.capture.borrow_mut().take() {
            flow.cancel();
        }
        host.selector.borrow_mut().take();
        for window in &self.hidden {
            window.set_visible(true);
        }
        host.select_on(Kind::Menu, Some(id), self.target.get());
    }

    /// Repeat a target chord: read the visible controls before replacing the
    /// overlay. Keep the same cancellation owner and never publish during prep.
    fn recapture(self: &Rc<Self>, kind: Kind) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        // A Wayland window belongs to the compositor's picker, not our still.
        if self.portal && kind == Kind::Window {
            self.start();
            return;
        }
        let Some(generation) = host.capture.borrow().as_ref().map(Flow::generation) else {
            return;
        };
        let display = match self.source.borrow().as_ref() {
            Some(Source::Menu { region, .. }) => Some(region.display().id.clone()),
            _ => None,
        };
        self.state.borrow_mut().primary.starting = true;
        let old = self.clone();
        glib::spawn_future_local(async move {
            let portal = old.portal;
            let include_cursor = old.settings.show_cursor_in_screenshots;
            let result = gio::spawn_blocking(move || {
                prepare(kind, display, generation, portal, true, include_cursor)
            })
            .await
            .unwrap_or_else(|_| Err("the capture worker stopped".into()));
            if old.done.get() || !capture_flow::is_current(generation) {
                return;
            }
            match result {
                Ok(Some(source)) => {
                    old.close();
                    let mut settings = old.settings.clone();
                    // A countdown must not replace the explicit controls snapshot
                    // with fresh pixels after its window has been hidden.
                    settings.screenshot_countdown_seconds = 0;
                    overlay(
                        &host,
                        kind,
                        source,
                        portal,
                        old.hidden.clone(),
                        settings,
                        old.started,
                        generation,
                        old.target.get(),
                    );
                }
                result => {
                    old.cancel(&host);
                    if let Err(error) = result {
                        host.capture_failed(&error);
                    }
                }
            }
        });
    }

    fn has_window_session(&self) -> bool {
        matches!(
            self.source.borrow().as_ref(),
            Some(Source::Menu {
                window: Some(_),
                ..
            })
        )
    }

    fn can_start(&self) -> bool {
        let state = self.state.borrow();
        if state.primary.starting || state.primary.switching_display {
            return false;
        }
        match self.mode() {
            MenuTarget::Region => state.drag.is_none() && selection::capturable(state.rect),
            MenuTarget::Window => self.portal || state.selected.is_some(),
            MenuTarget::Display => true,
        }
    }

    fn sync_bar(&self) {
        let Some(bar) = &self.bar else { return };
        let primary = self.state.borrow().primary;
        bar.sync(
            self.mode(),
            self.can_start(),
            self.settings.auto_start_on_selection,
            primary,
        );
    }

    /// The primary action: capture the chosen target.
    fn start(self: &Rc<Self>) {
        if self.done.get() || !self.can_start() {
            return;
        }
        let choice = match (self.mode(), self.portal) {
            (MenuTarget::Region, _) => match self.state.borrow().rect {
                Some(rect) => Choice::Region(rect),
                None => return,
            },
            (MenuTarget::Display, _) => Choice::Display,
            // The portal's own window picker, or X11 without a window list.
            (MenuTarget::Window, _) if self.portal || !self.has_window_session() => {
                let Some(host) = self.host.upgrade() else {
                    return;
                };
                self.close();
                if let Some(flow) = host.capture.borrow_mut().take() {
                    flow.cancel();
                }
                host.selector.borrow_mut().take();
                for window in &self.hidden {
                    window.set_visible(true);
                }
                host.capture_now(CaptureMode::Window);
                return;
            }
            (MenuTarget::Window, _) => {
                let selected = self.state.borrow().selected.flatten();
                Choice::Window(match selected {
                    Some(index) => match self.windows().get(index) {
                        Some(window) => Target::Window {
                            id: window.id.clone(),
                        },
                        None => return,
                    },
                    None => Target::Display,
                })
            }
        };
        self.state.borrow_mut().primary.starting = true;
        self.sync_bar();
        self.confirm(choice);
    }

    fn hover(&self, x: f64, y: f64) {
        let point = self.point(x, y);
        let hovered = match self.source.borrow().as_ref() {
            Some(Source::Window(session)) => session.hit_test(point),
            Some(Source::Menu {
                window: Some(window),
                ..
            }) => window.hit_test(point),
            _ => None,
        };
        let changed = self.state.borrow().hovered != Some(hovered);
        if changed {
            self.state.borrow_mut().hovered = Some(hovered);
            self.guidance();
            self.describe_target();
            self.area.queue_draw();
        }
    }

    /// AppKit's live target name (`Target: …`).
    fn describe_target(&self) {
        let windows = self.windows();
        let name = match self.state.borrow().hovered.flatten() {
            Some(index) => windows.get(index).map_or("Window", window_label).to_owned(),
            None => capture_menu::DISPLAY_TARGET.to_owned(),
        };
        self.root
            .update_property(&[gtk::accessible::Property::Description(
                &capture_menu::target_description(&name),
            )]);
    }

    fn draw(&self, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let surface = graphene::Rect::new(0., 0., width, height);
        if let Some(texture) = &self.texture {
            snapshot.append_texture(texture, &surface);
        }
        // The portal still maps onto the surface as laid out; the chip sits
        // 16% down it.
        if self.portal {
            self.bounds.set((f64::from(width), f64::from(height)));
        }
        if self.laid_out.replace(height as i32) != height as i32 {
            let (chip, top) = (
                self.chip.clone(),
                capture_menu::guidance_top(f64::from(height)),
            );
            glib::idle_add_local_once(move || chip.set_margin_top(top.round() as i32));
        }
        let t = self.tokens.clone();
        let (bounds_w, bounds_h) = self.bounds.get();
        let sx = width as f64 / bounds_w.max(1.);
        let sy = height as f64 / bounds_h.max(1.);
        let to_widget = |rect: Rect| {
            graphene::Rect::new(
                (rect.x * sx) as f32,
                (rect.y * sy) as f32,
                (rect.width * sx) as f32,
                (rect.height * sy) as f32,
            )
        };
        let state = self.state.borrow();
        let menu = self.kind == Kind::Menu;
        let veil = |alpha: f32| gdk::RGBA::new(4. / 255., 5. / 255., 8. / 255., alpha);
        match self.mode() {
            MenuTarget::Region => {
                let shade = if menu {
                    veil(MENU_SHADE)
                } else {
                    t.rgba("capture-shade")
                };
                match state
                    .rect
                    .filter(|rect| rect.width > 0. && rect.height > 0.)
                {
                    Some(rect) => {
                        let rect = to_widget(rect);
                        shade_around(snapshot, &surface, &rect, &shade);
                        self.marquee(snapshot, &t, &rect, menu);
                    }
                    None => snapshot.append_color(&shade, &surface),
                }
            }
            MenuTarget::Display => {
                // `.recording-target-display`: the darker veil and a 2 px
                // inset accent outline; the identity card is a widget.
                snapshot.append_color(&veil(MENU_SHADE_DISPLAY), &surface);
                snapshot.append_border(
                    &gsk::RoundedRect::from_rect(surface, 0.),
                    &[2.; 4],
                    &[t.rgba("theme-accent"); 4],
                );
            }
            MenuTarget::Window => {
                let accent = t.rgba("theme-accent");
                let windows = self.windows();
                // New Capture keeps the clicked target when the pointer leaves.
                let target = if menu {
                    state.hovered.or(state.selected)
                } else {
                    state.hovered
                };
                match target {
                    Some(Some(index)) => {
                        let Some(window) = windows.get(index) else {
                            return;
                        };
                        let display = match self.source.borrow().as_ref() {
                            Some(Source::Window(session)) => session.display().clone(),
                            Some(Source::Menu {
                                window: Some(window),
                                ..
                            }) => window.display().clone(),
                            _ => return,
                        };
                        let rect = to_widget(Rect {
                            x: f64::from(window.x - display.x),
                            y: f64::from(window.y - display.y),
                            width: f64::from(window.width),
                            height: f64::from(window.height),
                        })
                        .intersection(&surface)
                        .unwrap_or_else(graphene::Rect::zero);
                        let shade = if menu {
                            veil(MENU_SHADE_WINDOW)
                        } else {
                            t.rgba("capture-shade-window")
                        };
                        shade_around(snapshot, &surface, &rect, &shade);
                        let mut fill = accent;
                        fill.set_alpha(fill.alpha() * 0.14);
                        snapshot.append_color(&fill, &rect);
                        // An outer 2 px ring: never inset over the window.
                        let ring = rect.inset_r(-2., -2.);
                        snapshot.append_border(
                            &gsk::RoundedRect::from_rect(ring, 0.),
                            &[2.; 4],
                            &[accent; 4],
                        );
                        snapshot.push_clip(&rect);
                        let space = t.number("s-2");
                        self.chip_label(
                            snapshot,
                            &t,
                            (rect.x() + space, rect.y() + space),
                            window_label(window),
                            rect.width() - 8.,
                        );
                        snapshot.pop();
                    }
                    Some(None) => {
                        snapshot.append_border(
                            &gsk::RoundedRect::from_rect(surface, 0.),
                            &[2.; 4],
                            &[accent; 4],
                        );
                        let inset = t.number("s-4");
                        self.chip_label(
                            snapshot,
                            &t,
                            (inset, inset),
                            capture_menu::DISPLAY_TARGET,
                            360_f32.min(width - 8.),
                        );
                    }
                    // The window overlays stay clear until hovered.
                    None => {}
                }
            }
        }
    }

    /// Shipping `.selection-box`: a 1.5 px accent border between a 1 px dark
    /// outer and a 1 px light inner hairline, with the left-aligned accent
    /// size badge above it (or inside near the top edge), in the overlay's
    /// logical pixels as shipping's CSS pixels.
    ///
    /// `menu`: New Capture's `.recording-selection-frame`, 2 px corners, the
    /// badge centred over it and the four corner handles.
    fn marquee(&self, snapshot: &gtk::Snapshot, t: &Tokens, rect: &graphene::Rect, menu: bool) {
        let radius = if menu { 2. } else { 0. };
        let outer = rect.inset_r(-1., -1.);
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(outer, radius + 1.),
            &[1.; 4],
            &[t.rgba("selection-hairline-outer"); 4],
        );
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(*rect, radius),
            &[1.5; 4],
            &[t.rgba("theme-accent"); 4],
        );
        if rect.width() > 3. && rect.height() > 3. {
            snapshot.append_border(
                &gsk::RoundedRect::from_rect(rect.inset_r(1.5, 1.5), (radius - 1.5).max(0.)),
                &[1.; 4],
                &[t.rgba("selection-hairline-inner"); 4],
            );
        }
        let text = format!("{} × {}", rect.width().round(), rect.height().round());
        let layout = self.layout(&text, t.number("text-xs"), pango::Weight::Semibold);
        let (text_w, text_h) = layout.pixel_size();
        let padding = (t.number("s-4"), t.number("s-2"));
        let size = (
            text_w as f32 + 2. * padding.0,
            text_h as f32 + 2. * padding.1,
        );
        let top = rect.y()
            + 1.5
            + if rect.y() < 30. {
                t.number("s-3")
            } else {
                -30.
            };
        let left = if menu {
            rect.x() + (rect.width() - size.0) / 2.
        } else {
            rect.x() + 1.5
        };
        let badge = graphene::Rect::new(left, top, size.0, size.1);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(badge, t.number("r-sm")));
        snapshot.append_color(&t.rgba("theme-accent"), &badge);
        snapshot.pop();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(
            badge.x() + padding.0,
            badge.y() + padding.1,
        ));
        snapshot.append_layout(&layout, &t.rgba("theme-accent-ink"));
        snapshot.restore();
        if menu {
            // `.handle`: accent dots with a 2 px ink ring and a soft shadow,
            // 6 px out from the frame's padding box.
            let (accent, ink) = (t.rgba("theme-accent"), t.rgba("theme-accent-ink"));
            let shadow = gdk::RGBA::new(0., 0., 0., 0.4);
            let (left, top) = (rect.x() + 1.5 - 6., rect.y() + 1.5 - 6.);
            let (right, bottom) = (
                rect.x() + rect.width() - 1.5 + 6. - HANDLE,
                rect.y() + rect.height() - 1.5 + 6. - HANDLE,
            );
            for (x, y) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
                let dot = gsk::RoundedRect::from_rect(
                    graphene::Rect::new(x, y, HANDLE, HANDLE),
                    HANDLE / 2.,
                );
                snapshot.append_outset_shadow(&dot, &shadow, 0., 1., 0., 2.);
                snapshot.push_rounded_clip(&dot);
                snapshot.append_color(&accent, dot.bounds());
                snapshot.pop();
                snapshot.append_border(&dot, &[2.; 4], &[ink; 4]);
            }
        }
    }

    /// Shipping's glass title chip (`.window-target span`,
    /// `.capture-display-fallback span`): one ellipsized `--text-xs` line.
    fn chip_label(
        &self,
        snapshot: &gtk::Snapshot,
        t: &Tokens,
        (x, y): (f32, f32),
        text: &str,
        max_width: f32,
    ) {
        let padding = (t.number("s-3"), t.number("s-2"));
        let layout = self.layout(text, t.number("text-xs"), pango::Weight::Medium);
        layout.set_width(((max_width - 2. * padding.0).max(0.) * pango::SCALE as f32) as i32);
        layout.set_ellipsize(pango::EllipsizeMode::End);
        let (text_w, text_h) = layout.pixel_size();
        let chip = graphene::Rect::new(
            x,
            y,
            text_w as f32 + 2. * padding.0,
            text_h as f32 + 2. * padding.1,
        );
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(chip, t.number("r-sm")));
        snapshot.append_color(&t.rgba("glass-strong"), &chip);
        snapshot.pop();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x + padding.0, y + padding.1));
        snapshot.append_layout(&layout, &t.rgba("glass-text"));
        snapshot.restore();
    }

    fn layout(&self, text: &str, size: f32, weight: pango::Weight) -> pango::Layout {
        let layout = self.area.create_pango_layout(Some(text));
        let mut font = layout.context().font_description().unwrap_or_default();
        font.set_absolute_size(f64::from(size) * f64::from(pango::SCALE));
        font.set_weight(weight);
        layout.set_font_description(Some(&font));
        layout
    }

    fn close(&self) {
        self.done.set(true);
        if self.kind == Kind::Menu {
            crate::live::route_shortcuts_to_selector(None);
        }
        self.window.close();
    }

    fn cancel(&self, host: &Rc<Host>) {
        if self.done.get() {
            return;
        }
        self.close();
        if let Some(flow) = host.capture.borrow_mut().take() {
            flow.cancel();
        }
        host.selector.borrow_mut().take();
        for window in &self.hidden {
            window.set_visible(true);
        }
        emit(
            "capture-cancelled",
            serde_json::json!({"portal": self.portal}),
        );
    }

    /// Hide the overlay, run the countdown with fresh pixels after it, and
    /// capture off the main thread.
    fn confirm(self: &Rc<Self>, choice: Choice) {
        if self.done.get() {
            return;
        }
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let seconds = if self.portal {
            0
        } else {
            self.settings.screenshot_countdown_seconds
        };
        // Keep cancellation cleanup live until the handoff succeeds. Drop the
        // capture borrow before cancel(), which takes both host-owned handles.
        let countdown_failed = match host.capture.borrow_mut().as_mut() {
            Some(Flow::X11(flow)) => flow.start_countdown(seconds).is_err(),
            _ => false,
        };
        if countdown_failed {
            return self.cancel(&host);
        }
        self.close();
        let Some(source) = self.source.borrow_mut().take() else {
            return;
        };
        let display = match &source {
            Source::Region(region) | Source::Menu { region, .. } => Some(region.display().clone()),
            Source::Window(window) => Some(window.display().clone()),
            Source::Portal(_) => None,
        };
        let generation = host.capture.borrow().as_ref().map_or(0, Flow::generation);
        let (overlay, root, scale) = (
            self.clone(),
            host.history_root.clone(),
            self.texture.as_ref().map_or(1., |texture| {
                f64::from(texture.width()) / self.bounds.get().0.max(1.)
            }),
        );
        glib::spawn_future_local(async move {
            crate::countdown::wait(&host, generation, seconds, display.as_ref()).await;
            glib::timeout_future(SETTLE).await;
            let after_countdown = seconds > 0;
            let include_cursor = overlay.settings.show_cursor_in_screenshots;
            let result = gio::spawn_blocking(move || match (source, choice) {
                (Source::Region(session), Choice::Region(rect)) => {
                    match session.capture(&root, rect, after_countdown) {
                        Ok(artifact) => Ok(Some(artifact)),
                        Err(captures_app::Error::Cancelled) => Ok(None),
                        Err(error) => Err(error.to_string()),
                    }
                }
                (Source::Window(session), Choice::Window(target)) => {
                    match session.capture(&root, &target, after_countdown) {
                        Ok(artifact) => Ok(Some(artifact)),
                        Err(captures_app::Error::Cancelled) => Ok(None),
                        Err(error) => Err(error.to_string()),
                    }
                }
                (Source::Menu { region, .. }, Choice::Region(rect)) => {
                    match region.capture(&root, rect, after_countdown) {
                        Ok(artifact) => Ok(Some(artifact)),
                        Err(captures_app::Error::Cancelled) => Ok(None),
                        Err(error) => Err(error.to_string()),
                    }
                }
                (
                    Source::Menu {
                        window: Some(window),
                        ..
                    },
                    Choice::Window(target),
                ) => match window.capture(&root, &target, after_countdown) {
                    Ok(artifact) => Ok(Some(artifact)),
                    Err(captures_app::Error::Cancelled) => Ok(None),
                    Err(error) => Err(error.to_string()),
                },
                (Source::Portal(still), Choice::Region(rect)) => {
                    captures_app::persist_portal_region(&root, generation, &still, rect, scale)
                        .map_err(|error| error.to_string())
                }
                (Source::Menu { region, .. }, Choice::Display) => {
                    if let Some(still) = region.frozen_preview().filter(|_| !after_countdown) {
                        captures_app::persist_capture_still(
                            &root,
                            generation,
                            &still,
                            CaptureMode::Display,
                        )
                        .map_err(|error| error.to_string())
                    } else {
                        // Keep the selector's generation and selected display,
                        // including on X11 sessions without an EWMH window list.
                        match captures_app::execute(captures_app::Request::CaptureDisplay {
                            root,
                            display_id: region.display().id.clone(),
                            generation,
                            include_cursor,
                        }) {
                            Ok(captures_app::Response::Captured { artifact }) => Ok(Some(artifact)),
                            Ok(_) => unreachable!("CaptureDisplay returns Captured"),
                            Err(captures_app::Error::Cancelled) => Ok(None),
                            Err(error) => Err(error.to_string()),
                        }
                    }
                }
                (Source::Portal(still), Choice::Display) => captures_app::persist_capture_still(
                    &root,
                    generation,
                    &still,
                    CaptureMode::Display,
                )
                .map_err(|error| error.to_string()),
                _ => Err("the selection does not match its source".into()),
            })
            .await
            .unwrap_or_else(|_| Err("the capture worker stopped".into()));
            for window in &overlay.hidden {
                window.set_visible(true);
            }
            host.capture.borrow_mut().take();
            host.selector.borrow_mut().take();
            match result {
                Ok(Some(artifact)) => {
                    host.captured(artifact, &overlay.settings, overlay.portal, overlay.started);
                }
                Ok(None) => emit(
                    "capture-cancelled",
                    serde_json::json!({"portal": overlay.portal}),
                ),
                Err(error) => host.capture_failed(&error),
            }
        });
    }
}

/// The shade everywhere outside `hole`.
fn shade_around(
    snapshot: &gtk::Snapshot,
    surface: &graphene::Rect,
    hole: &graphene::Rect,
    color: &gdk::RGBA,
) {
    let (left, top, right, bottom) = (
        hole.x(),
        hole.y(),
        hole.x() + hole.width(),
        hole.y() + hole.height(),
    );
    for rect in [
        graphene::Rect::new(0., 0., surface.width(), top),
        graphene::Rect::new(0., bottom, surface.width(), surface.height() - bottom),
        graphene::Rect::new(0., top, left, hole.height()),
        graphene::Rect::new(right, top, surface.width() - right, hole.height()),
    ] {
        if rect.width() > 0. && rect.height() > 0. {
            snapshot.append_color(color, &rect);
        }
    }
}

fn window_label(window: &WindowDescriptor) -> &str {
    let title = window.title.trim();
    if !title.is_empty() {
        return title;
    }
    window
        .app_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Window")
}

/// Shipping's Enter rule: the surface, the Screenshot/Record and target
/// segments and the primary action confirm; any other focused button
/// (Close, a select's trigger) keeps Enter for itself.
fn enter_confirms(focus: Option<&gtk::Widget>) -> bool {
    let mut widget = focus.cloned();
    while let Some(current) = widget {
        if current.is::<gtk::Button>() {
            return current.has_css_class("capture-segment")
                || current.has_css_class("capture-bar-primary");
        }
        widget = current.parent();
    }
    true
}

/// Which part of a settled region a press grabs: a corner handle, the
/// inside (move) or outside (a new region).
fn hit_test(rect: Rect, point: Point, reach: f64) -> DragMode {
    let near = |x: f64, y: f64| (point.x - x).hypot(point.y - y) <= reach;
    if near(rect.x, rect.y) {
        DragMode::Nw
    } else if near(rect.x + rect.width, rect.y) {
        DragMode::Ne
    } else if near(rect.x, rect.y + rect.height) {
        DragMode::Sw
    } else if near(rect.x + rect.width, rect.y + rect.height) {
        DragMode::Se
    } else if point.x >= rect.x
        && point.x <= rect.x + rect.width
        && point.y >= rect.y
        && point.y <= rect.y + rect.height
    {
        DragMode::Move
    } else {
        DragMode::Create
    }
}
