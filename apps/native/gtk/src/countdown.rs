//! Shipping's ScreenshotCountdown: a fixed-media scrim, real accessible
//! labels and the shared countdown motions. The capture owner retains its
//! generation; this surface never acquires, commits or restarts a capture.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use captures_app::capture_flow::{self, Countdown};
use captures_app::motion::{Motion, Pose};
use captures_capture::DisplayDescriptor;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};

use crate::tokens::Tokens;
use crate::{Host, emit};

pub const TITLE: &str = "Captures Screenshot Countdown";

mod imp {
    use super::*;

    pub struct Content {
        pub tokens: OnceCell<Rc<Tokens>>,
        pub labels: OnceCell<[gtk::Label; 3]>,
        pub scrim: Cell<Pose>,
        pub content: Cell<Pose>,
        pub animation: RefCell<Option<gtk::TickCallbackId>>,
        pub font_width: Cell<i32>,
    }

    impl Default for Content {
        fn default() -> Self {
            Self {
                tokens: OnceCell::new(),
                labels: OnceCell::new(),
                scrim: Cell::new(Pose::REST),
                content: Cell::new(Pose::REST),
                animation: RefCell::default(),
                font_width: Cell::new(0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Content {
        const NAME: &'static str = "CapturesCountdown";
        type Type = super::Content;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Alert);
        }
    }

    impl ObjectImpl for Content {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Content {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _: i32) {
            let tokens = self.tokens.get().expect("countdown tokens");
            let labels = self.labels.get().expect("countdown labels");
            let heading = (width as f32 * 0.016).clamp(
                tokens.number("countdown-label-min"),
                tokens.number("countdown-label-max"),
            );
            let number = (width as f32 * 0.26).clamp(
                tokens.number("countdown-number-min"),
                tokens.number("countdown-number-max"),
            );
            let sizes = [heading, number, tokens.number("text-md")];
            // Replacing font attributes queues another layout. Do it only
            // when responsive sizing changes, not on every animation frame.
            if self.font_width.replace(width) != width {
                for (index, (label, size)) in labels.iter().zip(sizes).enumerate() {
                    let attributes = pango::AttrList::new();
                    attributes.insert(pango::AttrSize::new_size_absolute(
                        (size * pango::SCALE as f32).round() as i32,
                    ));
                    attributes.insert(pango::AttrInt::new_weight(match index {
                        0 => pango::Weight::Medium,
                        1 => pango::Weight::Bold,
                        _ => pango::Weight::Normal,
                    }));
                    if index < 2 {
                        let token = if index == 0 {
                            "tracking-wide"
                        } else {
                            "tracking-tighter"
                        };
                        attributes.insert(pango::AttrInt::new_letter_spacing(
                            (tokens.em(token, size) * pango::SCALE as f32).round() as i32,
                        ));
                    }
                    label.set_attributes(Some(&attributes));
                }
            }
            // The shipping grid: heading, 0.04em + 0.95em + 0.06em
            // number line, then the hint's --s-7 top margin.
            let heading_height = labels[0].measure(gtk::Orientation::Vertical, width).1;
            let hint_height = labels[2].measure(gtk::Orientation::Vertical, width).1;
            let number_height = (number * 0.95).round() as i32;
            let before = (number * 0.04).round() as i32;
            let after = (number * 0.06 + tokens.number("s-7")).round() as i32;
            let total = heading_height + before + number_height + after + hint_height;
            let mut y = (height - total) as f32 / 2.;
            for (index, (label, height)) in labels
                .iter()
                .zip([heading_height, number_height, hint_height])
                .enumerate()
            {
                if index == 1 {
                    y += before as f32;
                }
                if index == 2 {
                    y += after as f32;
                }
                label.allocate(
                    width,
                    height,
                    -1,
                    Some(gsk::Transform::new().translate(&graphene::Point::new(0., y))),
                );
                y += height as f32;
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            let tokens = self.tokens.get().expect("countdown tokens");
            snapshot.push_opacity(self.scrim.get().opacity);
            snapshot.append_color(
                &tokens.rgba("glass-countdown-scrim"),
                &graphene::Rect::new(0., 0., width, height),
            );
            let pose = self.content.get();
            snapshot.push_opacity(pose.opacity);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                width / 2.,
                height / 2. + pose.translate_y as f32,
            ));
            snapshot.scale(pose.scale as f32, pose.scale as f32);
            snapshot.translate(&graphene::Point::new(-width / 2., -height / 2.));
            for label in self.labels.get().expect("countdown labels") {
                widget.snapshot_child(label, snapshot);
            }
            snapshot.restore();
            snapshot.pop();
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Content(ObjectSubclass<imp::Content>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Content {
    fn new(tokens: Rc<Tokens>, seconds: u8) -> Self {
        let content: Self = glib::Object::new();
        let labels = [
            gtk::Label::new(Some("SCREENSHOT IN")),
            gtk::Label::new(Some(&seconds.to_string())),
            gtk::Label::new(Some("Press Esc to cancel")),
        ];
        for (index, label) in labels.iter().enumerate() {
            label.add_css_class(if index == 1 {
                "countdown-number"
            } else {
                "countdown-copy"
            });
            label.set_parent(&content);
        }
        content.imp().labels.set(labels).expect("new labels");
        content.imp().tokens.set(tokens).ok().expect("new tokens");
        content.set_remaining(seconds);
        content
    }

    fn set_remaining(&self, seconds: u8) {
        self.imp().labels.get().expect("labels")[1].set_label(&seconds.to_string());
        self.queue_allocate();
        self.update_property(&[gtk::accessible::Property::Label(&format!(
            "Screenshot in {seconds}. Press Esc to cancel"
        ))]);
    }

    fn animate(&self, exiting: bool) {
        if let Some(animation) = self.imp().animation.borrow_mut().take() {
            animation.remove();
        }
        let tokens = self.imp().tokens.get().expect("tokens");
        let scrim = if exiting {
            Motion::CountdownOut
        } else {
            Motion::CountdownIn
        }
        .resolve(tokens.as_ref())
        .expect("countdown motion");
        let content = if exiting {
            Motion::CountdownContentOut
        } else {
            Motion::CountdownContentIn
        }
        .resolve(tokens.as_ref())
        .expect("countdown content motion");
        let started = Instant::now();
        let sample = move |widget: &Self| {
            let reduced = !widget.settings().is_gtk_enable_animations();
            let ms = started.elapsed().as_secs_f64() * 1000.;
            widget.imp().scrim.set(scrim.pose_at(ms, reduced));
            widget.imp().content.set(content.pose_at(ms, reduced));
            widget.queue_draw();
            if scrim.running(ms, reduced) || content.running(ms, reduced) {
                glib::ControlFlow::Continue
            } else {
                widget.imp().animation.borrow_mut().take();
                glib::ControlFlow::Break
            }
        };
        if sample(self) == glib::ControlFlow::Continue {
            *self.imp().animation.borrow_mut() =
                Some(self.add_tick_callback(move |widget, _| sample(widget)));
        }
    }
}

/// Wait on a monotonic deadline, then unmap the surface. The caller must
/// still wait for compositor settlement before acquiring pixels. Zero-delay
/// captures do not create a window; cancelling never publishes an artifact.
pub async fn wait(host: &Host, generation: u64, seconds: u8, display: Option<&DisplayDescriptor>) {
    if seconds == 0 || !capture_flow::is_current(generation) {
        return;
    }
    let deadline = Countdown::new(Instant::now(), seconds);
    let window = gtk::ApplicationWindow::builder()
        .application(&host.app)
        .title(TITLE)
        .decorated(false)
        .build();
    window.add_css_class("captures");
    window.add_css_class("countdown-window");
    let content = Content::new(host.tokens.borrow().clone(), seconds);
    window.set_child(Some(&content));
    if let Some(display) = display {
        let (width, height) = display.overlay_size();
        window.set_default_size(width as i32, height as i32);
        window.set_resizable(false);
        #[cfg(feature = "x11")]
        if let Some(placer) = crate::previews::x11::Placer::new(&WidgetExt::display(&window)) {
            let geometry = (display.x, display.y, display.width, display.height);
            window.connect_realize(move |window| {
                if let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() {
                    placer.cover(&surface, geometry);
                }
            });
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
    let finished = Rc::new(Cell::new(false));
    let closing = finished.clone();
    window.connect_close_request(move |_| {
        if !closing.get() {
            capture_flow::cancel(generation);
        }
        glib::Propagation::Proceed
    });
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            capture_flow::cancel(generation);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
    content.animate(false);
    window.present();
    emit("countdown", serde_json::json!({"remaining": seconds}));
    let mut remaining = seconds;
    while capture_flow::is_current(generation) {
        let next = deadline.remaining(Instant::now());
        if next == 0 {
            break;
        }
        if next != remaining {
            remaining = next;
            content.set_remaining(next);
            emit("countdown", serde_json::json!({"remaining": next}));
        }
        glib::timeout_future(Duration::from_millis(50)).await;
    }
    if !capture_flow::is_current(generation) {
        content.imp().labels.get().expect("labels")[2].set_label("Cancelling…");
        content.queue_allocate();
        content.update_property(&[gtk::accessible::Property::Label("Cancelling screenshot")]);
        content.animate(true);
        if content.settings().is_gtk_enable_animations() {
            glib::timeout_future(Duration::from_millis(180)).await;
        }
    }
    finished.set(true);
    window.close();
}
