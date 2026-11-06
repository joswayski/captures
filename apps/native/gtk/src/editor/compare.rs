//! Shipping's `CompressionPreview` over the screenshot canvas (`is-embed
//! is-cover`): while Compress or Maximum file size is chosen and Export
//! settings are open, the right of a draggable divider shows the encoded file
//! and the left the live edit. The canvas draws the After texture and the
//! divider into its retained nodes; the badges, Hide, the round handle and the
//! full-width range are real widgets over the image for AT-SPI.
//! `captures_app::compression_compare` owns the copy, badges and bounds.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::compression_compare as compare;
use gtk::prelude::*;
use gtk::{gdk, graphene};

pub(super) struct Compare {
    pub(super) before: gtk::Label,
    pub(super) after: gtk::Box,
    after_text: gtk::Label,
    savings: gtk::Label,
    pub(super) hide: gtk::Button,
    pub(super) handle: gtk::Button,
    pub(super) range: gtk::Scale,
    /// The divider's place across the image, 0.06–0.94.
    pub(super) split: Rc<Cell<f64>>,
    /// The decoded encode Save would write, uploaded once per encode.
    pub(super) texture: RefCell<Option<gdk::Texture>>,
    pub(super) dismissed: Cell<bool>,
    /// The image's on-screen width, for handle drags.
    image_width: Cell<f64>,
    syncing: Rc<Cell<bool>>,
}

fn pill(class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("compare-pill");
    label.add_css_class(class);
    label
}

/// A button whose visible text is presentational: GTK 4.14 names a button
/// after its label child, which would hide shipping's `aria-label`.
fn labeled(text: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let label = gtk::Label::new(Some(text));
    label.set_accessible_role(gtk::AccessibleRole::Presentation);
    button.set_child(Some(&label));
    button
}

impl Compare {
    /// `moved` runs after the range moves the split, to redraw the canvas.
    /// The editor drags the handle from the overlay, whose coordinates stay
    /// put while the handle follows the pointer.
    pub(super) fn new(moved: impl Fn() + 'static) -> Rc<Self> {
        let moved: Rc<dyn Fn()> = Rc::new(moved);
        let split = Rc::new(Cell::new(compare::DEFAULT_SPLIT));
        let syncing = Rc::new(Cell::new(false));
        let before = pill("compare-before");
        before.set_can_target(false);
        let after = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        after.add_css_class("compare-pill");
        after.set_can_target(false);
        let after_text = gtk::Label::new(None);
        let savings = gtk::Label::new(None);
        savings.add_css_class("compare-savings");
        after.append(&after_text);
        after.append(&savings);
        let hide = labeled(compare::DISMISS);
        hide.add_css_class("compare-hide");
        hide.update_property(&[gtk::accessible::Property::Label(compare::DISMISS_LABEL)]);
        let handle = labeled(compare::HANDLE_GLYPH);
        handle.add_css_class("compare-handle");
        handle.set_size_request(compare::HANDLE_SIZE as i32, compare::HANDLE_SIZE as i32);
        handle.set_cursor_from_name(Some("ew-resize"));
        handle.update_property(&[gtk::accessible::Property::Label(compare::HANDLE_LABEL)]);
        let range = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            compare::MIN_SPLIT * 100.,
            compare::MAX_SPLIT * 100.,
            0.1,
        );
        range.add_css_class("compare-range");
        range.set_draw_value(false);
        range.set_value(compare::DEFAULT_SPLIT * 100.);
        range.update_property(&[gtk::accessible::Property::Label(compare::RANGE_LABEL)]);
        {
            let (split, moved, syncing) = (split.clone(), moved.clone(), syncing.clone());
            range.connect_value_changed(move |range| {
                if !syncing.get() {
                    split.set(compare::clamp_split(range.value() / 100.));
                    moved();
                }
            });
        }
        Rc::new(Compare {
            before,
            after,
            after_text,
            savings,
            hide,
            handle,
            range,
            split,
            texture: RefCell::default(),
            dismissed: Cell::new(false),
            image_width: Cell::new(1.),
            syncing,
        })
    }

    pub(super) fn set_split(&self, split: f64) {
        let split = compare::clamp_split(split);
        self.split.set(split);
        self.syncing.set(true);
        self.range.set_value(split * 100.);
        self.syncing.set(false);
    }

    /// Show or hide the widgets and fill the badges.
    pub(super) fn sync(
        &self,
        visible: bool,
        before: Option<u64>,
        after: Option<u64>,
        processing: bool,
        drawing: bool,
    ) {
        let badges = compare::badges(before, after, processing);
        self.before.set_label(&badges.before);
        self.after_text.set_label(&badges.after);
        self.savings
            .set_label(badges.savings.as_deref().unwrap_or_default());
        self.savings.set_visible(badges.savings.is_some());
        let split = visible && self.texture.borrow().is_some();
        for widget in [
            self.before.upcast_ref::<gtk::Widget>(),
            self.after.upcast_ref(),
            self.hide.upcast_ref(),
        ] {
            widget.set_visible(visible);
        }
        self.handle.set_visible(split);
        self.range.set_visible(split);
        self.handle.set_sensitive(!processing);
        // A drawing tool keeps the canvas; only the handle moves the split.
        self.range.set_sensitive(!processing && !drawing);
        self.range.set_can_target(!drawing);
    }

    pub(super) fn owns(&self, widget: &gtk::Widget) -> bool {
        [
            self.before.upcast_ref::<gtk::Widget>(),
            self.after.upcast_ref(),
            self.hide.upcast_ref(),
            self.handle.upcast_ref(),
            self.range.upcast_ref(),
        ]
        .contains(&widget)
    }

    /// Where `widget` sits over the visible part of the image `rect`.
    pub(super) fn position(
        &self,
        widget: &gtk::Widget,
        rect: &graphene::Rect,
    ) -> Option<gdk::Rectangle> {
        self.image_width.set(f64::from(rect.width()));
        let (_, natural) = widget.preferred_size();
        let (w, h) = (natural.width(), natural.height());
        let inset = 12;
        let (left, top) = (rect.x() as i32, rect.y() as i32);
        let (right, bottom) = (
            (rect.x() + rect.width()) as i32,
            (rect.y() + rect.height()) as i32,
        );
        let divider = rect.x() + rect.width() * self.split.get() as f32;
        let at = |x: i32, y: i32, w: i32, h: i32| Some(gdk::Rectangle::new(x, y, w, h));
        if widget == self.before.upcast_ref::<gtk::Widget>() {
            at(left + inset, bottom - inset - h, w, h)
        } else if widget == self.after.upcast_ref::<gtk::Widget>() {
            at(right - inset - w, bottom - inset - h, w, h)
        } else if widget == self.hide.upcast_ref::<gtk::Widget>() {
            at(right - inset - w, top + inset, w, h)
        } else if widget == self.handle.upcast_ref::<gtk::Widget>() {
            let side = compare::HANDLE_SIZE as i32;
            at(
                divider as i32 - side / 2,
                (rect.y() + rect.height() / 2.) as i32 - side / 2,
                side,
                side,
            )
        } else {
            // The full-width range along the bottom 28 px.
            at(left, bottom - 28, right - left, 28)
        }
    }
}
