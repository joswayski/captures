//! Small Captures widgets: shipping SVG icons stroked from
//! `captures_app::icons`, and a max-width column (`max-width` in CSS).
use std::cell::Cell;

use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod icon_imp {
    use super::*;

    #[derive(Default)]
    pub struct Icon {
        pub name: std::cell::RefCell<String>,
        pub side: Cell<i32>,
        pub stroke: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Icon {
        const NAME: &'static str = "CapturesIcon";
        type Type = super::Icon;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
            klass.set_css_name("captures-icon");
        }
    }

    impl ObjectImpl for Icon {}

    impl WidgetImpl for Icon {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let side = self.side.get();
            (side, side, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(polylines) = captures_app::icons::polylines(&self.name.borrow()) else {
                return;
            };
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
            #[allow(deprecated)] // `gtk_widget_get_color` needs GTK 4.10.
            let color = widget.style_context().color();
            cr.set_source_rgba(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
                color.alpha().into(),
            );
            let scale = f64::from(self.side.get()) / 24.;
            cr.translate(
                f64::from(w - self.side.get() as f32) / 2.,
                f64::from(h - self.side.get() as f32) / 2.,
            );
            cr.scale(scale, scale);
            cr.set_line_width(self.stroke.get());
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            for line in polylines {
                let mut points = line.iter();
                let Some(first) = points.next() else { continue };
                cr.move_to(first[0].into(), first[1].into());
                for point in points {
                    cr.line_to(point[0].into(), point[1].into());
                }
            }
            let _ = cr.stroke();
        }
    }
}

glib::wrapper! {
    /// A stroked shipping icon in the widget's CSS `color`.
    pub struct Icon(ObjectSubclass<icon_imp::Icon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Icon {
    pub fn new(name: &str, side: i32) -> Self {
        let icon: Self = glib::Object::new();
        *icon.imp().name.borrow_mut() = name.to_owned();
        icon.imp().side.set(side);
        icon.imp().stroke.set(1.8);
        icon
    }

    /// Stroke width in the icon's 24-unit space.
    pub fn with_stroke(self, stroke: f64) -> Self {
        self.imp().stroke.set(stroke);
        self
    }

    pub fn set_name(&self, name: &str) {
        *self.imp().name.borrow_mut() = name.to_owned();
        self.queue_draw();
    }
}

mod clamp_imp {
    use super::*;

    #[derive(Default)]
    pub struct MaxWidth {
        pub max: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MaxWidth {
        const NAME: &'static str = "CapturesMaxWidth";
        type Type = super::MaxWidth;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MaxWidth {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for MaxWidth {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            let max = self.max.get();
            if orientation == gtk::Orientation::Horizontal {
                let (min, natural, _, _) = child.measure(orientation, for_size);
                (min, natural.min(max).max(min), -1, -1)
            } else {
                let width = if for_size < 0 { -1 } else { for_size.min(max) };
                let (min, natural, _, _) = child.measure(orientation, width);
                (min, natural, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                // Shipping's `max-width` column stays at the start edge.
                child.allocate(width.min(self.max.get()), height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    /// One child laid out no wider than `max` pixels, start-aligned.
    pub struct MaxWidth(ObjectSubclass<clamp_imp::MaxWidth>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MaxWidth {
    pub fn new(child: &impl IsA<gtk::Widget>, max: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().max.set(max);
        child.set_parent(&widget);
        widget
    }
}
