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

use captures_app::capture_flow::{self, CaptureFlow, Countdown, PortalCapture};
use captures_app::capture_menu::{self, GuidanceTarget};
use captures_app::region::RegionSession;
use captures_app::selection::{self, Bounds, DragMode, DragOptions, Point, Rect};
use captures_app::window::{Target, WindowSession};
use captures_capture::{CaptureMode, WindowDescriptor};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk, pango};

use crate::capture::{Flow, SETTLE, pointer_display, wayland};
use crate::tokens::Tokens;
use crate::widgets::SnapshotArea;
use crate::{Host, emit};

/// Shipping's region minimum while dragging (`MINIMUM_SIZE`).
const MINIMUM_SIZE: f64 = 16.;
pub const REGION_TITLE: &str = "Captures Region Selector";
pub const WINDOW_TITLE: &str = "Captures Window Selector";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Region,
    Window,
}

/// What the overlay selects over and later captures from.
enum Source {
    Region(Box<RegionSession>),
    Window(Arc<WindowSession>),
    /// A portal still in its own pixels; selection is in overlay points.
    Portal(Arc<image::RgbaImage>),
}

/// What a confirmed overlay captures.
enum Choice {
    Region(Rect),
    Window(Target),
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
}

impl Host {
    /// Screenshot Region (the direct overlay).
    pub fn capture_region(self: &Rc<Self>) {
        self.select(Kind::Region);
    }

    /// Screenshot Window: the direct overlay on X11, the portal's window
    /// picker on Wayland.
    pub fn capture_window(self: &Rc<Self>) {
        if wayland(self) {
            self.capture_now(CaptureMode::Window);
        } else {
            self.select(Kind::Window);
        }
    }

    fn select(self: &Rc<Self>, kind: Kind) {
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let portal = wayland(self);
        let flow = if portal {
            PortalCapture::begin().map(Flow::Portal)
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
                prepare(kind, generation, portal, freeze, include_cursor)
            })
            .await
            .unwrap_or_else(|_| Err("the capture worker stopped".into()));
            let Some(host) = host.upgrade() else { return };
            match source {
                Ok(Some(source)) if capture_flow::is_current(generation) => {
                    overlay(
                        &host, kind, source, portal, hidden, settings, started, generation,
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
    let display = pointer_display()?;
    let result = match kind {
        Kind::Region => RegionSession::prepare(&display, generation, freeze, include_cursor)
            .map(|session| Source::Region(Box::new(session))),
        Kind::Window => WindowSession::prepare(&display, generation, freeze, include_cursor, 0.)
            .map(|session| Source::Window(Arc::new(session))),
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
        Source::Portal(still) => (Some(texture(still)), None),
    };
    let window = gtk::ApplicationWindow::builder()
        .application(&host.app)
        .title(if kind == Kind::Region {
            REGION_TITLE
        } else {
            WINDOW_TITLE
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
    root.update_property(&[gtk::accessible::Property::Label(if kind == Kind::Region {
        capture_menu::REGION_SELECTOR_LABEL
    } else {
        capture_menu::WINDOW_SELECTOR_LABEL
    })]);
    let area = SnapshotArea::new(|_, _, _| {});
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
    });
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
    }
    // Escape: CaptureFlow's global key on X11; the focused surface's key
    // on Wayland (the portal flow grabs nothing).
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&overlay);
    let host_ref = Rc::downgrade(host);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let (Some(overlay), Some(host)) = (weak.upgrade(), host_ref.upgrade())
        {
            overlay.cancel(&host);
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
    window.present();
    area.grab_focus();
    emit(
        "selector",
        serde_json::json!({"kind": if kind == Kind::Region { "region" } else { "window" },
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
            _ => Vec::new(),
        }
    }

    /// Shipping `CaptureGuidance` copy for the current state.
    fn guidance(&self) {
        let state = self.state.borrow();
        let target = match self.kind {
            Kind::Region => GuidanceTarget::Region,
            Kind::Window if state.hovered == Some(None) => GuidanceTarget::Display,
            Kind::Window => GuidanceTarget::Window,
        };
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

    fn hover(&self, x: f64, y: f64) {
        let point = self.point(x, y);
        let hovered = match self.source.borrow().as_ref() {
            Some(Source::Window(session)) => session.hit_test(point),
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
        match self.kind {
            Kind::Region => {
                let shade = t.rgba("capture-shade");
                match state
                    .rect
                    .filter(|rect| rect.width > 0. && rect.height > 0.)
                {
                    Some(rect) => {
                        let rect = to_widget(rect);
                        shade_around(snapshot, &surface, &rect, &shade);
                        self.marquee(snapshot, &t, &rect);
                    }
                    None => snapshot.append_color(&shade, &surface),
                }
            }
            Kind::Window => {
                let accent = t.rgba("theme-accent");
                let windows = self.windows();
                match state.hovered {
                    Some(Some(index)) => {
                        let Some(window) = windows.get(index) else {
                            return;
                        };
                        let display = match self.source.borrow().as_ref() {
                            Some(Source::Window(session)) => session.display().clone(),
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
                        shade_around(snapshot, &surface, &rect, &t.rgba("capture-shade-window"));
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
                    // The direct window overlay stays clear until hovered.
                    None => {}
                }
            }
        }
    }

    /// Shipping `.selection-box`: a 1.5 px accent border between a 1 px dark
    /// outer and a 1 px light inner hairline, with the left-aligned accent
    /// size badge above it (or inside near the top edge), in the overlay's
    /// logical pixels as shipping's CSS pixels.
    fn marquee(&self, snapshot: &gtk::Snapshot, t: &Tokens, rect: &graphene::Rect) {
        let outer = rect.inset_r(-1., -1.);
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(outer, 0.),
            &[1.; 4],
            &[t.rgba("selection-hairline-outer"); 4],
        );
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(*rect, 0.),
            &[1.5; 4],
            &[t.rgba("theme-accent"); 4],
        );
        if rect.width() > 3. && rect.height() > 3. {
            snapshot.append_border(
                &gsk::RoundedRect::from_rect(rect.inset_r(1.5, 1.5), 0.),
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
        let badge = graphene::Rect::new(rect.x() + 1.5, top, size.0, size.1);
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
        self.close();
        let Some(source) = self.source.borrow_mut().take() else {
            return;
        };
        let seconds = if self.portal {
            0
        } else {
            self.settings.screenshot_countdown_seconds
        };
        if let Some(Flow::X11(flow)) = host.capture.borrow_mut().as_mut()
            && flow.start_countdown(seconds).is_err()
        {
            return self.cancel(&host);
        }
        let generation = host.capture.borrow().as_ref().map_or(0, Flow::generation);
        let (overlay, root, scale) = (
            self.clone(),
            host.history_root.clone(),
            self.texture.as_ref().map_or(1., |texture| {
                f64::from(texture.width()) / self.bounds.get().0.max(1.)
            }),
        );
        glib::spawn_future_local(async move {
            let countdown = Countdown::new(Instant::now(), seconds);
            while countdown.remaining(Instant::now()) > 0 {
                glib::timeout_future(Duration::from_millis(50)).await;
                if !capture_flow::is_current(generation) {
                    break;
                }
            }
            glib::timeout_future(SETTLE).await;
            let after_countdown = seconds > 0;
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
                (Source::Portal(still), Choice::Region(rect)) => {
                    captures_app::persist_portal_region(&root, generation, &still, rect, scale)
                        .map_err(|error| error.to_string())
                }
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
