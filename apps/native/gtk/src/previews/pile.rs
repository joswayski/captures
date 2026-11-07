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
use captures_app::preview_motion::{self, DragSway, DustParticle, ExitKind, StackExits, StackFan};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::tokens::Tokens;
use crate::widgets::{Icon, Streak, Thumbnail, streak_step};

/// `THUMBNAIL_STACK_SCROLL_DURATION_MS`: an overflow cue's scroll.
const SCROLL_MS: f64 = 380.;

/// Shipping's 340 px window holds 284 px cards inside 28 px side padding.
pub const CARD_WIDTH: f64 = preview::THUMBNAIL_WIDTH - 2. * preview::THUMBNAIL_PADDING;
const WIDTH: f32 = CARD_WIDTH as f32;
const HEIGHT: f32 = preview::THUMBNAIL_CARD_HEIGHT as f32;

/// One card the pile places.
pub struct PileCard {
    pub id: String,
    pub root: gtk::Widget,
    /// `glass-strong-solid` shade over a rear card's image.
    pub dim: gtk::Widget,
    pub image: Thumbnail,
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
        /// Drop-on-self shake start, by card id.
        pub rejects: RefCell<HashMap<String, f64>>,
        pub exit_motion: Cell<Option<super::ExitMotion>>,
        /// An exit ended and its card left the pile.
        pub on_exited: RefCell<Option<Box<dyn Fn()>>>,
        /// A carried pile's gravity (`None`: ±1 from its corner).
        pub gravity: Cell<Option<f64>>,
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
        /// it, outside its clip.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let mut child = obj.first_child();
            while let Some(widget) = child {
                obj.snapshot_child(&widget, snapshot);
                let id = self
                    .cards
                    .borrow()
                    .iter()
                    .find(|card| card.root == widget)
                    .map(|card| card.id.clone());
                if let Some(id) = id {
                    obj.snapshot_dust(&id, snapshot);
                }
                child = widget.next_sibling();
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

/// Exit and reject timings, resolved once from the tokens.
#[derive(Clone, Copy)]
pub struct ExitMotion {
    dismiss: Animation,
    streak: Animation,
    fallback: Animation,
    /// `--thumbnail-drop-reject-duration` and `-x-1` … `-x-4`.
    reject_ms: f64,
    reject_x: [f64; 4],
}

impl ExitMotion {
    /// `thumbnail-drop-reject`'s horizontal shake, linear between stops.
    fn reject_offset(&self, elapsed_ms: f64) -> f64 {
        if elapsed_ms >= self.reject_ms {
            return 0.;
        }
        let [a, b, c, d] = self.reject_x;
        let stops = [
            (0., 0.),
            (0.1, -a),
            (0.3, a),
            (0.5, -b),
            (0.65, c),
            (0.8, -d),
            (0.9, d),
            (1., 0.),
        ];
        let t = (elapsed_ms / self.reject_ms).clamp(0., 1.);
        let pair = stops
            .windows(2)
            .find(|pair| t <= pair[1].0)
            .unwrap_or(&stops[6..]);
        lerp(
            pair[0].1,
            pair[1].1,
            (t - pair[0].0) / (pair[1].0 - pair[0].0),
        )
    }
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

impl Pile {
    pub fn new(tokens: &Tokens, controls: &impl IsA<gtk::Widget>) -> Pile {
        let pile: Pile = glib::Object::new();
        let imp = pile.imp();
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
            reject_ms: f64::from(tokens.number("thumbnail-drop-reject-duration")),
            reject_x: [1, 2, 3, 4]
                .map(|n| f64::from(tokens.number(&format!("thumbnail-drop-reject-x-{n}")))),
        }));
        controls.set_parent(&pile);
        *imp.controls.borrow_mut() = Some(controls.clone().upcast());

        // Overflow cues paint over the cards and under the stack toolbar.
        for (older, icon) in [(true, "chevron-up"), (false, "chevron-down")] {
            let cue = gtk::Button::new();
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

    /// Shipping's "no" shake when a card's file is dropped back on the stack.
    pub fn reject(&self, id: &str) {
        let imp = self.imp();
        if self.reduced() || imp.collapsed.get() || imp.exits.borrow().exiting(id).is_some() {
            return;
        }
        imp.rejects
            .borrow_mut()
            .insert(id.to_owned(), self.now_ms());
        self.animate();
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
        // Paint order is child order: cards stay under the cues and controls.
        card.root.insert_before(self, imp.cues.borrow().first());
        // The newest card comes into view.
        imp.scroll.set(None);
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
        imp.rejects.borrow_mut().remove(id);
        imp.dust.borrow_mut().remove(id);
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
        }
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
        // Shipping hides the stack toolbar while minimized.
        if let Some(controls) = imp.controls.borrow().as_ref() {
            controls.set_visible(!collapsed);
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
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            // An exiting card takes no input and leaves the tree at once.
            let exiting = exits.exiting(&card.id).is_some();
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
        let now = self.now_ms();
        let reduced = self.reduced();
        let (fly, fan, arrive) = self.tweens();
        let (_, since) = imp.fly.get();
        imp.arrivals
            .borrow_mut()
            .retain(|_, start| arrive.running(now - *start, reduced));
        let reject_ms = self.exit_motion().reject_ms;
        imp.rejects
            .borrow_mut()
            .retain(|_, start| !reduced && now - *start < reject_ms);
        let deepest = imp.cards.borrow().len().saturating_sub(1);
        fly.running(now - since, reduced)
            || imp.fan.borrow().running(deepest, now, &fan, reduced)
            || !imp.arrivals.borrow().is_empty()
            || !imp.rejects.borrow().is_empty()
            || !imp.exits.borrow().is_empty()
            || self.carry_moving(now, deepest, &fan, reduced)
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
        let rejects = imp.rejects.borrow();
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
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            let expanded = preview::card_layout_in(count, index, false, top, false)
                .map_or(0., |l| l.y)
                + shift;
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
            let mut slide = rejects
                .get(&card.id)
                .map_or(0., |start| motion.reject_offset(now - start));
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
