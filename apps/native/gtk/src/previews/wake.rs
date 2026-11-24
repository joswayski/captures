//! The drag wake a card leaves while its file is dragged out of the stack
//! (`captures_app::preview_wake`, which owns the motion and geometry). The
//! card's whole shell leaves its slot, the pile draws the emptied slot's
//! breathing imprint under the cards, and the end either snaps the wake
//! away (an accepted drop: the card stays hidden while it leaves the stack)
//! or retracts it while the card re-forms.
//!
//! The ribbon to the carried drag image needs the pointer anywhere on the
//! screen and a surface over it. Only composited X11 has both: there a
//! click-through, override-redirect window over the stack's monitor draws
//! the ribbon, motes and bloom from the root pointer, below the drag icon.
//! Its input shape is empty, so GTK's own drag never finds it as a target.
//! Uncomposited X11 (a transparent window would paint black) and Wayland
//! (no global pointer during a drag, and no overlay placement on GNOME)
//! keep the tip on the slot: the card leaving, the imprint and the end
//! animation, without a ribbon.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use captures_app::preview_wake::{
    DragWake, MOTE_HALO_OPACITY, MOTE_HALO_SCALE, SLOT_GLOW_BLUR, SLOT_GLOW_OPACITY,
    SLOT_LINE_OPACITY, SLOT_LINE_WIDTH, SLOT_SHADE_OPACITY, WakeEnd, WakeFrame, WakeInk, WakeRect,
};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use super::pile::Pile;
use crate::tokens::Tokens;

/// `--wake NAME` renders of the wake for review.
pub const FIXTURES: [&str; 3] = ["carried", "returning", "dropped"];

/// The wake's colours: the theme accent, `--glass-text` and `--glass-veil`,
/// and the slot's `--thumbnail-card-radius`.
#[derive(Clone, Copy)]
pub struct Palette {
    accent: gdk::RGBA,
    light: gdk::RGBA,
    veil: gdk::RGBA,
    pub radius: f64,
}

impl Palette {
    pub fn new(tokens: &Tokens) -> Self {
        Self {
            accent: tokens.rgba("theme-accent"),
            light: tokens.rgba("glass-text"),
            veil: tokens.rgba("glass-veil"),
            radius: f64::from(tokens.number("thumbnail-card-radius")),
        }
    }

    fn ink(&self, ink: WakeInk) -> gdk::RGBA {
        match ink {
            WakeInk::Accent => self.accent,
            WakeInk::Light => self.light,
        }
    }
}

/// `color`'s RGB at `alpha`.
fn tint(color: gdk::RGBA, alpha: f64) -> gdk::RGBA {
    gdk::RGBA::new(
        color.red(),
        color.green(),
        color.blue(),
        alpha.clamp(0., 1.) as f32,
    )
}

/// What the pile shows for a card with a wake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakePose {
    pub card_opacity: f64,
    pub card_scale: f64,
    pub slot_opacity: f64,
}

impl WakePose {
    fn of(frame: &WakeFrame) -> Self {
        Self {
            card_opacity: frame.card_opacity,
            card_scale: frame.card_scale,
            slot_opacity: frame.slot_opacity,
        }
    }
}

/// The emptied slot as a recessed pocket: the veil fill, a light hairline
/// inside the edge and an accent glow outside it, all at `opacity`.
pub fn snapshot_slot(
    snapshot: &gtk::Snapshot,
    rect: &graphene::Rect,
    opacity: f64,
    palette: &Palette,
) {
    let rounded = gsk::RoundedRect::from_rect(*rect, palette.radius as f32);
    snapshot.append_outset_shadow(
        &rounded,
        &tint(palette.accent, SLOT_GLOW_OPACITY * opacity),
        0.,
        0.,
        0.,
        SLOT_GLOW_BLUR as f32,
    );
    snapshot.push_rounded_clip(&rounded);
    snapshot.append_color(&tint(palette.veil, SLOT_SHADE_OPACITY * opacity), rect);
    snapshot.pop();
    let line = tint(palette.light, SLOT_LINE_OPACITY * opacity);
    snapshot.append_border(&rounded, &[SLOT_LINE_WIDTH as f32; 4], &[line; 4]);
}

