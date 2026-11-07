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

mod split_imp {
    use super::*;

    #[derive(Default)]
    pub struct SplitColumn {
        pub top_min: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SplitColumn {
        const NAME: &'static str = "CapturesSplitColumn";
        type Type = super::SplitColumn;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for SplitColumn {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SplitColumn {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (mut min, mut natural) = (0, 0);
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                let (cmin, cnat, _, _) = c.measure(orientation, for_size);
                if orientation == gtk::Orientation::Horizontal {
                    min = min.max(cmin);
                    natural = natural.max(cnat);
                } else {
                    min += cmin;
                    natural += cnat;
                }
                child = c.next_sibling();
            }
            if orientation == gtk::Orientation::Vertical {
                min = min.max(self.top_min.get());
            }
            (min, natural.max(min), -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(top) = self.obj().first_child() else {
                return;
            };
            // `minmax(top_min, 40%) minmax(0, 1fr)`.
            let top_height = ((f64::from(height) * 0.4).round() as i32)
                .max(self.top_min.get())
                .max(top.measure(gtk::Orientation::Vertical, width).0)
                .min(height);
            top.allocate(width, top_height, -1, None);
            if let Some(bottom) = top.next_sibling() {
                let rest =
                    (height - top_height).max(bottom.measure(gtk::Orientation::Vertical, width).0);
                let at = gtk::gsk::Transform::new()
                    .translate(&graphene::Point::new(0., top_height as f32));
                bottom.allocate(width, rest, -1, Some(at));
            }
        }
    }
}

glib::wrapper! {
    /// Two children stacked: the first takes 40% of the height (at least
    /// `top_min`), the second the rest, as a two-row CSS grid.
    pub struct SplitColumn(ObjectSubclass<split_imp::SplitColumn>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SplitColumn {
    pub fn new(top: &impl IsA<gtk::Widget>, bottom: &impl IsA<gtk::Widget>, top_min: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().top_min.set(top_min);
        top.set_parent(&widget);
        bottom.set_parent(&widget);
        widget
    }
}

mod thumbnail_imp {
    use super::*;

    #[derive(Default)]
    pub struct Thumbnail {
        pub texture: std::cell::RefCell<Option<gtk::gdk::Texture>>,
        pub cover: std::cell::Cell<bool>,
        pub dimmed: std::cell::Cell<bool>,
        /// Collapsed-pile media blur (`blur(…px)`, no dimming).
        pub blur: std::cell::Cell<f32>,
        /// The baked filtered image: (blur radius, brightness, texture),
        /// built once per texture and filter.
        pub dimmed_texture: std::cell::RefCell<Option<(f32, f32, gtk::gdk::Texture)>>,
        /// A Close streak: horizontal stretch and the stepped motion blur
        /// (`None` keeps the base look), over the hover look or not.
        pub streak: std::cell::Cell<Option<super::Streak>>,
        /// Baked streak steps by index, for the current base look.
        pub streak_textures: std::cell::RefCell<Vec<(usize, gtk::gdk::Texture)>>,
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
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let Some((texture, mut rect)) = widget.drawn() else {
                return;
            };
            let cover = self.cover.get();
            let texture = match self.streak.get() {
                Some(streak) => {
                    // Stretch about the centre, inside the card's clip.
                    let width = rect.width() * streak.scale_x;
                    rect = graphene::Rect::new(
                        rect.x() - (width - rect.width()) / 2.,
                        rect.y(),
                        width,
                        rect.height(),
                    );
                    match streak.step {
                        Some(step) => widget.streak_texture(step),
                        None => texture,
                    }
                }
                None => texture,
            };
            if cover {
                snapshot.push_clip(&graphene::Rect::new(0., 0., w, h));
            }
            snapshot.append_texture(&texture, &rect);
            if cover {
                snapshot.pop();
            }
        }
    }
}

impl Thumbnail {
    /// The texture as drawn now (plain, or the baked hover / pile filter)
    /// and where it lands in the widget, before any clip.
    pub fn drawn(&self) -> Option<(gtk::gdk::Texture, graphene::Rect)> {
        self.imp().drawn()
    }

    fn streak_texture(&self, step: usize) -> gtk::gdk::Texture {
        let imp = self.imp();
        if let Some((_, baked)) = imp
            .streak_textures
            .borrow()
            .iter()
            .find(|(index, _)| *index == step)
        {
            return baked.clone();
        }
        let streak = imp.streak.get().expect("streaking");
        let (base, _) = imp.drawn_with(streak.keep_look).expect("a texture");
        let scale = imp.texture_scale().unwrap_or(1.);
        let sigma = STREAK_BLURS[step] / scale;
        let baked = dim_and_blur(&base, sigma, 1., false);
        imp.streak_textures.borrow_mut().push((step, baked.clone()));
        baked
    }
}

mod thumbnail_draw {
    use super::*;
    use captures_app::preview_chrome;

    impl thumbnail_imp::Thumbnail {
        /// Texture pixels per logical pixel's inverse: the draw scale.
        pub fn texture_scale(&self) -> Option<f32> {
            let texture = self.texture.borrow().clone()?;
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            if w <= 0. || h <= 0. || tw <= 0. || th <= 0. {
                return None;
            }
            Some(if self.cover.get() {
                (w / tw).max(h / th)
            } else {
                (w / tw).min(h / th)
            })
        }

        pub fn drawn(&self) -> Option<(gtk::gdk::Texture, graphene::Rect)> {
            self.drawn_with(true)
        }

        /// `hover`: false draws the plain image even while dimmed (a Close
        /// streak leaves the locked hover look at its first step).
        pub fn drawn_with(&self, hover: bool) -> Option<(gtk::gdk::Texture, graphene::Rect)> {
            let texture = self.texture.borrow().clone()?;
            // `object-fit: contain` or `cover`, centered.
            let scale = self.texture_scale()?;
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            // Mini-preview hover: `blur(2px) brightness(.5)` and `scale(1.015)`.
            let dimmed = self.dimmed.get();
            let scale = if dimmed {
                scale * preview_chrome::HOVER_MEDIA_SCALE as f32
            } else {
                scale
            };
            let (dw, dh) = (tw * scale, th * scale);
            // The blurred, dimmed image is baked once and then drawn as a
            // plain texture: the Cairo renderer blurs only the damaged clip,
            // which left seams wherever the hover controls repainted.
            let dimmed = dimmed && hover;
            let filter = if dimmed {
                Some((
                    preview_chrome::HOVER_MEDIA_BLUR as f32,
                    preview_chrome::HOVER_MEDIA_BRIGHTNESS as f32,
                ))
            } else if self.blur.get() > 0. {
                Some((self.blur.get(), 1.))
            } else {
                None
            };
            let texture = if let Some((blur, brightness)) = filter {
                let sigma = blur / scale;
                let mut cached = self.dimmed_texture.borrow_mut();
                match cached.as_ref() {
                    Some((radius, light, baked)) if *radius == sigma && *light == brightness => {
                        baked.clone()
                    }
                    _ => {
                        let baked = super::dim_and_blur(&texture, sigma, brightness, true);
                        *cached = Some((sigma, brightness, baked.clone()));
                        baked
                    }
                }
            } else {
                texture
            };
            Some((
                texture,
                graphene::Rect::new((w - dw) / 2., (h - dh) / 2., dw, dh),
            ))
        }
    }
}

/// Shipping's stepped Close streak blurs (`#thumbnail-motion-blur-a/b/c`
/// `stdDeviation` on x), as the wgpu workbench's `STREAK_BLURS`.
const STREAK_BLURS: [f32; 3] = [3.5, 8., 14.];

/// The streak step `blur` (the `PreviewDismissStreak` pose's blur) shows:
/// each `url()` filter swaps in at its keyframe midpoint, as CSS does for
/// filters it cannot interpolate.
pub fn streak_step(blur: f64) -> Option<usize> {
    let mut previous = captures_app::preview_chrome::HOVER_MEDIA_BLUR as f32;
    let mut step = None;
    for (index, &sigma) in STREAK_BLURS.iter().enumerate() {
        if blur as f32 >= (previous + sigma) / 2. {
            step = Some(index);
        }
        previous = sigma;
    }
    step
}

/// A Close or Clear all streak frame on a [`Thumbnail`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Streak {
    pub scale_x: f32,
    pub step: Option<usize>,
    /// Clear all keeps the card's look under every step; Close drops the
    /// locked hover look from the first step.
    pub keep_look: bool,
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

    /// The source texture's pixel size.
    pub fn texture_size(&self) -> Option<(f64, f64)> {
        let texture = self.imp().texture.borrow().clone()?;
        Some((f64::from(texture.width()), f64::from(texture.height())))
    }

    /// A collapsed pile's rear-card media blur in logical pixels (0: none).
    pub fn set_blur(&self, blur: f32) {
        if self.imp().blur.replace(blur) != blur {
            self.queue_draw();
        }
    }

    pub fn set_dimmed(&self, dimmed: bool) {
        if self.imp().dimmed.replace(dimmed) != dimmed {
            self.queue_draw();
        }
    }

    /// Fill the widget (`object-fit: cover`) instead of fitting inside it.
    pub fn set_cover(&self, cover: bool) {
        self.imp().cover.set(cover);
        self.queue_draw();
    }

    pub fn set_texture(&self, texture: Option<gtk::gdk::Texture>) {
        *self.imp().texture.borrow_mut() = texture;
        self.imp().dimmed_texture.borrow_mut().take();
        self.imp().streak_textures.borrow_mut().clear();
        self.queue_draw();
    }

    pub fn set_streak(&self, streak: Option<Streak>) {
        let imp = self.imp();
        let old = imp.streak.replace(streak);
        if old.map(|s| s.keep_look) != streak.map(|s| s.keep_look) {
            imp.streak_textures.borrow_mut().clear();
        }
        if old != streak {
            self.queue_draw();
        }
    }
}

/// Shipping's hover media filter, `blur(σ) brightness(b)`, baked into a new
/// texture: three box passes approximate the Gaussian (σ in texture pixels).
/// Without `vertical` it blurs along rows only (the Close streak).
fn dim_and_blur(
    texture: &gtk::gdk::Texture,
    sigma: f32,
    brightness: f32,
    vertical: bool,
) -> gtk::gdk::Texture {
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let stride = width * 4;
    // `download` writes Cairo's native-endian premultiplied ARGB32.
    let mut pixels = vec![0u8; stride * height];
    texture.download(&mut pixels, stride);
    // n box passes of width w have variance n(w² − 1)/12: three per axis,
    // or six along rows alone.
    let passes = if vertical { 3. } else { 6. };
    let box_width = (12.0 * sigma * sigma / passes + 1.0).sqrt();
    let radius = ((box_width - 1.0) / 2.0).round().max(0.0) as usize;
    if radius > 0 {
        let mut scratch = vec![0u8; pixels.len()];
        for _ in 0..3 {
            box_pass(&pixels, &mut scratch, (width, height), radius, true);
            box_pass(&scratch, &mut pixels, (width, height), radius, !vertical);
        }
    }
    for pixel in pixels.chunks_exact_mut(4) {
        for channel in &mut pixel[..3] {
            *channel = (f32::from(*channel) * brightness).round() as u8;
        }
    }
    gtk::gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gtk::gdk::MemoryFormat::B8g8r8a8Premultiplied,
        &glib::Bytes::from_owned(pixels),
        stride,
    )
    .upcast()
}

/// One sliding-window box blur along rows or columns, clamped at the edges.
fn box_pass(
    src: &[u8],
    dst: &mut [u8],
    (width, height): (usize, usize),
    radius: usize,
    rows: bool,
) {
    let (len, lines, step, line_step) = if rows {
        (width, height, 4, width * 4)
    } else {
        (height, width, width * 4, 4)
    };
    let span = (2 * radius + 1) as u32;
    let last = len as isize - 1;
    for line in 0..lines {
        let base = line * line_step;
        for channel in 0..4 {
            let at = |i: isize| u32::from(src[base + i.clamp(0, last) as usize * step + channel]);
            let r = radius as isize;
            let mut sum: u32 = (-r..=r).map(at).sum();
            for i in 0..len as isize {
                dst[base + i as usize * step + channel] = ((sum + span / 2) / span) as u8;
                sum += at(i + r + 1);
                sum -= at(i - r);
            }
        }
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
