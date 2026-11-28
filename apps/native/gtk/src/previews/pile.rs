//! The preview stack's layout and motion (`.thumbnail-stack`): expanded
//! slots, the collapsed 3D pile and its hover fan, the cards' arrival, and the
//! fly between expanded and collapsed. Positions come from
//! `captures_app::preview` (`card_layout_in`, `collapsed_card_pose`) and
//! timing from `captures_app::motion`; each card is placed with a transform,
//! so moving one never re-renders its contents. With GTK animations off
//! (reduced motion) every change lands at once.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use captures_app::motion::{Animation, Motion, Transition, Tween};
use captures_app::preview::{self, CollapsedCardPose};
use captures_app::preview_drag_style::{
    self as drag_style, CardPose, DragEnd, DragOut, DragStyle, Reorder,
};
use captures_app::preview_motion::{self, DragSway, DustParticle, ExitKind, StackExits, StackFan};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::tokens::Tokens;
use crate::widgets::{Icon, MediaPose, SnapshotArea, Streak, Thumbnail, streak_step};

/// `THUMBNAIL_STACK_SCROLL_DURATION_MS`: an overflow cue's scroll.
const SCROLL_MS: f64 = 380.;

/// Shipping's 340 px window holds 284 px cards inside 28 px side padding.
pub const CARD_WIDTH: f64 = preview::THUMBNAIL_WIDTH - 2. * preview::THUMBNAIL_PADDING;
const WIDTH: f32 = CARD_WIDTH as f32;
const HEIGHT: f32 = preview::THUMBNAIL_CARD_HEIGHT as f32;

/// Told a card's id when its drag has played out.
type CardCallback = Box<dyn Fn(&str)>;
/// Told a settled reorder's card and its new chronological index.
type ReorderCallback = Box<dyn Fn(&str, usize)>;

/// One card the pile places.
pub struct PileCard {
    pub id: String,
    pub root: gtk::Widget,
    /// `glass-strong-solid` shade over a rear card's image.
    pub dim: gtk::Widget,
    pub image: Thumbnail,
    /// What a file drag leaves in the card's slot, over its image.
    pub imprint: Imprint,
}

impl PileCard {
    /// Chips, buttons and rings: every overlay but the image, the depth
    /// shade and the imprint.
    fn for_each_chrome(&self, mut f: impl FnMut(&gtk::Widget)) {
        let mut child = self.root.first_child();
        while let Some(widget) = child {
            if widget != *self.image.upcast_ref::<gtk::Widget>()
                && widget != self.dim
                && widget != *self.imprint.area.upcast_ref::<gtk::Widget>()
            {
                f(&widget);
            }
            child = widget.next_sibling();
        }
    }

    /// Show the card under `pose` (`None`: at rest).
    fn set_pose(&self, pose: Option<(CardPose, f64)>) {
        let media = pose.map(|(pose, hover)| MediaPose {
            blur: pose.media_blur as f32,
            opacity: pose.media_opacity as f32,
            scale: pose.media_scale as f32,
            hover: hover as f32,
        });
        self.image.set_media(media);
        self.imprint.set(pose.map(|(pose, _)| pose));
        let chrome = pose.map_or(1., |(pose, _)| pose.chrome_opacity);
        self.for_each_chrome(|widget| {
            if widget.opacity() != chrome {
                widget.set_opacity(chrome);
            }
        });
    }
}

/// The imprint a file drag leaves in a card's slot: the hairline inside its
/// edge, the well's veil and inner shadow, and the "out" glyph in its disc
/// (`preview_drag_style`'s constants), inside the card's rounded clip.
#[derive(Clone)]
pub struct Imprint {
    pub area: SnapshotArea,
    pose: std::rc::Rc<Cell<Option<CardPose>>>,
}

impl Imprint {
    pub fn new(tokens: &Tokens) -> Self {
        let pose = std::rc::Rc::new(Cell::new(None::<CardPose>));
        let (text, veil) = (tokens.rgba("glass-text"), tokens.rgba("glass-veil"));
        let radius = tokens.number("thumbnail-card-radius");
        let shown = pose.clone();
        let area = SnapshotArea::new(move |snapshot, w, h| {
            if let Some(pose) = shown.get() {
                draw_imprint(snapshot, (w, h), radius, (text, veil), &pose, 1.);
            }
        });
        area.set_can_target(false);
        area.update_state(&[gtk::accessible::State::Hidden(true)]);
        Self { area, pose }
    }

    fn set(&self, pose: Option<CardPose>) {
        // Only what the imprint draws: hairline, well and glyph.
        let drawn = pose.filter(|pose| pose.hairline > 0. || pose.well > 0. || pose.glyph > 0.);
        if self.pose.replace(drawn) != drawn {
            self.area.queue_draw();
        }
    }
}

fn alpha(color: gtk::gdk::RGBA, alpha: f64) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::new(color.red(), color.green(), color.blue(), alpha as f32)
}

/// Draw `pose`'s imprint over a `w` × `h` card (inside its rounded clip):
/// the well's veil and inner shadow, the hairline and the glyph, which
/// Preferences' small demo cards draw at `glyph_size` × its full size.
pub fn draw_imprint(
    snapshot: &gtk::Snapshot,
    (w, h): (f32, f32),
    radius: f32,
    (text, veil): (gtk::gdk::RGBA, gtk::gdk::RGBA),
    pose: &CardPose,
    glyph_size: f64,
) {
    let corner = graphene::Size::new(radius, radius);
    let outline = gsk::RoundedRect::new(
        graphene::Rect::new(0., 0., w, h),
        corner,
        corner,
        corner,
        corner,
    );
    if pose.well > 0. {
        snapshot.append_color(
            &alpha(veil, drag_style::WELL_FILL_OPACITY * pose.well),
            &graphene::Rect::new(0., 0., w, h),
        );
        let shadow = drag_style::WELL_INNER_SHADOW_OPACITY * pose.well;
        snapshot.append_inset_shadow(
            &outline,
            &alpha(gtk::gdk::RGBA::BLACK, shadow),
            0.,
            1.,
            0.,
            drag_style::WELL_INNER_SHADOW_BLUR as f32,
        );
    }
    if pose.hairline > 0. {
        let line = alpha(text, drag_style::HAIRLINE_OPACITY * pose.hairline);
        let width = drag_style::HAIRLINE_WIDTH as f32;
        snapshot.append_border(&outline, &[width; 4], &[line; 4]);
    }
    if pose.glyph > 0. {
        let (cx, cy) = (f64::from(w) / 2., f64::from(h) / 2.);
        let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
        cr.translate(cx, cy);
        cr.scale(pose.glyph_scale * glyph_size, pose.glyph_scale * glyph_size);
        cr.arc(
            0.,
            0.,
            drag_style::GLYPH_DIAMETER / 2.,
            0.,
            std::f64::consts::TAU,
        );
        cr.set_source_rgba(1., 1., 1., drag_style::GLYPH_DISC_OPACITY * pose.glyph);
        let _ = cr.fill();
        let scale = drag_style::GLYPH_SIZE / 24.;
        cr.scale(scale, scale);
        cr.translate(-12., -12.);
        for line in captures_app::icons::flatten(drag_style::GLYPH_PATH) {
            let mut points = line.iter();
            if let Some(first) = points.next() {
                cr.move_to(first[0].into(), first[1].into());
                for point in points {
                    cr.line_to(point[0].into(), point[1].into());
                }
            }
        }
        cr.set_line_width(drag_style::GLYPH_STROKE);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.set_source_rgba(
            text.red().into(),
            text.green().into(),
            text.blue().into(),
            drag_style::GLYPH_OPACITY * pose.glyph,
        );
        let _ = cr.stroke();
    }
}

/// A card's file drag out of the stack, until its end has played.
pub struct FileDrag {
    id: String,
    model: DragOut,
    began: f64,
    /// The card wore its hover look when the drag began; it fades out.
    hovered: bool,
}

/// The hover look's fade as a drag begins (`.preview-overlay`'s 160 ms).
const HOVER_FADE_MS: f64 = 160.;

/// A card carried up or down the stack.
pub struct Carried {
    id: String,
    model: Reorder,
    /// The latest frame: each card's offset (chronological) and the lift.
    offsets: Vec<f64>,
    lift: f64,
    /// The latest frame moved something.
    moving: bool,
    /// The carried card's resting top in the pile when it was picked up.
    home_top: f64,
}

/// Take the drags whose end has played by `now`: the others keep their
/// card in its slot, posed.
fn take_finished(drags: &mut Vec<FileDrag>, now: f64) -> Vec<(String, DragEnd)> {
    let done = drags
        .iter()
        .filter(|drag| drag.model.finished(now))
        .filter_map(|drag| Some((drag.id.clone(), drag.model.ended()?)))
        .collect();
    drags.retain(|drag| !drag.model.finished(now));
    done
}

/// Each card's y offset while file drags close their slots: cards older
/// than a closing slot (`(index, slot)`) move toward the anchored edge.
pub fn closing_offsets(count: usize, closing: &[(usize, f64)], top: bool) -> Vec<f64> {
    (0..count)
        .map(|index| {
            closing
                .iter()
                .filter(|(dragged, _)| index < *dragged)
                .map(|(_, slot)| drag_style::closing_shift(*slot, top))
                .sum()
        })
        .collect()
}