/// The ribbon passes, motes and an accepted drop's bloom, clipped to `clip`.
pub fn snapshot_trail(
    snapshot: &gtk::Snapshot,
    frame: &WakeFrame,
    palette: &Palette,
    clip: &graphene::Rect,
) {
    if frame.ribbons.is_empty() && frame.motes.is_empty() && frame.bloom.is_none() {
        return;
    }
    let (x, y, width, height) = frame.bounds;
    let bounds = graphene::Rect::new(x as f32, y as f32, width as f32, height as f32);
    let Some(area) = bounds.intersection(clip) else {
        return;
    };
    let cr = snapshot.append_cairo(&area);
    let rgb = |color: gdk::RGBA| {
        (
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
        )
    };
    for ribbon in &frame.ribbons {
        let (from, to) = (ribbon.from, ribbon.to);
        // A zero-length gradient would paint its last stop everywhere.
        if (to.0 - from.0).hypot(to.1 - from.1) < 0.5 {
            continue;
        }
        let Some((&(x0, y0), rest)) = ribbon.outline.split_first() else {
            continue;
        };
        cr.move_to(x0, y0);
        for &(x, y) in rest {
            cr.line_to(x, y);
        }
        cr.close_path();
        let gradient = gtk::cairo::LinearGradient::new(from.0, from.1, to.0, to.1);
        let (r, g, b) = rgb(palette.ink(ribbon.ink));
        for (offset, opacity) in ribbon.stops {
            gradient.add_color_stop_rgba(offset, r, g, b, opacity);
        }
        let _ = cr.set_source(&gradient);
        let _ = cr.fill();
    }
    let (ar, ag, ab) = rgb(palette.accent);
    let glow = |x: f64, y: f64, radius: f64, opacity: f64| {
        let gradient = gtk::cairo::RadialGradient::new(x, y, 0., x, y, radius);
        gradient.add_color_stop_rgba(0., ar, ag, ab, opacity);
        gradient.add_color_stop_rgba(1., ar, ag, ab, 0.);
        let _ = cr.set_source(&gradient);
        cr.arc(x, y, radius, 0., std::f64::consts::TAU);
        let _ = cr.fill();
    };
    let (lr, lg, lb) = rgb(palette.light);
    for mote in &frame.motes {
        glow(
            mote.x,
            mote.y,
            mote.radius * MOTE_HALO_SCALE,
            mote.opacity * MOTE_HALO_OPACITY,
        );
        cr.set_source_rgba(lr, lg, lb, mote.opacity);
        cr.arc(mote.x, mote.y, mote.radius, 0., std::f64::consts::TAU);
        let _ = cr.fill();
    }
    if let Some(bloom) = frame.bloom {
        glow(bloom.x, bloom.y, bloom.radius, bloom.opacity);
    }
}

fn now_ms() -> f64 {
    glib::monotonic_time() as f64 / 1000.
}

fn wake_rect(rect: graphene::Rect, offset: (f64, f64), radius: f64) -> WakeRect {
    WakeRect {
        x: f64::from(rect.x()) + offset.0,
        y: f64::from(rect.y()) + offset.1,
        width: f64::from(rect.width()),
        height: f64::from(rect.height()),
        radius,
    }
}

fn center(rect: &WakeRect) -> (f64, f64) {
    (rect.x + rect.width / 2., rect.y + rect.height / 2.)
}

