//! Shipping `.check-row.switch-row` switch: a 32×19 pill with a 13 pt knob,
//! drawn with GtkSnapshot from tokens. The knob slides with a frame-clock
//! tween that only invalidates this widget's own render node.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::motion::CubicBezier;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene, gsk};

use crate::tokens::Tokens;

const WIDTH: f32 = 32.;
const HEIGHT: f32 = 19.;
const KNOB: f32 = 13.;
const TRAVEL: f32 = 13.;

type Toggled = Box<dyn Fn(bool)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TokenSwitch {
        pub tokens: RefCell<Option<Rc<Tokens>>>,
        pub on: Cell<bool>,
        /// Drawn knob position, 0 (off) to 1 (on).
        pub position: Cell<f64>,
        pub hovered: Cell<bool>,
        pub tween: RefCell<Option<gtk::TickCallbackId>>,
        pub toggled: RefCell<Option<Toggled>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TokenSwitch {
        const NAME: &'static str = "CapturesTokenSwitch";
        type Type = super::TokenSwitch;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Switch);
            klass.set_css_name("captures-switch");
        }
    }

    impl ObjectImpl for TokenSwitch {
        fn constructed(&self) {
            self.parent_constructed();
            let widget = self.obj();
            widget.set_focusable(true);
            widget.set_valign(gtk::Align::Center);
            widget.set_cursor_from_name(Some("pointer"));
            let hover = gtk::EventControllerMotion::new();
            let weak = widget.downgrade();
            hover.connect_enter(move |_, _, _| {
                if let Some(widget) = weak.upgrade() {
                    widget.imp().hovered.set(true);
                    widget.queue_draw();
                }
            });
            let weak = widget.downgrade();
            hover.connect_leave(move |_| {
                if let Some(widget) = weak.upgrade() {
                    widget.imp().hovered.set(false);
                    widget.queue_draw();
                }
            });
            widget.add_controller(hover);
            let keys = gtk::EventControllerKey::new();
            let weak = widget.downgrade();
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(widget) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if matches!(key, gdk::Key::space | gdk::Key::Return | gdk::Key::KP_Enter) {
                    widget.activate_toggle();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            widget.add_controller(keys);
            widget.update_checked();
        }
    }

    impl WidgetImpl for TokenSwitch {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = if orientation == gtk::Orientation::Horizontal {
                WIDTH
            } else {
                HEIGHT
            } as i32;
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(t) = self.tokens.borrow().clone() else {
                return;
            };
            let widget = self.obj();
            let enabled = widget.is_sensitive();
            let alpha = if enabled { 1. } else { 0.55 };
            let position = self.position.get() as f32;
            let y = ((widget.height() as f32 - HEIGHT) / 2.).round();
            let track = graphene::Rect::new(0., y, WIDTH, HEIGHT);
            let pill = gsk::RoundedRect::from_rect(track, HEIGHT / 2.);
            let mix = |a: gdk::RGBA, b: gdk::RGBA| {
                let p = position;
                gdk::RGBA::new(
                    a.red() + (b.red() - a.red()) * p,
                    a.green() + (b.green() - a.green()) * p,
                    a.blue() + (b.blue() - a.blue()) * p,
                    (a.alpha() + (b.alpha() - a.alpha()) * p) * alpha,
                )
            };
            let mut off_border = t.rgba(if enabled && self.hovered.get() {
                "text-faint"
            } else {
                "border-strong"
            });
            let mut clear = off_border;
            clear.set_alpha(0.);
            off_border = mix(off_border, clear);
            snapshot.push_rounded_clip(&pill);
            snapshot.append_color(
                &mix(t.rgba("surface-sunken"), t.rgba("theme-accent")),
                &track,
            );
            snapshot.pop();
            snapshot.append_border(&pill, &[1.; 4], &[off_border; 4]);
            // `::before`: 2 px inside the 1 px border, sliding 13 px.
            let knob = graphene::Rect::new(3. + TRAVEL * position, y + 3., KNOB, KNOB);
            let round = gsk::RoundedRect::from_rect(knob, KNOB / 2.);
            let mut shadow = t.rgba("text");
            shadow.set_alpha(0.1 * alpha);
            snapshot.append_outset_shadow(&round, &shadow, 0., 1., 0., 1.);
            snapshot.push_rounded_clip(&round);
            snapshot.append_color(
                &mix(t.rgba("text-subtle"), t.rgba("theme-accent-ink")),
                &knob,
            );
            snapshot.pop();
            if widget.has_visible_focus() {
                let mut ring = t.rgba("theme-accent");
                ring.set_alpha(0.55);
                let outer = gsk::RoundedRect::from_rect(
                    graphene::Rect::new(-2., y - 2., WIDTH + 4., HEIGHT + 4.),
                    HEIGHT / 2. + 2.,
                );
                snapshot.append_border(&outer, &[2.; 4], &[ring; 4]);
            }
        }
    }
}