/// Move `from` to `to` in a chronological list, as
/// `PreviewStack::move_card` does to the membership.
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from < items.len() {
        let item = items.remove(from);
        items.insert(to.min(items.len()), item);
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Pile {
        /// Chronological, oldest first: the newest paints on top.
        pub cards: RefCell<Vec<PileCard>>,
        pub controls: RefCell<Option<gtk::Widget>>,
        /// Shipping `.thumbnail-collapsed-hit-target`: one button over the
        /// collapsed pile that expands it.
        pub expand: RefCell<Option<gtk::Button>>,
        pub top_anchor: Cell<bool>,
        pub collapsed: Cell<bool>,
        /// Collapse progress when the latest fly began, and when.
        pub fly: Cell<(f64, f64)>,
        pub fan: RefCell<StackFan>,
        pub hovered: Cell<bool>,
        /// Arrival start per card id.
        pub arrivals: RefCell<HashMap<String, f64>>,
        pub motion: RefCell<Option<(Tween, Tween, Animation)>>,
        pub ticking: Cell<bool>,
        pub on_expand: RefCell<Option<Box<dyn Fn()>>>,
        pub right_anchor: Cell<bool>,
        /// Cards playing an exit keep their slot here until it ends.
        pub exits: RefCell<StackExits>,
        /// A Delete's dust chips, by card id.
        pub dust: RefCell<HashMap<String, Vec<DustParticle>>>,
        /// Each card's top-left in the pile at the latest layout.
        pub origins: RefCell<HashMap<String, (f32, f32)>>,
        pub exit_motion: Cell<Option<super::ExitMotion>>,
        /// An exit ended and its card left the pile.
        pub on_exited: RefCell<Option<Box<dyn Fn()>>>,
        /// A carried pile's gravity (`None`: ±1 from its corner).
        pub gravity: Cell<Option<f64>>,
        /// Shipping's hit-target sparkles: the two layers' motion, the
        /// accent dot colour, and when the hover that plays them began.
        pub sparkle: RefCell<Option<(Animation, Animation, gtk::gdk::RGBA)>>,
        pub sparkle_since: Cell<Option<f64>>,
        /// The stack toolbar's In, Out, Exit and Clear motion, and the one
        /// playing with its start.
        pub toolbar_motion: RefCell<Option<[Animation; 4]>>,
        pub toolbar: Cell<Option<(super::ToolbarPhase, f64)>>,
        pub carry: Cell<super::Carry>,
        /// The tallest the expanded stack may be (the work area's room).
        pub max_height: Cell<Option<f64>>,
        /// An expanded stack's scroll (`scrollTop`) easing from, to, since;
        /// `None` keeps the newest card in view.
        pub scroll: Cell<Option<(f64, f64, f64)>>,
        /// The scroll range at the latest layout.
        pub max_scroll: Cell<f64>,
        /// `.thumbnail-overflow-cue-older` and `-newer`.
        pub cues: RefCell<Vec<gtk::Button>>,
        /// File drags out of the stack, until their ends have played.
        pub drags: RefCell<Vec<super::FileDrag>>,
        /// A card carried up or down the stack.
        pub carried: RefCell<Option<super::Carried>>,
        /// A dropped file's slot has closed: the stack lets its card go.
        pub on_dropped: RefCell<Option<super::CardCallback>>,
        /// A file drag's card is back at rest.
        pub on_returned: RefCell<Option<super::CardCallback>>,
        /// A reorder settled: the card now sits at this chronological index.
        pub on_reordered: RefCell<Option<super::ReorderCallback>>,
        /// `--drag` fixtures: a still clock for drags and reorders.
        pub drag_clock: Cell<Option<f64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Pile {
        const NAME: &'static str = "CapturesPreviewPile";
        type Type = super::Pile;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for Pile {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Pile {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = if orientation == gtk::Orientation::Horizontal {
                preview::THUMBNAIL_WIDTH
            } else {
                let obj = self.obj();
                let count = self.cards.borrow().len();
                let (_, since) = self.fly.get();
                let now = obj.now_ms();
                // Shipping retains the frame through the fly, so the
                // window resizes and moves once, not on every frame.
                let height = if obj.tweens().0.running(now - since, obj.reduced()) {
                    preview::stack_height(count).max(preview::collapsed_frame_height(count))
                } else {
                    obj.content_height(obj.collapse_progress(now))
                };
                // A stack taller than the work area scrolls inside it.
                self.max_height
                    .get()
                    .map_or(height, |max| height.min(max))
                    .max(preview::collapsed_frame_height(count))
            };
            let size = size.round() as i32;
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            self.obj().place(width as f32, height as f32);
        }

        /// Children in order; a dissolving card's dust paints right above
        /// it, outside its clip, and a carried card above the other cards.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let carried = self.carried.borrow().as_ref().and_then(|carried| {
                self.cards
                    .borrow()
                    .iter()
                    .find(|card| card.id == carried.id)
                    .map(|card| card.root.clone())
            });
            let mut held = carried.clone();
            let mut child = obj.first_child();
            while let Some(widget) = child {
                if carried.as_ref() == Some(&widget) {
                    child = widget.next_sibling();
                    continue;
                }
                let card = self.cards.borrow().iter().any(|card| card.root == widget);
                if !card && let Some(carried) = held.take() {
                    obj.snapshot_child(&carried, snapshot);
                }
                if self.controls.borrow().as_ref() == Some(&widget) {
                    obj.snapshot_toolbar(&widget, snapshot);
                } else {
                    obj.snapshot_child(&widget, snapshot);
                }
                let id = self
                    .cards
                    .borrow()
                    .iter()
                    .find(|card| card.root == widget)
                    .map(|card| card.id.clone());
                if let Some(id) = id {
                    obj.snapshot_dust(&id, snapshot);
                }
                if imp_is_expand(self, &widget) {
                    obj.snapshot_sparkles(snapshot);
                }
                child = widget.next_sibling();
            }
            if let Some(carried) = held {
                obj.snapshot_child(&carried, snapshot);
            }
        }
    }
}

/// The collapsed pile being carried (shipping `CollapsedThumbnailStackDrag`
/// with its sway): the fan holds until it has gathered, then the rear cards
/// lean with `DragSway`; dropping eases the lean back over the fan's
/// staggered transition.
#[derive(Clone, Copy, Debug, Default)]
pub struct Carry {
    /// When the press became a carry, until the drop.
    since: Option<f64>,
    /// The lean is live.
    ready: bool,
    sway: DragSway,
    /// The latest desktop pointer, and the one the sway last sampled.
    pointer: Option<(f64, f64)>,
    last: Option<((f64, f64), f64)>,
    /// The lean at the drop, and when it began easing back.
    released: Option<((f64, f64), f64)>,
}

/// Exit timings, resolved once from the tokens.
#[derive(Clone, Copy)]
pub struct ExitMotion {
    dismiss: Animation,
    streak: Animation,
    fallback: Animation,
}