/// One drag's wake, from drag begin until its end animation has played.
pub struct Wake {
    id: String,
    pile: glib::WeakRef<Pile>,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    drag: RefCell<DragWake>,
    /// From the pointer to the drag image's centre.
    tip_offset: (f64, f64),
    overlay: Option<Overlay>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Wake {
    /// Start `id`'s wake as its drag begins. `grab` is the press in the card
    /// (the drag icon's hotspot) and `icon` the drag image's size. `None`
    /// when the card has no slot in the pile.
    pub fn begin(
        state: &super::State,
        id: &str,
        grab: (f64, f64),
        icon: (f64, f64),
        drag: &gdk::Drag,
    ) -> Option<Rc<Wake>> {
        let pile = &state.pile;
        let local = pile.wake_slot(id)?;
        let palette = pile.wake_palette();
        #[cfg(feature = "x11")]
        let overlay = state
            .placer
            .as_ref()
            .and_then(|placer| Overlay::open(&state.window, placer, drag, palette));
        #[cfg(not(feature = "x11"))]
        let overlay: Option<Overlay> = {
            let _ = drag;
            None
        };
        let slot = match &overlay {
            Some(overlay) => overlay.slot(&state.window, pile, local, palette.radius),
            None => Some(wake_rect(local, (0., 0.), palette.radius)),
        }
        .unwrap_or_else(|| wake_rect(local, (0., 0.), palette.radius));
        let reduced = !state.window.settings().is_gtk_enable_animations();
        let wake = Rc::new(Wake {
            id: id.to_owned(),
            pile: pile.downgrade(),
            window: state.window.downgrade(),
            drag: RefCell::new(DragWake::new(slot, center(&slot), now_ms(), reduced)),
            tip_offset: (icon.0 / 2. - grab.0, icon.1 / 2. - grab.1),
            overlay,
            timer: RefCell::default(),
        });
        // The card starts leaving at once, then ~60 frames a second while
        // the wake lasts, mapped or not (an emptied stack hides its window).
        wake.tick();
        // The timer keeps the wake alive through its end, even when the
        // card (and its drag source) has already left the stack.
        let running = wake.clone();
        *wake.timer.borrow_mut() = Some(glib::timeout_add_local(
            Duration::from_millis(16),
            move || {
                let wake = &running;
                if wake.tick() {
                    return glib::ControlFlow::Continue;
                }
                // Ending inside its own callback: the source goes with Break.
                wake.timer.borrow_mut().take();
                wake.settle();
                glib::ControlFlow::Break
            },
        ));
        Some(wake)
    }

    /// The drag ended; the wake plays its end and then stops itself.
    pub fn end(&self, end: WakeEnd) {
        self.drag.borrow_mut().end(end, now_ms());
    }

    /// Stop at once (the same card started another drag).
    pub fn stop(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
            self.settle();
        }
    }

    /// Follow the slot and the pointer, then show one frame. False once
    /// the end has played.
    fn tick(&self) -> bool {
        let (Some(pile), Some(window)) = (self.pile.upgrade(), self.window.upgrade()) else {
            return false;
        };
        let radius = pile.wake_palette().radius;
        let local = pile.wake_slot(&self.id);
        let mut drag = self.drag.borrow_mut();
        match &self.overlay {
            Some(overlay) => {
                if let Some(slot) =
                    local.and_then(|local| overlay.slot(&window, &pile, local, radius))
                {
                    drag.set_slot(slot);
                }
                if let Some((x, y)) = overlay.pointer(&window) {
                    drag.pointer((x + self.tip_offset.0, y + self.tip_offset.1));
                }
            }
            // No pointer or surface beyond the window: the tip stays on the
            // slot, so no ribbon shows.
            None => {
                if let Some(local) = local {
                    let slot = wake_rect(local, (0., 0.), radius);
                    drag.set_slot(slot);
                    drag.pointer(center(&slot));
                }
            }
        }
        let Some(frame) = drag.frame(now_ms()) else {
            return false;
        };
        pile.set_wake(&self.id, WakePose::of(&frame));
        if let Some(overlay) = &self.overlay {
            overlay.show(frame);
        }
        true
    }

    /// The end has played: a returned card is back; a dropped one stays
    /// hidden until the stack lets it go.
    fn settle(&self) {
        if let Some(overlay) = &self.overlay {
            overlay.close();
        }
        let Some(pile) = self.pile.upgrade() else {
            return;
        };
        match self.drag.borrow().ended() {
            Some(WakeEnd::Dropped) => pile.set_wake(
                &self.id,
                WakePose {
                    card_opacity: 0.,
                    card_scale: 1.,
                    slot_opacity: 0.,
                },
            ),
            _ => pile.clear_wake(&self.id),
        }
    }
}

/// The composited X11 surface over the stack's monitor that draws the
/// ribbon: override-redirect (no frame, so its empty input shape is what
/// GTK's drag sees), transparent, out of the taskbar and pager, and below
/// the drag icon.
#[cfg(feature = "x11")]
struct Overlay {
    window: gtk::Window,
    area: crate::widgets::SnapshotArea,
    frame: Rc<RefCell<Option<WakeFrame>>>,
    placer: Rc<super::x11::Placer>,
    /// The monitor's top-left in desktop logical pixels.
    origin: (f64, f64),
}

