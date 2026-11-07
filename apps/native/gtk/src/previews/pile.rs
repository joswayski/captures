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
use captures_app::preview_motion::StackFan;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::tokens::Tokens;
use crate::widgets::Thumbnail;

const WIDTH: f32 = preview::THUMBNAIL_WIDTH as f32;
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
                preview::THUMBNAIL_WIDTH + 2. * preview::THUMBNAIL_PADDING
            } else {
                let count = self.cards.borrow().len();
                let progress = self.obj().collapse_progress(self.obj().now_ms());
                lerp(
                    preview::stack_height(count),
                    preview::collapsed_frame_height(count),
                    progress,
                )
            };
            let size = size.round() as i32;
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            self.obj().place(width as f32, height as f32);
        }
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
        controls.set_parent(&pile);
        *imp.controls.borrow_mut() = Some(controls.clone().upcast());

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

    /// Add the newest card; it rises in with shipping's arrival.
    pub fn add_card(&self, card: PileCard) {
        let imp = self.imp();
        // Paint order is child order: cards stay under the controls.
        card.root
            .insert_before(self, imp.controls.borrow().as_ref());
        imp.arrivals
            .borrow_mut()
            .insert(card.id.clone(), self.now_ms());
        imp.cards.borrow_mut().push(card);
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

    fn now_ms(&self) -> f64 {
        self.frame_clock()
            .map_or_else(glib::monotonic_time, |clock| clock.frame_time()) as f64
            / 1000.
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
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            card.root.set_can_target(!collapsed);
            card.root
                .update_state(&[gtk::accessible::State::Hidden(collapsed && depth > 0)]);
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
            pile.queue_resize();
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
        let deepest = imp.cards.borrow().len().saturating_sub(1);
        fly.running(now - since, reduced)
            || imp.fan.borrow().running(deepest, now, &fan, reduced)
            || !imp.arrivals.borrow().is_empty()
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
        let gravity = if top { -1. } else { 1. };
        let (_, fan_tween, arrive) = self.tweens();
        let fan = imp.fan.borrow();
        let arrivals = imp.arrivals.borrow();
        let cards = imp.cards.borrow();
        let count = cards.len();
        let pad = preview::THUMBNAIL_PADDING;
        for (index, card) in cards.iter().enumerate() {
            let depth = count - 1 - index;
            let expanded =
                preview::card_layout_in(count, index, false, top, false).map_or(0., |l| l.y);
            let front = preview::collapsed_padding(count);
            let rest = preview::collapsed_card_pose(&card.id, depth, false, gravity, top);
            let fanned = preview::collapsed_card_pose(&card.id, depth, true, gravity, top);
            let (fan_pose, _) = fan.progress(&card.id, depth, now, &fan_tween, reduced);
            let pose = mix(rest, fanned, fan_pose);
            let half = (
                preview::THUMBNAIL_WIDTH / 2.,
                preview::THUMBNAIL_CARD_HEIGHT / 2.,
            );
            let centre_x = pad + half.0 + pose.dx * progress;
            let centre_y = lerp(expanded + half.1, front + half.1 + pose.dy, progress);
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
            let front = preview::collapsed_padding(count) as f32;
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