glib::wrapper! {
    pub struct Pile(ObjectSubclass<imp::Pile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

fn mix(rest: CollapsedCardPose, fan: CollapsedCardPose, t: f64) -> CollapsedCardPose {
    CollapsedCardPose {
        dx: lerp(rest.dx, fan.dx, t),
        dy: lerp(rest.dy, fan.dy, t),
        slot_dy: lerp(rest.slot_dy, fan.slot_dy, t),
        rotation_deg: lerp(rest.rotation_deg, fan.rotation_deg, t),
        scale_x: lerp(rest.scale_x, fan.scale_x, t),
        scale_y: lerp(rest.scale_y, fan.scale_y, t),
        projection: rest.projection,
        media_blur: lerp(rest.media_blur, fan.media_blur, t),
    }
}

/// Shipping's `thumbnail-stack-toolbar-entering`, `-leaving`, `-exiting`
/// and `-clearing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarPhase {
    In,
    Out,
    Exit,
    Clear,
}

/// Whether `widget` is the collapsed pile's hit target.
fn imp_is_expand(imp: &imp::Pile, widget: &gtk::Widget) -> bool {
    imp.expand
        .borrow()
        .as_ref()
        .is_some_and(|expand| expand.upcast_ref::<gtk::Widget>() == widget)
}

impl Pile {
    pub fn new(tokens: &Tokens, controls: &impl IsA<gtk::Widget>) -> Pile {
        let pile: Pile = glib::Object::new();
        let imp = pile.imp();
        let toolbar = |motion: Motion| motion.resolve(tokens).expect("stack toolbar tokens");
        *imp.toolbar_motion.borrow_mut() = Some([
            toolbar(Motion::PreviewToolbarIn),
            toolbar(Motion::PreviewToolbarOut),
            toolbar(Motion::PreviewToolbarExit),
            toolbar(Motion::PreviewToolbarClear),
        ]);
        *imp.sparkle.borrow_mut() = Some((
            Motion::PreviewPileSparkle
                .resolve(tokens)
                .expect("pile sparkle tokens"),
            Motion::PreviewPileSparkleLate
                .resolve(tokens)
                .expect("pile sparkle tokens"),
            tokens.rgba("theme-accent"),
        ));
        *imp.motion.borrow_mut() = Some((
            Transition::PreviewStackFly
                .resolve(tokens)
                .expect("stack fly tokens"),
            Transition::PreviewStackFan
                .resolve(tokens)
                .expect("stack fan tokens"),
            Motion::PreviewCardArrive
                .resolve(tokens)
                .expect("card arrive tokens"),
        ));
        let animation = |motion: Motion| motion.resolve(tokens).expect("preview exit tokens");
        imp.exit_motion.set(Some(ExitMotion {
            dismiss: animation(Motion::PreviewDismiss),
            streak: animation(Motion::PreviewDismissStreak),
            fallback: animation(Motion::PreviewDeleteFallback),
        }));
        controls.set_parent(&pile);
        *imp.controls.borrow_mut() = Some(controls.clone().upcast());

        // Overflow cues paint over the cards and under the stack toolbar.
        for (older, icon) in [(true, "chevron-up"), (false, "chevron-down")] {
            let cue = gtk::Button::new();
            cue.set_cursor_from_name(Some("pointer"));
            cue.add_css_class("preview-overflow-cue");
            cue.add_css_class(if older { "older" } else { "newer" });
            cue.set_child(Some(&Icon::new(icon, 16).with_stroke(2.)));
            cue.set_opacity(0.);
            cue.set_can_target(false);
            cue.set_focusable(false);
            cue.insert_before(&pile, Some(controls));
            let weak = pile.downgrade();
            cue.connect_clicked(move |_| {
                if let Some(pile) = weak.upgrade() {
                    let slot = preview::THUMBNAIL_CARD_SLOT;
                    pile.scroll_by(if older { -slot } else { slot }, true);
                }
            });
            imp.cues.borrow_mut().push(cue);
        }
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = pile.downgrade();
        wheel.connect_scroll(move |_, _, dy| {
            let Some(pile) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if pile.imp().collapsed.get() || pile.imp().max_scroll.get() <= 0. {
                return glib::Propagation::Proceed;
            }
            // GTK's own wheel step: the viewport height to the 2/3 power.
            let step = f64::from(pile.height()).powf(2. / 3.);
            pile.scroll_by(dy * step, false);
            glib::Propagation::Stop
        });
        pile.add_controller(wheel);

        let expand = gtk::Button::new();
        expand.set_cursor_from_name(Some("pointer"));
        expand.add_css_class("preview-pile-expand");
        expand.set_visible(false);
        expand.set_parent(&pile);
        let weak = pile.downgrade();
        expand.connect_clicked(move |_| {
            if let Some(pile) = weak.upgrade()
                && let Some(expand) = pile.imp().on_expand.borrow().as_ref()
            {
                expand();
            }
        });
        *imp.expand.borrow_mut() = Some(expand);

        // Hovering the collapsed pile fans it.
        let hover = gtk::EventControllerMotion::new();
        let weak = pile.downgrade();
        hover.connect_enter(move |_, _, _| {
            if let Some(pile) = weak.upgrade() {
                pile.set_hovered(true);
            }
        });
        let weak = pile.downgrade();
        hover.connect_leave(move |_| {
            if let Some(pile) = weak.upgrade() {
                pile.set_hovered(false);
            }
        });
        pile.add_controller(hover);
        pile
    }

    pub fn connect_expand(&self, expand: impl Fn() + 'static) {
        *self.imp().on_expand.borrow_mut() = Some(Box::new(expand));
    }

    /// The tallest the expanded stack may be before it scrolls.
    pub fn set_max_height(&self, max: Option<f64>) {
        if self.imp().max_height.replace(max) != max {
            self.queue_resize();
        }
    }

    /// The expanded stack's scroll now, clamped to the latest range.
    fn scroll_top(&self, now: f64, max_scroll: f64) -> f64 {
        let imp = self.imp();
        let top = match imp.scroll.get() {
            None if imp.top_anchor.get() => 0.,
            None => max_scroll,
            Some((from, to, since)) => {
                let t = if self.reduced() {
                    1.
                } else {
                    ((now - since) / SCROLL_MS).clamp(0., 1.)
                };
                from + (to - from) * (1. - (1. - t).powi(3))
            }
        };
        top.clamp(0., max_scroll)
    }

    /// Scroll the expanded stack by `delta` points; a cue eases there
    /// (`animateThumbnailStackScroll`), the wheel moves at once.
    pub fn scroll_by(&self, delta: f64, animate: bool) {
        let imp = self.imp();
        let (now, max) = (self.now_ms(), imp.max_scroll.get());
        let from = self.scroll_top(now, max);
        let to = (from + delta).clamp(0., max);
        imp.scroll.set(Some(if animate {
            (from, to, now)
        } else {
            (to, to, now)
        }));
        self.animate();
    }

    /// Bring `id`'s card fully into view (keyboard focus moved to it).
    pub fn reveal(&self, id: &str) {
        let imp = self.imp();
        if imp.collapsed.get() {
            return;
        }
        let (count, index) = {
            let cards = imp.cards.borrow();
            let Some(index) = cards.iter().position(|card| card.id == id) else {
                return;
            };
            (cards.len(), index)
        };
        let top = imp.top_anchor.get();
        let Some(layout) = preview::card_layout_in(count, index, false, top, false) else {
            return;
        };
        let (max, viewport) = (imp.max_scroll.get(), f64::from(self.height()));
        let scroll = self.scroll_top(self.now_ms(), max);
        // Clear of the toolbar gutter on the anchored edge.
        let gutter = preview::THUMBNAIL_CONTROL_GUTTER;
        let (lo, hi) = if top {
            (gutter, viewport)
        } else {
            (0., viewport - gutter)
        };
        let (start, end) = (
            layout.y - scroll,
            layout.y + preview::THUMBNAIL_CARD_HEIGHT - scroll,
        );
        if start < lo {
            self.scroll_by(start - lo, true);
        } else if end > hi {
            self.scroll_by(end - hi, true);
        }
    }

    /// The collapsed pile's hit target, which a carry starts on.
    pub fn expand_button(&self) -> gtk::Button {
        self.imp().expand.borrow().clone().expect("expand button")
    }

    /// A carried pile's gravity, or `None` for its corner's.
    pub fn set_gravity(&self, gravity: Option<f64>) {
        if self.imp().gravity.replace(gravity) != gravity {
            self.queue_allocate();
        }
    }

    pub fn carry_start(&self) {
        let imp = self.imp();
        imp.carry.set(Carry {
            since: Some(self.now_ms()),
            ..Carry::default()
        });
        imp.hovered.set(true);
        self.set_sparkling(false);
        self.animate();
    }

    /// The pointer on the desktop, in logical pixels.
    #[cfg_attr(not(feature = "x11"), allow(dead_code))]
    pub fn carry_pointer(&self, x: f64, y: f64) {
        let imp = self.imp();
        let mut carry = imp.carry.get();
        carry.pointer = Some((x, y));
        imp.carry.set(carry);
        self.animate();
    }

    pub fn carry_end(&self) {
        let imp = self.imp();
        let lean = imp.carry.get().sway.offset();
        imp.carry.set(Carry {
            released: (lean != (0., 0.)).then_some((lean, self.now_ms())),
            ..Carry::default()
        });
        self.animate();
    }

    /// Advance the lean one frame: a still pointer is a zero step, so the
    /// spring settles when the carry pauses.
    fn tick_carry(&self) {
        let imp = self.imp();
        let mut carry = imp.carry.get();
        let Some(since) = carry.since else { return };
        let (now, reduced) = (self.now_ms(), self.reduced());
        let deepest = imp.cards.borrow().len().saturating_sub(1);
        let gather = StackFan::settle_ms(deepest, &self.tweens().1);
        if !carry.ready && (reduced || now - since >= gather) {
            carry.ready = true;
            carry.sway = DragSway::default();
            carry.last = carry.pointer.map(|point| (point, now));
        }
        if carry.ready
            && let Some(point) = carry.pointer
        {
            let (step, dt) = carry.last.map_or(
                ((0., 0.), preview_motion::DRAG_SWAY_DEFAULT_DT_MS),
                |(last, at)| ((point.0 - last.0, point.1 - last.1), now - at),
            );
            carry.sway.tick(step.0, step.1, dt, reduced);
            carry.last = Some((point, now));
        }
        imp.carry.set(carry);
    }

    /// Each depth's lean now.
    fn lean(&self, depth: usize, now: f64, tween: &Tween, reduced: bool) -> (f64, f64) {
        let carry = self.imp().carry.get();
        if carry.ready {
            return carry.sway.offset();
        }
        let Some(((x, y), at)) = carry.released else {
            return (0., 0.);
        };
        let left = 1. - tween.progress(now - at - preview_motion::fan_delay_ms(depth), reduced);
        (x * left, y * left)
    }

    fn carry_moving(&self, now: f64, deepest: usize, tween: &Tween, reduced: bool) -> bool {
        let imp = self.imp();
        let mut carry = imp.carry.get();
        if let Some((_, at)) = carry.released {
            if tween.running(now - at - preview_motion::fan_delay_ms(deepest), reduced) {
                return true;
            }
            carry.released = None;
            imp.carry.set(carry);
        }
        (carry.since.is_some() && !carry.ready) || (carry.ready && !carry.sway.settled())
    }

    pub fn connect_exited(&self, exited: impl Fn() + 'static) {
        *self.imp().on_exited.borrow_mut() = Some(Box::new(exited));
    }

    /// No card left, exiting or not.
    pub fn is_empty(&self) -> bool {
        self.imp().cards.borrow().is_empty()
    }

    pub fn is_top_anchor(&self) -> bool {
        self.imp().top_anchor.get()
    }

    /// Close streaks and the dust origin mirror on right-anchored stacks.
    pub fn set_right_anchor(&self, right: bool) {
        self.imp().right_anchor.set(right);
    }

    fn live_ids(&self) -> Vec<String> {
        let exits = self.imp().exits.borrow();
        self.imp()
            .cards
            .borrow()
            .iter()
            .filter(|card| exits.exiting(&card.id).is_none())
            .map(|card| card.id.clone())
            .collect()
    }

    /// Play `id`'s shipping exit while it keeps its slot, then drop it.
    /// False when it should leave at once: reduced motion, a collapsed or
    /// flying pile, or an unknown card. `saved` places the dust origin.
    pub fn begin_exit(
        &self,
        id: &str,
        kind: ExitKind,
        delay_ms: f64,
        settles: bool,
        saved: bool,
    ) -> bool {
        let imp = self.imp();
        let now = self.now_ms();
        let (_, since) = imp.fly.get();
        if self.reduced() || imp.collapsed.get() || self.tweens().0.running(now - since, false) {
            return false;
        }
        let live = self.live_ids();
        let image = {
            let cards = imp.cards.borrow();
            let Some(card) = cards.iter().find(|card| card.id == id) else {
                return false;
            };
            card.root.set_can_target(false);
            card.image.clone()
        };
        let size = image.texture_size();
        let kind = match (kind, size) {
            (ExitKind::Dust, None) => ExitKind::DeleteFallback,
            _ => kind,
        };
        let begun = imp.exits.borrow_mut().begin(
            &live,
            id,
            kind,
            now,
            delay_ms,
            settles,
            &preview_motion::settle_tween(),
        );
        if !begun {
            return false;
        }
        if let (ExitKind::Dust, Some(size)) = (kind, size) {
            let origin = preview_motion::delete_origin_x(CARD_WIDTH, saved, imp.right_anchor.get());
            let seed = id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            });
            let particles = preview_motion::dust_particles(
                CARD_WIDTH,
                preview::THUMBNAIL_CARD_HEIGHT,
                size,
                (origin, preview_motion::DELETE_ORIGIN_Y),
                seed,
            );
            imp.dust.borrow_mut().insert(id.to_owned(), particles);
            if let Some(card) = imp.cards.borrow().iter().find(|card| card.id == id) {
                card.root.add_css_class("exit-dust");
            }
        }
        self.update_cards();
        self.animate();
        true
    }

    /// The clock drags and reorders run on; `--drag` fixtures hold it.
    fn drag_now(&self) -> f64 {
        self.imp().drag_clock.get().unwrap_or_else(|| self.now_ms())
    }

    /// `--drag` fixtures: hold every drag and reorder at `at` milliseconds.
    pub fn set_drag_clock(&self, at: Option<f64>) {
        self.imp().drag_clock.set(at);
        self.animate();
    }

    pub fn connect_dropped(&self, dropped: impl Fn(&str) + 'static) {
        *self.imp().on_dropped.borrow_mut() = Some(Box::new(dropped));
    }

    pub fn connect_returned(&self, returned: impl Fn(&str) + 'static) {
        *self.imp().on_returned.borrow_mut() = Some(Box::new(returned));
    }

    pub fn connect_reordered(&self, reordered: impl Fn(&str, usize) + 'static) {
        *self.imp().on_reordered.borrow_mut() = Some(Box::new(reordered));
    }

    fn card_root(&self, id: &str) -> Option<gtk::Widget> {
        self.imp()
            .cards
            .borrow()
            .iter()
            .find(|card| card.id == id)
            .map(|card| card.root.clone())
    }

    /// `id`'s file left the stack: its slot shows `style` until the drag
    /// ends. A collapsed pile has no slot to show it in.
    pub fn drag_out_begin(&self, id: &str, style: DragStyle, hovered: bool) {
        let imp = self.imp();
        if imp.collapsed.get() {
            return;
        }
        let Some(root) = self.card_root(id) else {
            return;
        };
        let now = self.drag_now();
        let mut drags = imp.drags.borrow_mut();
        drags.retain(|drag| drag.id != id);
        drags.push(FileDrag {
            id: id.to_owned(),
            model: DragOut::new(style, now, self.reduced()),
            began: now,
            hovered: hovered && !self.reduced(),
        });
        drop(drags);
        root.remove_css_class("drag-returning");
        // The frame's shadow and rings leave with the chrome.
        if style != DragStyle::Dock {
            root.add_css_class("drag-imprint");
        }
        self.animate();
    }

    /// `id`'s file drag ended: its card returns, or leaves and its slot
    /// closes (then [`Self::connect_dropped`] lets it go). False when no
    /// drag of it was showing.
    pub fn drag_out_end(&self, id: &str, end: DragEnd) -> bool {
        let imp = self.imp();
        let now = self.drag_now();
        let style = {
            let mut drags = imp.drags.borrow_mut();
            let Some(drag) = drags
                .iter_mut()
                .find(|drag| drag.id == id && drag.model.ended().is_none())
            else {
                return false;
            };
            drag.model.end(end, now);
            drag.model.style()
        };
        // A slot it opened while hovering the stack closes again.
        if let Some(carried) = imp
            .carried
            .borrow_mut()
            .as_mut()
            .filter(|carried| carried.id == id)
        {
            carried.model.cancel(now);
            carried.moving = true;
        }
        if end == DragEnd::Returned
            && style != DragStyle::Dock
            && let Some(root) = self.card_root(id)
        {
            root.remove_css_class("drag-imprint");
            root.add_css_class("drag-returning");
        }
        self.update_cards();
        self.animate();
        true
    }

    /// The card whose file is out is back over the stack at pile `y`, or
    /// left it (`None`): the card eases back into view and the cards it
    /// would pass open the slot under the pointer
    /// (`Reorder::hovering`), or it eases back into its imprint and the
    /// stack closes up again.
    pub fn drag_out_hover(&self, y: Option<f64>) {
        let imp = self.imp();
        let now = self.drag_now();
        let Some((id, style)) = imp.drags.borrow_mut().iter_mut().find_map(|drag| {
            drag.model.ended().is_none().then(|| {
                drag.model.hover(y.is_some(), now);
                (drag.id.clone(), drag.model.style())
            })
        }) else {
            return;
        };
        if style != DragStyle::Dock
            && let Some(root) = self.card_root(&id)
        {
            let (on, off) = if y.is_some() {
                ("drag-returning", "drag-imprint")
            } else {
                ("drag-imprint", "drag-returning")
            };
            root.remove_css_class(off);
            root.add_css_class(on);
        }
        let count = imp.cards.borrow().len();
        let from = imp.cards.borrow().iter().position(|card| card.id == id);
        let home_top = imp
            .origins
            .borrow()
            .get(&id)
            .map(|&(_, top)| f64::from(top));
        if let (Some(from), Some(home_top)) = (from, home_top) {
            let top = imp.top_anchor.get();
            let content = home_top - Reorder::slot_top(count, from, top);
            let to = y.map_or(from, |y| drag_style::drop_index(count, y - content, top));
            let settled = !imp.collapsed.get() && imp.exits.borrow().is_empty();
            let mut carried = imp.carried.borrow_mut();
            if carried.is_none() && settled && to != from {
                let mut model = Reorder::hovering(count, from, top, now, self.reduced());
                let offsets = self.take_slot(&id, &mut model, now);
                *carried = Some(Carried {
                    id: id.clone(),
                    model,
                    offsets,
                    lift: 0.,
                    moving: true,
                    home_top,
                });
            }
            if let Some(carried) = carried.as_mut().filter(|carried| carried.id == id) {
                carried.model.aim(to);
                carried.moving = true;
            }
        }
        self.animate();
    }

    /// `id`'s file was dropped back on the stack at pile `y`: the card
    /// returns into the slot under the pointer, as if carried there. A
    /// dock whose gap has closed moves it at once and reopens the gap
    /// there; otherwise the cards it passes glide out of its way while it
    /// returns. False when no drag of it was showing.
    pub fn drag_out_land(&self, id: &str, y: f64) -> bool {
        let imp = self.imp();
        let now = self.drag_now();
        let Some(closed) = imp
            .drags
            .borrow()
            .iter()
            .find(|drag| drag.id == id && drag.model.ended().is_none())
            .map(|drag| drag.model.slot_closed(now))
        else {
            return false;
        };
        let count = imp.cards.borrow().len();
        let from = imp.cards.borrow().iter().position(|card| card.id == id);
        let home_top = imp.origins.borrow().get(id).map(|&(_, top)| f64::from(top));
        let top = imp.top_anchor.get();
        // Hovering, the stack already opened a slot: land there.
        let hovering = imp.carried.borrow_mut().take_if(|carried| carried.id == id);
        if let Some(mut carried) = hovering {
            if let (Some(from), Some(home_top)) = (from, home_top) {
                let content = home_top - Reorder::slot_top(count, from, top);
                carried
                    .model
                    .aim(drag_style::drop_index(count, y - content, top));
            }
            carried.model.drop(now);
            carried.moving = true;
            self.drag_out_end(id, DragEnd::Returned);
            *imp.carried.borrow_mut() = Some(carried);
            self.animate();
            return true;
        }
        let to = from.zip(home_top).and_then(|(from, home_top)| {
            let content = home_top - Reorder::slot_top(count, from, top);
            let to = drag_style::drop_index(count, y - content, top);
            let settled = !imp.collapsed.get()
                && imp.exits.borrow().is_empty()
                && imp.carried.borrow().is_none();
            (settled && to != from).then_some((from, to, home_top))
        });
        self.drag_out_end(id, DragEnd::Returned);
        let Some((from, to, home_top)) = to else {
            return true;
        };
        if closed {
            self.commit_move(id, from, to);
        } else {
            let mut model = Reorder::landing(count, from, to, top, now, self.reduced());
            let offsets = self.take_slot(id, &mut model, now);
            *imp.carried.borrow_mut() = Some(Carried {
                id: id.to_owned(),
                model,
                offsets,
                lift: 0.,
                moving: true,
                home_top,
            });
        }
        self.animate();
        true
    }

    /// How much of its slot `id`'s card takes while its file drag plays.
    fn slot_open(&self, id: &str, now: f64) -> f64 {
        self.imp()
            .drags
            .borrow()
            .iter()
            .find(|drag| drag.id == id)
            .map_or(1., |drag| drag.model.pose(now).slot)
    }

    /// Hand `id`'s closing slot to `model` ([`Reorder::set_slot`]), which
    /// then starts every card where the slot holds it now, so the slot
    /// reopening and the cards making room never add up to a jump. Returns
    /// the first frame's offsets.
    fn take_slot(&self, id: &str, model: &mut Reorder, now: f64) -> Vec<f64> {
        model.set_slot(self.slot_open(id, now));
        model.frame(now).0
    }

    /// Each dragged card's pose now and how much hover look it keeps.
    fn drag_poses(&self, now: f64) -> HashMap<String, (CardPose, f64)> {
        self.imp()
            .drags
            .borrow()
            .iter()
            .map(|drag| {
                let hover = if drag.hovered {
                    1. - ((now - drag.began) / HOVER_FADE_MS).clamp(0., 1.)
                } else {
                    0.
                };
                (drag.id.clone(), (drag.model.pose(now), hover))
            })
            .collect()
    }

    /// A drag is still moving: it ended, or its look is still settling.
    fn drags_moving(&self, now: f64) -> bool {
        self.imp().drags.borrow().iter().any(|drag| {
            drag.model.ended().is_some()
                || (drag.hovered && now - drag.began < HOVER_FADE_MS)
                || drag.model.pose(now) != drag.model.pose(now + 10_000.)
        })
    }

    /// Let go of drags whose end has played: a returned card is at rest
    /// again; a dropped one leaves the stack in this same frame.
    fn prune_drags(&self) {
        let imp = self.imp();
        let now = self.drag_now();
        let finished = take_finished(&mut imp.drags.borrow_mut(), now);
        for (id, end) in finished {
            if end == DragEnd::Returned {
                if let Some(card) = imp.cards.borrow().iter().find(|card| card.id == id) {
                    card.set_pose(None);
                    card.root.remove_css_class("drag-imprint");
                    card.root.remove_css_class("drag-returning");
                }
                self.update_cards();
                if let Some(returned) = imp.on_returned.borrow().as_ref() {
                    returned(&id);
                }
            } else if let Some(dropped) = imp.on_dropped.borrow().as_ref() {
                dropped(&id);
            } else {
                self.remove_card(&id);
            }
        }
    }

    /// Pick `id` up to carry it within the stack. False when it cannot
    /// move: a lone card, a collapsed or flying pile, or another drag, exit
    /// or reorder still playing.
    pub fn reorder_begin(&self, id: &str) -> bool {
        let imp = self.imp();
        let now = self.now_ms();
        let (_, since) = imp.fly.get();
        if imp.collapsed.get()
            || self.tweens().0.running(now - since, self.reduced())
            || !imp.exits.borrow().is_empty()
            || !imp.drags.borrow().is_empty()
            || imp.carried.borrow().is_some()
        {
            return false;
        }
        let (count, from, root) = {
            let cards = imp.cards.borrow();
            let Some(from) = cards.iter().position(|card| card.id == id) else {
                return false;
            };
            (cards.len(), from, cards[from].root.clone())
        };
        let Some(&(_, home_top)) = imp.origins.borrow().get(id) else {
            return false;
        };
        if count < 2 {
            return false;
        }
        let top = imp.top_anchor.get();
        let model = Reorder::new(count, from, top, self.drag_now(), self.reduced());
        *imp.carried.borrow_mut() = Some(Carried {
            id: id.to_owned(),
            model,
            offsets: vec![0.; count],
            lift: 0.,
            moving: true,
            home_top: f64::from(home_top),
        });
        root.add_css_class("reorder-lifted");
        self.animate();
        true
    }

    /// The pointer moved `dy` points since the press that picked it up.
    pub fn reorder_pointer(&self, dy: f64) {
        if let Some(carried) = self.imp().carried.borrow_mut().as_mut() {
            carried.model.pointer(dy);
            carried.moving = true;
        }
        self.animate();
    }

    /// Whether the pointer, at `(x, y)` in the pile, has left the carried
    /// card's column or gone past the stack's outermost slot far enough to
    /// become a file drag (`preview_drag_style::reorder_escapes`).
    pub fn reorder_escapes(&self, x: f64, y: f64) -> bool {
        let imp = self.imp();
        let carried = imp.carried.borrow();
        let Some(carried) = carried.as_ref() else {
            return false;
        };
        let count = carried.offsets.len();
        let top = imp.top_anchor.get();
        let from = carried.model.from();
        let content = carried.home_top - Reorder::slot_top(count, from, top);
        let tops = (0..count).map(|index| Reorder::slot_top(count, index, top));
        let first = tops.clone().fold(f64::INFINITY, f64::min) + content;
        let last = tops.fold(f64::NEG_INFINITY, f64::max) + content + f64::from(HEIGHT);
        let pad = preview::THUMBNAIL_PADDING;
        let outside = (pad - x).max(x - (pad + CARD_WIDTH)).max(0.);
        drag_style::reorder_escapes(outside, (first - y).max(y - last))
    }

    /// Let go of the carried card: it glides into its new slot.
    pub fn reorder_drop(&self) {
        self.reorder_release(false);
    }

    /// Give the reorder up (it became a file drag): every card glides home.
    pub fn reorder_cancel(&self) {
        self.reorder_release(true);
    }

    fn reorder_release(&self, cancel: bool) {
        let now = self.drag_now();
        let id = {
            let mut carried = self.imp().carried.borrow_mut();
            let Some(carried) = carried.as_mut() else {
                return;
            };
            if cancel {
                carried.model.cancel(now);
            } else {
                carried.model.drop(now);
            }
            carried.moving = true;
            carried.id.clone()
        };
        if let Some(root) = self.card_root(&id) {
            root.remove_css_class("reorder-lifted");
        }
        self.animate();
    }

    /// A card is carried (until it has settled into its slot).
    pub fn is_reordering(&self) -> bool {
        self.imp().carried.borrow().is_some()
    }

    /// Drop a reorder at once: the stack's cards changed under it.
    fn abandon_reorder(&self) {
        if let Some(carried) = self.imp().carried.take()
            && let Some(root) = self.card_root(&carried.id)
        {
            root.remove_css_class("reorder-lifted");
        }
    }

    /// Advance a reorder one frame; once let go and settled, commit its
    /// order and lay the stack out without offsets in the same frame.
    fn tick_reorder(&self) {
        let imp = self.imp();
        let now = self.drag_now();
        let settled = {
            let mut carried = imp.carried.borrow_mut();
            let Some(carried) = carried.as_mut() else {
                return;
            };
            carried.model.set_slot(self.slot_open(&carried.id, now));
            let (offsets, lift) = carried.model.frame(now);
            carried.moving = offsets != carried.offsets || lift != carried.lift;
            carried.offsets = offsets;
            carried.lift = lift;
            carried
                .model
                .settled(now)
                .then(|| (carried.id.clone(), carried.model.from(), carried.model.to()))
        };
        let Some((id, from, to)) = settled else {
            return;
        };
        imp.carried.take();
        self.commit_move(&id, from, to);
    }

    /// Move `id` from chronological `from` to `to` for good and tell
    /// [`Self::connect_reordered`].
    fn commit_move(&self, id: &str, from: usize, to: usize) {
        let imp = self.imp();
        if from != to {
            move_item(&mut imp.cards.borrow_mut(), from, to);
            // Paint order follows the chronology again.
            let first_cue = imp.cues.borrow().first().cloned();
            for card in imp.cards.borrow().iter() {
                card.root.insert_before(self, first_cue.as_ref());
            }
        }
        self.update_cards();
        self.queue_resize();
        if let Some(reordered) = imp.on_reordered.borrow().as_ref() {
            reordered(id, to);
        }
    }

    /// `--drag reorder`: carry chronological `from` by `dy`, played up to
    /// `at` ms on the held clock.
    pub fn reorder_fixture(&self, from: usize, dy: f64, at: f64) {
        let imp = self.imp();
        let Some(id) = imp.cards.borrow().get(from).map(|card| card.id.clone()) else {
            return;
        };
        imp.drag_clock.set(Some(0.));
        if !self.reorder_begin(&id) {
            return;
        }
        self.reorder_pointer(dy);
        let mut now = 0.;
        while now < at {
            now = (now + 16.).min(at);
            imp.drag_clock.set(Some(now));
            self.tick_reorder();
        }
    }

    /// Drop exits that have finished and let the stack shrink.
    fn prune_exits(&self) {
        let imp = self.imp();
        let display = {
            let mut exits = imp.exits.borrow_mut();
            if !exits.prune(
                self.now_ms(),
                self.reduced(),
                &preview_motion::settle_tween(),
            ) {
                return;
            }
            exits.display_ids().to_vec()
        };
        imp.cards.borrow_mut().retain(|card| {
            let keep = display.contains(&card.id);
            if !keep {
                card.root.unparent();
            }
            keep
        });
        imp.dust.borrow_mut().retain(|id, _| display.contains(id));
        imp.origins
            .borrow_mut()
            .retain(|id, _| display.contains(id));
        self.update_cards();
        self.queue_resize();
        if let Some(exited) = imp.on_exited.borrow().as_ref() {
            exited();
        }
    }

    /// Add the newest card; it rises in with shipping's arrival.
    pub fn add_card(&self, card: PileCard) {
        let imp = self.imp();
        // A capture after an exit or Clear all brings the toolbar back.
        if matches!(
            imp.toolbar.get(),
            Some((ToolbarPhase::Exit | ToolbarPhase::Clear, _))
        ) {
            imp.toolbar.set(None);
        }
        // Paint order is child order: cards stay under the cues and controls.
        card.root.insert_before(self, imp.cues.borrow().first());
        // The newest card comes into view.
        imp.scroll.set(None);
        self.abandon_reorder();
        imp.arrivals
            .borrow_mut()
            .insert(card.id.clone(), self.now_ms());
        imp.cards.borrow_mut().push(card);
        let live = self.live_ids();
        imp.exits.borrow_mut().sync(&live);
        self.update_cards();
        self.animate();
    }

    pub fn remove_card(&self, id: &str) {
        let imp = self.imp();
        imp.cards.borrow_mut().retain(|card| {
            let keep = card.id != id;
            if !keep {
                card.root.unparent();
            }
            keep
        });
        imp.arrivals.borrow_mut().remove(id);
        imp.dust.borrow_mut().remove(id);
        imp.drags.borrow_mut().retain(|drag| drag.id != id);
        self.abandon_reorder();
        imp.exits.borrow_mut().release(id, false);
        let live = self.live_ids();
        imp.exits.borrow_mut().sync(&live);
        self.update_cards();
        self.queue_resize();
    }

    pub fn set_top_anchor(&self, top: bool) {
        if self.imp().top_anchor.replace(top) != top {
            self.queue_resize();
        }
    }

    /// Fly into or out of the pile from wherever the cards are now.
    pub fn set_collapsed(&self, collapsed: bool) {
        let imp = self.imp();
        if imp.collapsed.get() == collapsed {
            return;
        }
        let now = self.now_ms();
        let from = self.collapse_progress(now);
        imp.collapsed.set(collapsed);
        imp.fly.set((from, now));
        // Flying cards stay sharp; the rear blur settles at rest.
        for card in imp.cards.borrow().iter() {
            card.image.set_blur(0.);
        }
        if !collapsed {
            imp.hovered.set(false);
            *imp.fan.borrow_mut() = StackFan::default();
            self.set_sparkling(false);
        }
        self.start_toolbar(if collapsed {
            ToolbarPhase::Out
        } else {
            ToolbarPhase::In
        });
        self.update_cards();
        self.animate();
    }

    fn set_hovered(&self, hovered: bool) {
        let imp = self.imp();
        // A carried window moves under the pointer: the fan stays open.
        if !hovered && imp.carry.get().since.is_some() {
            return;
        }
        imp.hovered.set(hovered);
        if !imp.collapsed.get() {
            return;
        }
        self.set_sparkling(hovered);
        let ids = self.depths();
        let cards: Vec<(&str, usize)> = ids.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        let fan_tween = self.tweens().1;
        let changed =
            imp.fan
                .borrow_mut()
                .set(hovered, self.now_ms(), &cards, &fan_tween, self.reduced());
        if changed {
            self.animate();
        }
    }

    /// Clear all: the toolbar leaves with the first Close streak.
    pub fn toolbar_clear(&self) {
        self.start_toolbar(ToolbarPhase::Clear);
    }

    /// Play `phase` on the toolbar; at once (nothing plays) with animations off.
    fn start_toolbar(&self, phase: ToolbarPhase) {
        let imp = self.imp();
        if self.reduced() {
            imp.toolbar.set(None);
        } else {
            imp.toolbar.set(Some((phase, self.now_ms())));
        }
        self.update_cards();
        self.animate();
    }

    /// The toolbar's motion and how far it has played, while it plays.
    fn toolbar_pose(&self, now: f64) -> Option<(ToolbarPhase, Animation, f64)> {
        let imp = self.imp();
        let (phase, since) = imp.toolbar.get()?;
        let motion = (*imp.toolbar_motion.borrow())?;
        let animation = motion[phase as usize];
        Some((phase, animation, now - since))
    }

    /// End an entrance or a collapse once it has played; an exit or clear
    /// holds its last frame until a new capture arrives.
    fn settle_toolbar(&self) -> bool {
        let Some((phase, animation, elapsed)) = self.toolbar_pose(self.now_ms()) else {
            return false;
        };
        if animation.running(elapsed, false) {
            return true;
        }
        if matches!(phase, ToolbarPhase::In | ToolbarPhase::Out) {
            self.imp().toolbar.set(None);
            self.update_cards();
            self.queue_resize();
        }
        false
    }

    /// The toolbar under its pose: opacity, rise, scale about its centre
    /// and the dissolve blur.
    fn snapshot_toolbar(&self, toolbar: &gtk::Widget, snapshot: &gtk::Snapshot) {
        let Some((_, animation, elapsed)) = self.toolbar_pose(self.now_ms()) else {
            self.snapshot_child(toolbar, snapshot);
            return;
        };
        let pose = animation.pose_at(elapsed, false);
        if pose.opacity <= 0. {
            return;
        }
        let Some(bounds) = toolbar.compute_bounds(self) else {
            return;
        };
        let centre = bounds.center();
        snapshot.save();
        snapshot.translate(&centre);
        snapshot.translate(&graphene::Point::new(
            pose.translate_x as f32,
            pose.translate_y as f32,
        ));
        snapshot.scale((pose.scale * pose.scale_x) as f32, pose.scale as f32);
        snapshot.translate(&graphene::Point::new(-centre.x(), -centre.y()));
        snapshot.push_opacity(pose.opacity);
        let blurred = pose.blur > 0.;
        if blurred {
            snapshot.push_blur(pose.blur);
        }
        self.snapshot_child(toolbar, snapshot);
        if blurred {
            snapshot.pop();
        }
        snapshot.pop();
        snapshot.restore();
    }

    /// Shipping plays the sparkles on a hovered collapsed pile, but not
    /// while it is carried or with animations off.
    fn set_sparkling(&self, on: bool) {
        let imp = self.imp();
        let on = on && !self.reduced() && imp.carry.get().since.is_none();
        if on == imp.sparkle_since.get().is_some() {
            return;
        }
        if !on {
            imp.sparkle_since.set(None);
            self.queue_draw();
            return;
        }
        imp.sparkle_since.set(Some(self.now_ms()));
        self.add_tick_callback(|pile, _| {
            pile.queue_draw();
            if pile.imp().sparkle_since.get().is_some() {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    /// `.thumbnail-collapsed-hit-target::before/::after`: each layer of
    /// dots rises and fades over the pile, `PreviewPileSparkle` and its late
    /// twin, with the layer's centre as the transform origin.
    fn snapshot_sparkles(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let Some(since) = imp.sparkle_since.get() else {
            return;
        };
        let Some((early, late, accent)) = *imp.sparkle.borrow() else {
            return;
        };
        let Some(expand) = imp.expand.borrow().clone() else {
            return;
        };
        let Some(bounds) = expand.compute_bounds(self) else {
            return;
        };
        let layer = preview_motion::sparkle_layer(
            (
                f64::from(bounds.x()),
                f64::from(bounds.y()),
                f64::from(bounds.width()),
                f64::from(bounds.height()),
            ),
            imp.top_anchor.get(),
        );
        let elapsed = self.now_ms() - since;
        let centre = graphene::Point::new(
            (layer.0 + layer.2 / 2.) as f32,
            (layer.1 + layer.3 / 2.) as f32,
        );
        let white = gtk::gdk::RGBA::WHITE;
        let layers: [(&Animation, &[preview_motion::SparkleDot]); 2] = [
            (&early, &preview_motion::SPARKLES_EARLY),
            (&late, &preview_motion::SPARKLES_LATE),
        ];
        for (animation, dots) in layers {
            let pose = animation.pose_repeating(elapsed, false);
            if pose.opacity <= 0. {
                continue;
            }
            snapshot.save();
            snapshot.translate(&centre);
            snapshot.translate(&graphene::Point::new(0., pose.translate_y as f32));
            snapshot.scale(pose.scale as f32, pose.scale as f32);
            snapshot.translate(&graphene::Point::new(-centre.x(), -centre.y()));
            snapshot.push_opacity(pose.opacity);
            for dot in dots {
                let (x, y) = preview_motion::sparkle_center(dot, layer);
                let base = if dot.accent { accent } else { white };
                let color = gtk::gdk::RGBA::new(
                    base.red(),
                    base.green(),
                    base.blue(),
                    base.alpha() * dot.alpha as f32,
                );
                let clear = gtk::gdk::RGBA::new(color.red(), color.green(), color.blue(), 0.);
                let fade = dot.fade as f32;
                let core = (dot.core / dot.fade) as f32;
                let (x, y) = (x as f32, y as f32);
                // `radial-gradient(circle, color 0 core, transparent fade)`.
                snapshot.append_radial_gradient(
                    &graphene::Rect::new(x - fade, y - fade, 2. * fade, 2. * fade),
                    &graphene::Point::new(x, y),
                    fade,
                    fade,
                    0.,
                    1.,
                    &[
                        gsk::ColorStop::new(0., color),
                        gsk::ColorStop::new(core, color),
                        gsk::ColorStop::new(1., clear),
                    ],
                );
            }
            snapshot.pop();
            snapshot.restore();
        }
    }

    fn depths(&self) -> Vec<(String, usize)> {
        let cards = self.imp().cards.borrow();
        let count = cards.len();
        cards
            .iter()
            .enumerate()
            .map(|(index, card)| (card.id.clone(), count - 1 - index))
            .collect()
    }

    fn tweens(&self) -> (Tween, Tween, Animation) {
        self.imp().motion.borrow().expect("pile motion")
    }

    /// GNOME's "Reduce animation" turns GTK animations off.
    fn reduced(&self) -> bool {
        !self.settings().is_gtk_enable_animations()
    }

    /// The monotonic clock: an idle frame clock's last frame time is stale,
    /// which would start a new motion already partly played.
    fn now_ms(&self) -> f64 {
        glib::monotonic_time() as f64 / 1000.
    }

    fn content_height(&self, progress: f64) -> f64 {
        let count = self.imp().cards.borrow().len();
        lerp(
            preview::stack_height(count),
            preview::collapsed_frame_height(count),
            progress,
        )
    }

    fn collapse_progress(&self, now: f64) -> f64 {
        let imp = self.imp();
        let (from, since) = imp.fly.get();
        let target = if imp.collapsed.get() { 1. } else { 0. };
        self.tweens()
            .0
            .value(from, target, now - since, self.reduced())
    }

    /// Collapsed cards are one hit target: rear cards leave the
    /// accessibility tree, and none takes the pointer until it expands.
    fn update_cards(&self) {
        let imp = self.imp();
        let collapsed = imp.collapsed.get();
        let cards = imp.cards.borrow();
        let count = cards.len();
        // Shipping hides the stack toolbar while minimized, after it leaves.
        if let Some(controls) = imp.controls.borrow().as_ref() {
            let phase = imp.toolbar.get().map(|(phase, _)| phase);
            controls.set_visible(!collapsed || phase == Some(ToolbarPhase::Out));
            // The toolbar takes no input while it moves.
            controls.set_can_target(phase.is_none());
        }
        if let Some(expand) = imp.expand.borrow().as_ref() {
            expand.set_visible(collapsed);
            let label = if count == 1 {
                "Expand preview".to_owned()
            } else {
                format!("Expand {count} previews")
            };
            expand.update_property(&[gtk::accessible::Property::Label(&label)]);
        }
        let exits = imp.exits.borrow();
        let drags = imp.drags.borrow();
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            // An exiting card, or one whose file was dropped elsewhere,
            // takes no input and leaves the tree at once.
            let exiting = exits.exiting(&card.id).is_some()
                || drags
                    .iter()
                    .any(|drag| drag.id == card.id && drag.model.ended() == Some(DragEnd::Dropped));
            card.root.set_can_target(!collapsed && !exiting);
            card.root.update_state(&[gtk::accessible::State::Hidden(
                exiting || (collapsed && depth > 0),
            )]);
        }
    }

    /// Tick while something moves; settle each card's media blur after.
    fn animate(&self) {
        let imp = self.imp();
        self.queue_resize();
        if imp.ticking.replace(true) {
            return;
        }
        self.add_tick_callback(|pile, _| {
            pile.prune_exits();
            pile.prune_drags();
            pile.tick_reorder();
            pile.tick_carry();
            pile.queue_resize();
            if !pile.imp().dust.borrow().is_empty() {
                pile.queue_draw();
            }
            if pile.running() {
                return glib::ControlFlow::Continue;
            }
            pile.imp().ticking.set(false);
            pile.settle_blur();
            glib::ControlFlow::Break
        });
    }

    fn running(&self) -> bool {
        let imp = self.imp();
        if self.is_clearing()
            && !matches!(
                imp.toolbar.get(),
                Some((ToolbarPhase::Clear | ToolbarPhase::Exit, _))
            )
            && !imp.collapsed.get()
            && !self.reduced()
        {
            imp.toolbar.set(Some((ToolbarPhase::Exit, self.now_ms())));
            self.update_cards();
        }
        let toolbar = self.settle_toolbar();
        if toolbar {
            self.queue_draw();
        }
        let now = self.now_ms();
        let reduced = self.reduced();
        let (fly, fan, arrive) = self.tweens();
        let (_, since) = imp.fly.get();
        imp.arrivals
            .borrow_mut()
            .retain(|_, start| arrive.running(now - *start, reduced));
        let deepest = imp.cards.borrow().len().saturating_sub(1);
        toolbar
            || fly.running(now - since, reduced)
            || imp.fan.borrow().running(deepest, now, &fan, reduced)
            || !imp.arrivals.borrow().is_empty()
            || !imp.exits.borrow().is_empty()
            || self.carry_moving(now, deepest, &fan, reduced)
            || self.drags_moving(self.drag_now())
            || imp
                .carried
                .borrow()
                .as_ref()
                .is_some_and(|carried| carried.moving)
            || imp
                .scroll
                .get()
                .is_some_and(|(_, _, since)| !reduced && now - since < SCROLL_MS)
    }

    /// Clear all is emptying the stack: every card is leaving.
    fn is_clearing(&self) -> bool {
        let imp = self.imp();
        let exits = imp.exits.borrow();
        let cards = imp.cards.borrow();
        !cards.is_empty() && cards.iter().all(|card| exits.exiting(&card.id).is_some())
    }

    fn exit_motion(&self) -> ExitMotion {
        self.imp().exit_motion.get().expect("exit motion")
    }

    /// A dissolving card's chips (`thumbnail-dust-layer`): each a piece of
    /// the frozen hover image that rises, turns and fades from the trash
    /// control while the layer's clip opens past the card.
    fn snapshot_dust(&self, id: &str, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let dust = imp.dust.borrow();
        let Some(particles) = dust.get(id) else {
            return;
        };
        let Some(elapsed) = imp
            .exits
            .borrow()
            .exiting(id)
            .map(|exit| exit.elapsed_ms(self.now_ms()).max(0.))
        else {
            return;
        };
        let frame = preview_motion::dust_frame(elapsed, self.reduced());
        let Some(&(x, y)) = imp.origins.borrow().get(id) else {
            return;
        };
        let cards = imp.cards.borrow();
        let Some((texture, drawn)) = cards
            .iter()
            .find(|card| card.id == id)
            .and_then(|card| card.image.drawn())
        else {
            return;
        };
        if frame.layer_opacity <= 0. {
            return;
        }
        let pad = preview_motion::DUST_LAYER_PAD as f32;
        let open = pad * frame.clip_open as f32;
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x, y));
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(
            graphene::Rect::new(-open, -open, WIDTH + 2. * open, HEIGHT + 2. * open),
            12.,
        ));
        for particle in particles {
            let visual = preview_motion::dust_visual_at(particle, elapsed);
            let alpha = visual.opacity * frame.layer_opacity;
            if alpha <= 0. {
                continue;
            }
            let (w, h) = (particle.width as f32, particle.height as f32);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (particle.left + visual.dx) as f32 - pad + w / 2.,
                (particle.top + visual.dy) as f32 - pad + h / 2.,
            ));
            snapshot.rotate(visual.rotate as f32);
            snapshot.scale(visual.scale as f32, visual.scale as f32);
            snapshot.push_opacity(alpha);
            snapshot.push_clip(&graphene::Rect::new(-w / 2., -h / 2., w, h));
            snapshot.append_texture(
                &texture,
                &graphene::Rect::new(
                    drawn.x() - particle.source_left as f32 - w / 2.,
                    drawn.y() - particle.source_top as f32 - h / 2.,
                    drawn.width(),
                    drawn.height(),
                ),
            );
            snapshot.pop();
            snapshot.pop();
            snapshot.restore();
        }
        snapshot.pop();
        snapshot.restore();
    }

    /// Rear cards' media blur, baked once the pile is at rest.
    fn settle_blur(&self) {
        let imp = self.imp();
        let collapsed = imp.collapsed.get();
        let fanned = imp.fan.borrow().open();
        let cards = imp.cards.borrow();
        let count = cards.len();
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            let blur = if collapsed {
                preview::collapsed_media_blur(depth, fanned)
            } else {
                0.
            };
            card.image.set_blur(blur as f32);
        }
    }

    fn place(&self, width: f32, height: f32) {
        let imp = self.imp();
        let now = self.now_ms();
        let reduced = self.reduced();
        let progress = self.collapse_progress(now);
        let top = imp.top_anchor.get();
        let gravity = imp.gravity.get().unwrap_or(if top { -1. } else { 1. });
        let (_, fan_tween, arrive) = self.tweens();
        let motion = self.exit_motion();
        let settle = preview_motion::settle_tween();
        let exits = imp.exits.borrow();
        let mut origins = imp.origins.borrow_mut();
        let mirror = if imp.right_anchor.get() { -1. } else { 1. };
        let fan = imp.fan.borrow();
        let arrivals = imp.arrivals.borrow();
        let cards = imp.cards.borrow();
        let count = cards.len();
        let pad = preview::THUMBNAIL_PADDING;
        // A retained frame keeps the cards on the anchored edge.
        let offset = if top {
            0.
        } else {
            f64::from(height) - self.content_height(progress)
        };
        // An expanded stack taller than the window scrolls (`scrollTop`).
        let content = preview::stack_height(count);
        let max_scroll = (content - f64::from(height)).max(0.);
        imp.max_scroll.set(max_scroll);
        let scroll_top = self.scroll_top(now, max_scroll);
        let shift = if top {
            -scroll_top
        } else {
            max_scroll - scroll_top
        };
        let overflow = if progress > 0. || imp.collapsed.get() || self.is_clearing() {
            preview::StackOverflow::default()
        } else {
            preview::stack_overflow(scroll_top, content, f64::from(height))
        };
        for (cue, shown) in imp
            .cues
            .borrow()
            .iter()
            .zip([overflow.above, overflow.below])
        {
            let older = cue.has_css_class("older");
            cue.set_opacity(if shown { 1. } else { 0. });
            cue.set_can_target(shown);
            cue.set_focusable(shown);
            cue.update_state(&[gtk::accessible::State::Hidden(!shown)]);
            let label = preview::overflow_cue_label(older, top);
            cue.set_tooltip_text(Some(label));
            cue.update_property(&[gtk::accessible::Property::Label(label)]);
            let (w, h) = (46., 22.);
            let y = if older { 6. } else { height - 6. - h };
            let transform =
                gsk::Transform::new().translate(&graphene::Point::new((width - w) / 2., y));
            cue.allocate(w as i32, h as i32, -1, Some(transform));
        }
        // File drags close their slots toward the anchored edge, and a
        // carried card's neighbours glide out of its way.
        // A carried card's slot moves with its reorder instead.
        let poses = self.drag_poses(self.drag_now());
        let carried = imp.carried.borrow();
        let carried = carried
            .as_ref()
            .filter(|carried| carried.offsets.len() == count);
        let closing: Vec<(usize, f64)> = cards
            .iter()
            .enumerate()
            .filter(|(_, card)| carried.is_none_or(|carried| carried.id != card.id))
            .filter_map(|(index, card)| Some((index, poses.get(&card.id)?.0.slot)))
            .collect();
        let closing = closing_offsets(count, &closing, top);
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            let moved = closing[index] + carried.map_or(0., |carried| carried.offsets[index]);
            let expanded = preview::card_layout_in(count, index, false, top, false)
                .map_or(0., |l| l.y)
                + shift
                + moved * (1. - progress);
            let front = preview::collapsed_padding(count);
            let rest = preview::collapsed_card_pose(&card.id, depth, false, gravity, top);
            let lean = self.lean(depth, now, &fan_tween, reduced);
            let fanned = if lean == (0., 0.) {
                preview::collapsed_card_pose(&card.id, depth, true, gravity, top)
            } else {
                preview::collapsed_card_sway_pose(&card.id, depth, gravity, top, lean)
            };
            let (fan_pose, _) = fan.progress(&card.id, depth, now, &fan_tween, reduced);
            let pose = mix(rest, fanned, fan_pose);
            let half = (CARD_WIDTH / 2., preview::THUMBNAIL_CARD_HEIGHT / 2.);
            let centre_x = pad + half.0 + pose.dx * progress;
            let centre_y = offset + lerp(expanded + half.1, front + half.1 + pose.dy, progress);
            let mut scale = (
                lerp(1., pose.scale_x, progress),
                lerp(1., pose.scale_y, progress),
            );
            let mut lift = 0.;
            let mut opacity = 1.;
            if let Some(start) = arrivals.get(&card.id) {
                let frame = arrive.pose_at(now - start, reduced);
                lift = frame.translate_y;
                scale = (scale.0 * frame.scale, scale.1 * frame.scale);
                opacity = frame.opacity;
            }
            // Survivors slide into an exiting card's slot; the exiting card
            // streaks out, shrinks away or hands its look to the dust.
            let shift = exits.shift_px(&card.id, now, reduced, &settle, top);
            let mut slide = 0.;
            match exits.exiting(&card.id) {
                Some(exit) if exit.kind == ExitKind::Dust => {
                    let frame = preview_motion::dust_frame(exit.elapsed_ms(now).max(0.), reduced);
                    let mut child = card.root.first_child();
                    while let Some(widget) = child {
                        widget.set_opacity(
                            if widget == card.image.clone().upcast::<gtk::Widget>() {
                                frame.source_opacity
                            } else if widget == card.dim {
                                widget.opacity()
                            } else {
                                frame.chrome_opacity
                            },
                        );
                        child = widget.next_sibling();
                    }
                }
                Some(exit) => {
                    let elapsed = exit.elapsed_ms(now).max(0.);
                    let animation = if exit.kind == ExitKind::Dismiss {
                        motion.dismiss
                    } else {
                        motion.fallback
                    };
                    let pose = animation.pose_at(elapsed, reduced);
                    slide += pose.translate_x * mirror;
                    opacity *= pose.opacity;
                    scale = (scale.0 * pose.scale, scale.1 * pose.scale);
                    if exit.kind == ExitKind::Dismiss {
                        let streak = motion.streak.pose_at(elapsed, reduced);
                        card.image.set_streak(Some(Streak {
                            scale_x: streak.scale_x as f32,
                            step: streak_step(streak.blur),
                            keep_look: !exit.settles,
                        }));
                    }
                }
                None => {}
            }
            let dragged = poses.get(&card.id).copied();
            if let Some((drag, _)) = dragged {
                opacity *= drag.opacity;
                scale = (scale.0 * drag.scale, scale.1 * drag.scale);
            }
            if dragged.is_some() || card.image.has_media() {
                card.set_pose(dragged);
            }
            if let Some(carried) = carried.filter(|carried| carried.id == card.id) {
                let lift = 1. + (drag_style::REORDER_LIFT_SCALE - 1.) * carried.lift;
                scale = (scale.0 * lift, scale.1 * lift);
            }
            let centre_x = centre_x + slide;
            let centre_y = centre_y + shift;
            origins.insert(
                card.id.clone(),
                ((centre_x - half.0) as f32, (centre_y - half.1) as f32),
            );
            card.root.set_opacity(opacity);
            card.dim.set_opacity(
                preview::collapsed_dim_opacity(depth) * progress * (1. - 0.35 * fan_pose),
            );
            let transform = gsk::Transform::new()
                .translate(&graphene::Point::new(
                    centre_x as f32,
                    (centre_y + lift) as f32,
                ))
                .rotate((pose.rotation_deg * progress) as f32)
                .scale(scale.0 as f32, scale.1 as f32)
                .translate(&graphene::Point::new(-WIDTH / 2., -HEIGHT / 2.));
            card.root
                .allocate(WIDTH as i32, HEIGHT as i32, -1, Some(transform));
        }
        if let Some(expand) = imp.expand.borrow().as_ref()
            && expand.is_visible()
        {
            let front = (offset + preview::collapsed_padding(count)) as f32;
            let transform =
                gsk::Transform::new().translate(&graphene::Point::new(pad as f32, front));
            expand.allocate(WIDTH as i32, HEIGHT as i32, -1, Some(transform));
        }
        if let Some(controls) = imp.controls.borrow().as_ref()
            && controls.is_visible()
        {
            let gutter = preview::THUMBNAIL_CONTROL_GUTTER as f32;
            let y = if top { 0. } else { height - gutter };
            let transform = gsk::Transform::new().translate(&graphene::Point::new(pad as f32, y));
            controls.allocate(
                (width - 2. * pad as f32) as i32,
                gutter as i32,
                -1,
                Some(transform),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::preview::PreviewStack;

    /// A card's top in a stack of `count` whose window is `height` tall,
    /// as `place` lays an expanded, unscrolled stack out: a bottom stack
    /// keeps its cards on the window's bottom edge.
    fn top(count: usize, index: usize, top: bool, height: f64) -> f64 {
        let edge = if top {
            0.
        } else {
            height - preview::stack_height(count)
        };
        edge + preview::card_layout_in(count, index, false, top, false)
            .expect("in the stack")
            .y
    }

    fn drag(id: &str, style: DragStyle) -> FileDrag {
        FileDrag {
            id: id.into(),
            model: DragOut::new(style, 0., false),
            began: 0.,
            hovered: false,
        }
    }

    #[test]
    fn a_dropped_card_keeps_its_slot_until_its_end_has_played() {
        let mut drags = vec![drag("a", DragStyle::Frosted), drag("b", DragStyle::Dock)];
        // Out of the stack, with no end yet: nothing finishes.
        assert!(take_finished(&mut drags, 5_000.).is_empty());
        drags[0].model.end(DragEnd::Dropped, 5_000.);
        drags[1].model.end(DragEnd::Returned, 5_000.);
        let dropped = DragOut::end_ms(DragStyle::Frosted, DragEnd::Dropped);
        let returned = DragOut::end_ms(DragStyle::Dock, DragEnd::Returned);
        assert!(returned < dropped);
        assert!(take_finished(&mut drags, 5_000. + returned - 1.).is_empty());
        assert_eq!(drags.len(), 2);
        assert_eq!(
            take_finished(&mut drags, 5_000. + returned),
            vec![("b".to_owned(), DragEnd::Returned)]
        );
        assert_eq!(
            take_finished(&mut drags, 5_000. + dropped),
            vec![("a".to_owned(), DragEnd::Dropped)]
        );
        assert!(drags.is_empty());
    }

    #[test]
    fn a_closed_slot_leaves_every_card_where_removal_puts_it() {
        for anchored_top in [false, true] {
            for (count, dragged) in [(2, 0), (2, 1), (4, 0), (4, 2), (4, 3)] {
                let mut model = DragOut::new(DragStyle::Frosted, 0., false);
                model.end(DragEnd::Dropped, 1_000.);
                let done = 1_000. + DragOut::end_ms(DragStyle::Frosted, DragEnd::Dropped);
                assert!(model.finished(done));
                let closing =
                    closing_offsets(count, &[(dragged, model.pose(done).slot)], anchored_top);
                // The window is one slot shorter afterwards; a bottom stack
                // keeps its bottom edge, a top stack its top edge.
                let (before, after) = (
                    preview::stack_height(count),
                    preview::stack_height(count - 1),
                );
                let grown = if anchored_top { 0. } else { before - after };
                for index in (0..count).filter(|index| *index != dragged) {
                    let left = top(count, index, anchored_top, before) + closing[index];
                    let new = if index < dragged { index } else { index - 1 };
                    let right = top(count - 1, new, anchored_top, after) + grown;
                    assert!(
                        (left - right).abs() < 1e-9,
                        "top {anchored_top}, {count} cards, drag {dragged}, card {index}"
                    );
                }
            }
        }
    }

    #[test]
    fn older_cards_close_toward_the_anchored_edge_and_newer_ones_stay() {
        // Chronological, oldest first: card 2 of 4 is half closed.
        let bottom = closing_offsets(4, &[(2, 0.5)], false);
        let top = closing_offsets(4, &[(2, 0.5)], true);
        let half = drag_style::SLOT_PITCH / 2.;
        // A bottom stack lists oldest first from its top: older cards move
        // down toward the bottom edge.
        assert_eq!(bottom, vec![half, half, 0., 0.]);
        // A top stack lists newest first from its top edge: older cards,
        // below the slot, move up.
        assert_eq!(top, vec![-half, -half, 0., 0.]);
        // Two closing slots add up for the cards older than both.
        assert_eq!(
            closing_offsets(3, &[(1, 0.), (2, 0.)], false),
            vec![2. * drag_style::SLOT_PITCH, drag_style::SLOT_PITCH, 0.]
        );
    }

    #[test]
    fn a_settled_reorder_commits_the_order_its_cards_already_show() {
        for anchored_top in [false, true] {
            let ids: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
            let mut stack = PreviewStack::default();
            for id in &ids {
                stack.insert(id.clone());
            }
            // Carry the oldest card two slots on, toward the newest.
            let toward_newest = if anchored_top { -1. } else { 1. };
            let mut reorder = Reorder::new(4, 0, anchored_top, 0., false);
            reorder.pointer(toward_newest * 2. * drag_style::SLOT_PITCH);
            reorder.drop(100.);
            let (mut now, mut offsets) = (100., vec![]);
            while !reorder.settled(now) {
                now += 16.;
                offsets = reorder.frame(now).0;
                assert!(now < 2_000.);
            }
            let (from, to) = (reorder.from(), reorder.to());
            assert_eq!((from, to), (0, 2));
            let mut cards = ids.clone();
            move_item(&mut cards, from, to);
            assert!(stack.move_card(&ids[from], to));
            assert_eq!(stack.ids(), cards.as_slice());
            assert_eq!(cards, ["b", "c", "a", "d"]);
            // Laid out without offsets, every card stays where it glided.
            for (old, id) in ids.iter().enumerate() {
                let new = cards.iter().position(|card| card == id).expect("kept");
                let slot = |index| Reorder::slot_top(4, index, anchored_top);
                assert!((slot(old) + offsets[old] - slot(new)).abs() < 1e-9, "{id}");
            }
        }
    }
}