#[cfg(feature = "x11")]
impl Overlay {
    fn open(
        stack: &gtk::ApplicationWindow,
        placer: &Rc<super::x11::Placer>,
        drag: &gdk::Drag,
        palette: Palette,
    ) -> Option<Overlay> {
        let display = WidgetExt::display(stack);
        if !display.is_composited() || !display.is_rgba() {
            return None;
        }
        let monitor = display.monitor_at_surface(&stack.surface()?)?;
        let (geometry, scale) = (monitor.geometry(), monitor.scale_factor());
        let frame = Rc::new(RefCell::new(None::<WakeFrame>));
        let shown = frame.clone();
        let area = crate::widgets::SnapshotArea::new(move |snapshot, width, height| {
            if let Some(frame) = shown.borrow().as_ref() {
                let clip = graphene::Rect::new(0., 0., width, height);
                snapshot_trail(snapshot, frame, &palette, &clip);
            }
        });
        area.set_can_target(false);
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.update_state(&[gtk::accessible::State::Hidden(true)]);
        let window = gtk::Window::builder()
            .title("Captures Drag Wake")
            .decorated(false)
            .resizable(false)
            .focusable(false)
            .build();
        window.add_css_class("captures");
        window.add_css_class("notice-window");
        window.set_child(Some(&area));
        window.set_default_size(geometry.width(), geometry.height());
        let device = (
            geometry.x() * scale,
            geometry.y() * scale,
            (geometry.width() * scale).max(1) as u32,
            (geometry.height() * scale).max(1) as u32,
        );
        let cover = placer.clone();
        window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            surface.set_input_region(&gtk::cairo::Region::create());
            if let Ok(surface) = surface.downcast::<gdk4_x11::X11Surface>() {
                surface.set_skip_taskbar_hint(true);
                surface.set_skip_pager_hint(true);
                cover.cover(&surface, device);
            }
        });
        // Mapped above everything, then the drag image goes back on top.
        let (raise, icon) = (
            placer.clone(),
            drag.drag_surface().and_downcast::<gdk4_x11::X11Surface>(),
        );
        window.connect_map(move |_| {
            if let Some(icon) = &icon {
                raise.raise(icon);
            }
        });
        window.set_visible(true);
        Some(Overlay {
            window,
            area,
            frame,
            placer: placer.clone(),
            origin: (f64::from(geometry.x()), f64::from(geometry.y())),
        })
    }

    /// The stack window's top-left on the overlay.
    fn stack_origin(&self, stack: &gtk::ApplicationWindow) -> Option<(f64, f64)> {
        let surface = stack.surface().and_downcast::<gdk4_x11::X11Surface>()?;
        let at = self.placer.geometry(&surface)?;
        let (x, y) = stack.surface_transform();
        Some((at.x + x - self.origin.0, at.y + y - self.origin.1))
    }

    /// The card's slot (`local`, in pile coordinates) on the overlay.
    fn slot(
        &self,
        stack: &gtk::ApplicationWindow,
        pile: &Pile,
        local: graphene::Rect,
        radius: f64,
    ) -> Option<WakeRect> {
        let (x, y) = self.stack_origin(stack)?;
        let corner = pile.compute_point(stack, &local.top_left())?;
        let rect = graphene::Rect::new(corner.x(), corner.y(), local.width(), local.height());
        Some(wake_rect(rect, (x, y), radius))
    }

    /// The pointer on the overlay, from the root window.
    fn pointer(&self, stack: &gtk::ApplicationWindow) -> Option<(f64, f64)> {
        let surface = stack.surface().and_downcast::<gdk4_x11::X11Surface>()?;
        let (x, y) = self.placer.pointer(&surface)?;
        Some((x - self.origin.0, y - self.origin.1))
    }

    fn show(&self, frame: WakeFrame) {
        *self.frame.borrow_mut() = Some(frame);
        self.area.queue_draw();
    }

    fn close(&self) {
        self.frame.borrow_mut().take();
        self.window.destroy();
    }
}

/// Never built without X11: there is no overlay to draw a ribbon in.
#[cfg(not(feature = "x11"))]
enum Overlay {}

#[cfg(not(feature = "x11"))]
impl Overlay {
    fn slot(
        &self,
        _: &gtk::ApplicationWindow,
        _: &Pile,
        _: graphene::Rect,
        _: f64,
    ) -> Option<WakeRect> {
        match *self {}
    }

    fn pointer(&self, _: &gtk::ApplicationWindow) -> Option<(f64, f64)> {
        match *self {}
    }

    fn show(&self, _: WakeFrame) {
        match *self {}
    }