glib::wrapper! {
    pub struct TokenSwitch(ObjectSubclass<imp::TokenSwitch>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TokenSwitch {
    pub fn new(tokens: Rc<Tokens>, on: bool) -> Self {
        let widget: Self = glib::Object::new();
        *widget.imp().tokens.borrow_mut() = Some(tokens);
        widget.imp().on.set(on);
        widget.imp().position.set(if on { 1. } else { 0. });
        widget.update_checked();
        widget
    }

    pub fn is_on(&self) -> bool {
        self.imp().on.get()
    }

    pub fn connect_toggled(&self, f: impl Fn(bool) + 'static) {
        *self.imp().toggled.borrow_mut() = Some(Box::new(f));
    }

    /// Flip the switch as the user would, animating the knob.
    pub fn activate_toggle(&self) {
        if !self.is_sensitive() {
            return;
        }
        let on = !self.is_on();
        self.imp().on.set(on);
        self.update_checked();
        self.animate(on);
        if let Some(toggled) = self.imp().toggled.borrow().as_ref() {
            toggled(on);
        }
    }

    /// Reflect a value the app read back (no callback), animating the knob.
    pub fn activate_toggle_quietly(&self, on: bool) {
        self.imp().on.set(on);
        self.update_checked();
        self.animate(on);
    }

    pub fn set_tokens(&self, tokens: Rc<Tokens>) {
        *self.imp().tokens.borrow_mut() = Some(tokens);
        self.queue_draw();
    }

    fn update_checked(&self) {
        let state = if self.imp().on.get() {
            gtk::AccessibleTristate::True
        } else {
            gtk::AccessibleTristate::False
        };
        self.update_state(&[gtk::accessible::State::Checked(state)]);
    }

    fn animate(&self, on: bool) {
        let imp = self.imp();
        if let Some(id) = imp.tween.borrow_mut().take() {
            id.remove();
        }
        let target = if on { 1. } else { 0. };
        let settings = gtk::Settings::default();
        let Some(t) = imp.tokens.borrow().clone() else {
            return;
        };
        let animations = settings
            .map(|s| s.is_gtk_enable_animations())
            .unwrap_or(true);
        let Some(clock) = self.frame_clock().filter(|_| animations) else {
            imp.position.set(target);
            self.queue_draw();
            return;
        };
        let duration_us = f64::from(t.number("dur-3")) * 1000.;
        let [x1, y1, x2, y2] = t.easing("ease-standard");
        let curve = CubicBezier::new(x1, y1, x2, y2).expect("token easing");
        let from = imp.position.get();
        let start = clock.frame_time();
        let id = self.add_tick_callback(move |widget, clock| {
            let progress = ((clock.frame_time() - start) as f64 / duration_us).clamp(0., 1.);
            widget
                .imp()
                .position
                .set(from + (target - from) * curve.ease(progress));
            widget.queue_draw();
            if progress >= 1. {
                widget.imp().tween.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        *imp.tween.borrow_mut() = Some(id);
    }
}
