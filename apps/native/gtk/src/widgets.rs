//! Small Captures widgets: shipping SVG icons stroked from
//! `captures_app::icons`, a max-width column (`max-width` in CSS), and a
//! contain-fit thumbnail that draws one GPU texture.
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
        if *self.imp().name.borrow() != name {
            *self.imp().name.borrow_mut() = name.to_owned();
            self.queue_draw();
        }
    }
}

mod clamp_imp {
    use super::*;

    #[derive(Default)]
    pub struct MaxWidth {
        pub max: Cell<i32>,
        pub center: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MaxWidth {
        const NAME: &'static str = "CapturesMaxWidth";
        type Type = super::MaxWidth;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // Layout only: AT-SPI shows a plain group, not the type name.
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for MaxWidth {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl MaxWidth {
        pub fn limit(&self, child: &gtk::Widget) -> i32 {
            match self.max.get() {
                super::MaxWidth::NATURAL => child.measure(gtk::Orientation::Horizontal, -1).1,
                max => max,
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
            let max = self.limit(&child);
            if orientation == gtk::Orientation::Horizontal {
                let (min, natural, _, _) = child.measure(orientation, for_size);
                (min, natural.min(max).max(min), -1, -1)
            } else {
                let width = if for_size < 0 { max } else { for_size.min(max) };
                let (min, natural, _, _) = child.measure(orientation, width);
                (min, natural, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                // Shipping's `max-width` column stays at the start edge, or
                // centers with `margin: 0 auto`.
                let inner = width.min(self.limit(&child));
                let x = if self.center.get() {
                    (width - inner) / 2
                } else {
                    0
                };
                let at = gtk::gsk::Transform::new().translate(&graphene::Point::new(x as f32, 0.));
                child.allocate(inner, height, baseline, Some(at));
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
    /// Cap at the child's own natural width, such as a wrapping label's
    /// `max-width-chars`, which GTK otherwise ignores once the label is
    /// allocated more.
    pub const NATURAL: i32 = -1;

    pub fn new(child: &impl IsA<gtk::Widget>, max: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().max.set(max);
        child.set_parent(&widget);
        widget
    }

    /// Like [`MaxWidth::new`], centered (`margin: 0 auto`).
    pub fn centered(child: &impl IsA<gtk::Widget>, max: i32) -> Self {
        let widget = Self::new(child, max);
        widget.imp().center.set(true);
        widget
    }
}

mod thumbnail_imp {
    use super::*;

    #[derive(Default)]
    pub struct Thumbnail {
        pub texture: std::cell::RefCell<Option<gtk::gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Thumbnail {
        const NAME: &'static str = "CapturesThumbnail";
        type Type = super::Thumbnail;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
            klass.set_css_name("captures-thumbnail");
        }
    }

    impl ObjectImpl for Thumbnail {}

    impl WidgetImpl for Thumbnail {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            if w <= 0. || h <= 0. || tw <= 0. || th <= 0. {
                return;
            }
            // `object-fit: contain`, centered.
            let scale = (w / tw).min(h / th);
            let (dw, dh) = (tw * scale, th * scale);
            snapshot.append_texture(
                &texture,
                &graphene::Rect::new((w - dw) / 2., (h - dh) / 2., dw, dh),
            );
        }
    }
}

glib::wrapper! {
    /// A texture drawn `object-fit: contain`. Swapping the texture redraws
    /// only this widget; the image is never re-decoded to repaint.
    pub struct Thumbnail(ObjectSubclass<thumbnail_imp::Thumbnail>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Thumbnail {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Thumbnail {
    pub fn has_texture(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    pub fn set_texture(&self, texture: Option<gtk::gdk::Texture>) {
        *self.imp().texture.borrow_mut() = texture;
        self.queue_draw();
    }
}

/// A button with a leading shipping icon and a label (`.ui-btn svg` at 15 px).
pub fn icon_button(icon: &str, label: &str, classes: &[&str]) -> (gtk::Button, Icon, gtk::Label) {
    let button = gtk::Button::new();
    for class in classes {
        button.add_css_class(class);
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    let glyph = Icon::new(icon, 15).with_stroke(1.7);
    glyph.set_valign(gtk::Align::Center);
    let text = gtk::Label::new(Some(label));
    content.append(&glyph);
    content.append(&text);
    button.set_child(Some(&content));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    (button, glyph, text)
}

type WidthChanged = Box<dyn Fn(i32)>;

mod watch_imp {
    use super::*;

    #[derive(Default)]
    pub struct WidthWatch {
        pub width: Cell<i32>,
        pub changed: std::cell::RefCell<Option<WidthChanged>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WidthWatch {
        const NAME: &'static str = "CapturesWidthWatch";
        type Type = super::WidthWatch;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for WidthWatch {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for WidthWatch {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.obj()
                .first_child()
                .map_or(gtk::SizeRequestMode::ConstantSize, |child| {
                    child.request_mode()
                })
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.obj()
                .first_child()
                .map_or((0, 0, -1, -1), |child| child.measure(orientation, for_size))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if self.width.replace(width) != width
                && let Some(changed) = self.changed.borrow().as_ref()
            {
                changed(width);
            }
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    /// A transparent wrapper that reports its allocated width when it
    /// changes, for layouts that derive a model from width (grid columns).
    pub struct WidthWatch(ObjectSubclass<watch_imp::WidthWatch>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WidthWatch {
    pub fn new(child: &impl IsA<gtk::Widget>, changed: impl Fn(i32) + 'static) -> Self {
        let widget: Self = glib::Object::new();
        *widget.imp().changed.borrow_mut() = Some(Box::new(changed));
        child.set_parent(&widget);
        widget
    }
}

type Draw = Box<dyn Fn(&gtk::Snapshot, f32, f32)>;

mod area_imp {
    use super::*;

    #[derive(Default)]
    pub struct SnapshotArea {
        pub draw: std::cell::RefCell<Option<Draw>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SnapshotArea {
        const NAME: &'static str = "CapturesSnapshotArea";
        type Type = super::SnapshotArea;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("captures-area");
        }
    }

    impl ObjectImpl for SnapshotArea {}

    impl WidgetImpl for SnapshotArea {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            if let Some(draw) = self.draw.borrow().as_ref() {
                draw(snapshot, widget.width() as f32, widget.height() as f32);
            }
        }
    }
}

glib::wrapper! {
    /// A retained surface that records its render nodes with a callback.
    /// GTK keeps the nodes until `queue_draw`, so pointer motion over other
    /// widgets never re-records it, and textures it appends are uploaded once
    /// and then only transformed.
    pub struct SnapshotArea(ObjectSubclass<area_imp::SnapshotArea>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SnapshotArea {
    pub fn new(draw: impl Fn(&gtk::Snapshot, f32, f32) + 'static) -> Self {
        let widget: Self = glib::Object::new();
        *widget.imp().draw.borrow_mut() = Some(Box::new(draw));
        widget
    }

    pub fn set_draw(&self, draw: impl Fn(&gtk::Snapshot, f32, f32) + 'static) {
        *self.imp().draw.borrow_mut() = Some(Box::new(draw));
        self.queue_draw();
    }
}