    fn close(&self) {
        match *self {}
    }
}

/// `--scene preview --wake NAME --screenshot OUT.png`: the newest card's
/// wake as composited X11 shows it, on a 1280 × 800 desktop with the stack
/// in its bottom-right corner, after a scripted carry up and to the left.
/// `carried` is mid-carry (the drag image drawn at the tip), `returning`
/// is a cancel drawing the ribbon back while the card re-forms, and
/// `dropped` is an accepted drop snapping away with its bloom.
pub fn render_fixture(
    state: &super::State,
    path: PathBuf,
    phase: &str,
    light: bool,
    done: Box<dyn FnOnce()>,
) {
    let card = state.cards.borrow().last().cloned();
    let pile = state.pile.clone();
    let Some((card, local)) = card.and_then(|card| {
        let local = pile.wake_slot(&card.entry.id)?;
        Some((card, local))
    }) else {
        eprintln!("no preview card to render a wake for");
        done();
        return;
    };
    let palette = pile.wake_palette();
    // The drag image: the card's own image, drawn on its own.
    let still = gtk::WidgetPaintable::new(Some(&card.image));
    let icon = (
        f64::from(card.image.width()),
        f64::from(card.image.height()),
    );
    let canvas = graphene::Rect::new(0., 0., 1280., 800.);
    let (width, height) = (f64::from(pile.width()), f64::from(pile.height()));
    let origin = (
        f64::from(canvas.width()) - width,
        f64::from(canvas.height()) - height,
    );
    let slot = wake_rect(local, origin, palette.radius);
    let start = center(&slot);
    let carry = move |t: f64| {
        let p = (t / 900.).clamp(0., 1.);
        let eased = 1. - (1. - p).powi(3);
        (
            start.0 - 700. * eased,
            start.1 - 400. * eased + 150. * (p * std::f64::consts::PI).sin(),
        )
    };
    let (end, at) = match phase {
        "returning" => (Some(WakeEnd::Returned), 1_250.),
        "dropped" => (Some(WakeEnd::Dropped), 1_110.),
        _ => (None, 560.),
    };
    let mut wake = DragWake::new(slot, start, 0., false);
    let mut frame = None;
    let mut t: f64 = 0.;
    loop {
        let now = t.min(at);
        wake.pointer(carry(now));
        if let Some(end) = end
            && now >= 1_000.
        {
            wake.end(end, 1_000.);
        }
        frame = wake.frame(now).or(frame);
        if now >= at {
            break;
        }
        t += 16.;
    }
    let Some(frame) = frame else {
        done();
        return;
    };
    pile.set_wake(&card.entry.id, WakePose::of(&frame));
    let window = state.window.clone();
    let carried = end.is_none();
    glib::timeout_add_local_once(Duration::from_millis(200), move || {
        let snapshot = gtk::Snapshot::new();
        // A plain desktop under the stack, darker or lighter by appearance.
        let (top, bottom) = if light {
            ("#e8ebf0", "#b9c3cf")
        } else {
            ("#2a313b", "#151a21")
        };
        let stop = |offset: f32, color: &str| {
            gsk::ColorStop::new(offset, gdk::RGBA::parse(color).expect("backdrop colour"))
        };
        snapshot.append_linear_gradient(
            &canvas,
            &graphene::Point::new(0., 0.),
            &graphene::Point::new(canvas.width(), canvas.height()),
            &[stop(0., top), stop(1., bottom)],
        );
        snapshot.save();
        snapshot.translate(&graphene::Point::new(origin.0 as f32, origin.1 as f32));
        gtk::WidgetPaintable::new(Some(&pile)).snapshot(&snapshot, width, height);
        snapshot.restore();
        snapshot_trail(&snapshot, &frame, &palette, &canvas);
        if carried {
            let tip = carry(at);
            let (w, h) = icon;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (tip.0 - w / 2.) as f32,
                (tip.1 - h / 2.) as f32,
            ));
            still.snapshot(&snapshot, w, h);
            snapshot.restore();
        }
        let result = snapshot
            .to_node()
            .zip(window.renderer())
            .ok_or_else(|| "nothing to render".to_owned())
            .and_then(|(node, renderer)| {
                renderer
                    .render_texture(&node, Some(&canvas))
                    .save_to_png(&path)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            eprintln!("wake render failed: {error}");
        }
        done();
    });
}
