//! Shipping's per-layer settings card (`.screenshot-layer-menu-panel`, opened
//! from a row's ⋯): Appearance (Blend mode, Opacity), Transform for image
//! layers, Arrange, Combine, and the Duplicate / Delete footer. The card is
//! parented to the Layers panel rather than the row, because every committed
//! edit rebuilds the rows; `sync` keeps its values and enabled states in step
//! with the document the worker publishes.
use std::cell::Cell;
use std::rc::Rc;

use captures_app::editor::{Element, ImageTransform, LayerEdit};
use captures_app::editor_layers::{BLEND_MODES, menu};
use captures_app::editor_session::Request;
use gtk::prelude::*;

use super::controls::Choice;
use super::properties::RangeSlider;
use crate::widgets::Icon;

/// What the card asks the editor to do.
pub(super) enum Action {
    /// An edit of this layer; `live` folds slider drags into one undo step.
    Layer {
        edit: LayerEdit,
        live: Option<&'static str>,
        close: bool,
    },
    /// A document request; the editor selects `select` afterwards.
    Document {
        request: Request,
        select: Option<String>,
    },
}

/// The document facts the enabled states depend on.
pub(super) struct Caps<'a> {
    pub(super) merge_down_ids: &'a [String],
    pub(super) can_merge_visible: bool,
    pub(super) can_flatten: bool,
}

pub(super) struct LayerMenu {
    pub(super) id: String,
    pub(super) popover: gtk::Popover,
    blend: Rc<Choice>,
    opacity: RangeSlider,
    front: gtk::Button,
    back: gtk::Button,
    merge_down: gtk::Button,
    merge_visible: gtk::Button,
    flatten: gtk::Button,
    delete: gtk::Button,
    syncing: Rc<Cell<bool>>,
}

fn section(title: &str, class: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("layer-menu-section");
    section.add_css_class(class);
    let heading = gtk::Label::new(Some(&title.to_uppercase()));
    heading.add_css_class("layer-menu-title");
    heading.set_xalign(0.);
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    heading.update_property(&[gtk::accessible::Property::Label(title)]);
    section.append(&heading);
    section
}

/// `.screenshot-layer-menu-action` (or `-tile`): an icon and a label.
fn action(icon: &str, label: &str, name: &str, tip: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::new();
    crate::widgets::action_cursor(&button);
    button.add_css_class(class);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let glyph = Icon::new(icon, 15).with_stroke(1.8);
    glyph.add_css_class("layer-menu-icon");
    row.append(&glyph);
    let caption = gtk::Label::new(Some(label));
    caption.set_xalign(0.);
    caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
    caption.set_accessible_role(gtk::AccessibleRole::Presentation);
    row.append(&caption);
    button.set_child(Some(&row));
    button.set_tooltip_text(Some(tip));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

impl LayerMenu {
    pub(super) fn new(element: &Element, name: &str, act: impl Fn(Action) + 'static) -> Rc<Self> {
        let act: Rc<dyn Fn(Action)> = Rc::new(act);
        let syncing = Rc::new(Cell::new(false));
        let id = element.base().id.clone();
        let popover = gtk::Popover::new();
        popover.add_css_class("captures-menu");
        popover.add_css_class("layer-menu");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Left);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.set_size_request(280, -1);
        card.set_accessible_role(gtk::AccessibleRole::Dialog);
        card.update_property(&[gtk::accessible::Property::Label(&menu::settings_label(
            name,
        ))]);

        // Appearance stays outside the scroll region, as in shipping.
        let appearance = section(menu::APPEARANCE, "layer-menu-static");
        let legend = gtk::Label::new(Some(menu::BLEND_MODE));
        legend.add_css_class("layer-menu-field-label");
        legend.set_xalign(0.);
        let blend = {
            let act = act.clone();
            let syncing = syncing.clone();
            Choice::new(Some(&legend), menu::BLEND_MODE, false, move |blend_mode| {
                if !syncing.get() {
                    act(Action::Layer {
                        edit: LayerEdit::BlendMode { blend_mode },
                        live: None,
                        close: false,
                    });
                }
            })
        };
        blend.set_options(
            BLEND_MODES
                .iter()
                .map(|(value, label)| ((*value).to_owned(), (*label).to_owned()))
                .collect(),
        );
        appearance.append(&blend.root);
        let opacity = RangeSlider::new(
            menu::OPACITY,
            menu::OPACITY_LABEL,
            (0., 100.),
            element.base().opacity,
            &[],
            |v| format!("{}%", v.round()),
        );
        {
            let act = act.clone();
            let syncing = syncing.clone();
            opacity.scale.connect_value_changed(move |scale| {
                if !syncing.get() {
                    act(Action::Layer {
                        edit: LayerEdit::Opacity {
                            opacity: scale.value().round(),
                        },
                        live: Some("layer-opacity"),
                        close: false,
                    });
                }
            });
        }
        appearance.append(&opacity.root);
        card.append(&appearance);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let layer = |edit: LayerEdit, close: bool| {
            let act = act.clone();
            move |_: &gtk::Button| {
                act(Action::Layer {
                    edit: edit.clone(),
                    live: None,
                    close,
                })
            }
        };
        if matches!(element, Element::Image(_)) {
            let transform = section(menu::TRANSFORM, "layer-menu-transform");
            let grid = gtk::Grid::new();
            grid.set_row_spacing(6);
            grid.set_column_spacing(6);
            grid.set_column_homogeneous(true);
            grid.set_accessible_role(gtk::AccessibleRole::Group);
            grid.update_property(&[gtk::accessible::Property::Label(&format!(
                "Image transforms for {name}"
            ))]);
            for (index, (icon, label, aria, tip, transform)) in [
                (
                    "rotate-counterclockwise",
                    menu::ROTATE_LEFT,
                    "Rotate image counterclockwise",
                    menu::ROTATE_LEFT_TIP,
                    ImageTransform::RotateCounterclockwise,
                ),
                (
                    "rotate-clockwise",
                    menu::ROTATE_RIGHT,
                    "Rotate image clockwise",
                    menu::ROTATE_RIGHT_TIP,
                    ImageTransform::RotateClockwise,
                ),
                (
                    "flip-horizontal",
                    menu::FLIP_HORIZONTAL,
                    "Flip image horizontally",
                    menu::FLIP_HORIZONTAL_TIP,
                    ImageTransform::FlipHorizontal,
                ),
                (
                    "flip-vertical",
                    menu::FLIP_VERTICAL,
                    "Flip image vertically",
                    menu::FLIP_VERTICAL_TIP,
                    ImageTransform::FlipVertical,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let tile = action(icon, label, aria, tip, "layer-menu-tile");
                tile.connect_clicked(layer(LayerEdit::ImageTransform { transform }, false));
                grid.attach(&tile, (index % 2) as i32, (index / 2) as i32, 1, 1);
            }
            transform.append(&grid);
            body.append(&transform);
        }

        let arrange = section(menu::ARRANGE, "layer-menu-arrange");
        let front = action(
            "bring-front",
            menu::BRING_FRONT,
            menu::BRING_FRONT,
            menu::BRING_FRONT_TIP,
            "layer-menu-action",
        );
        front.connect_clicked(layer(LayerEdit::Arrange { front: true }, true));
        let back = action(
            "send-back",
            menu::SEND_BACK,
            menu::SEND_BACK,
            menu::SEND_BACK_TIP,
            "layer-menu-action",
        );
        back.connect_clicked(layer(LayerEdit::Arrange { front: false }, true));
        arrange.append(&front);
        arrange.append(&back);
        body.append(&arrange);

        let combine = section(menu::COMBINE, "layer-menu-combine");
        let document = |make: fn(&str, String) -> Request| {
            let act = act.clone();
            let id = id.clone();
            move |_: &gtk::Button| {
                let new_id = uuid::Uuid::new_v4().to_string();
                act(Action::Document {
                    request: make(&id, new_id.clone()),
                    select: Some(new_id),
                })
            }
        };
        let merge_down = action(
            "merge-down",
            menu::MERGE_DOWN,
            menu::MERGE_DOWN,
            menu::MERGE_DOWN_TIP,
            "layer-menu-action",
        );
        merge_down.connect_clicked(document(|id, new_id| Request::MergeDown {
            id: id.to_owned(),
            new_id,
        }));
        let merge_visible = action(
            "merge-visible",
            menu::MERGE_VISIBLE,
            menu::MERGE_VISIBLE,
            menu::MERGE_VISIBLE_TIP,
            "layer-menu-action",
        );
        merge_visible.connect_clicked(document(|_, new_id| Request::MergeVisible { new_id }));
        let flatten = action(
            "flatten",
            menu::FLATTEN,
            menu::FLATTEN,
            menu::FLATTEN_TIP,
            "layer-menu-action",
        );
        flatten.connect_clicked(document(|_, new_id| Request::Flatten { new_id }));
        combine.append(&merge_down);
        combine.append(&merge_visible);
        combine.append(&flatten);
        body.append(&combine);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(440)
            .child(&body)
            .build();
        card.append(&scroller);

        let footer = gtk::Box::new(gtk::Orientation::Vertical, 4);
        footer.add_css_class("layer-menu-footer");
        let duplicate = action(
            "duplicate",
            menu::DUPLICATE,
            menu::DUPLICATE,
            menu::DUPLICATE_TIP,
            "layer-menu-action",
        );
        {
            let act = act.clone();
            let id = id.clone();
            duplicate.connect_clicked(move |_| {
                let new_id = uuid::Uuid::new_v4().to_string();
                act(Action::Document {
                    request: Request::Layer {
                        id: id.clone(),
                        edit: LayerEdit::Duplicate {
                            new_id: new_id.clone(),
                        },
                    },
                    select: Some(new_id),
                });
            });
        }
        let delete = action(
            "trash",
            menu::DELETE,
            menu::DELETE,
            menu::DELETE_TIP,
            "layer-menu-action",
        );
        delete.add_css_class("danger");
        {
            let act = act.clone();
            let id = id.clone();
            delete.connect_clicked(move |_| {
                act(Action::Document {
                    request: Request::Layer {
                        id: id.clone(),
                        edit: LayerEdit::Delete,
                    },
                    select: None,
                })
            });
        }
        footer.append(&duplicate);
        footer.append(&delete);
        card.append(&footer);
        popover.set_child(Some(&card));

        Rc::new(LayerMenu {
            id,
            popover,
            blend,
            opacity,
            front,
            back,
            merge_down,
            merge_visible,
            flatten,
            delete,
            syncing,
        })
    }

    /// Show `element` at `index` of `count` layers (bottom first).
    pub(super) fn sync(&self, element: &Element, index: usize, count: usize, caps: &Caps) {
        let base = element.base();
        self.syncing.set(true);
        self.blend.set(&base.blend_mode);
        self.opacity.set(base.opacity, false);
        self.syncing.set(false);
        let locked = base.locked;
        self.front.set_sensitive(!locked && index + 1 < count);
        self.back.set_sensitive(!locked && index > 0);
        self.merge_down
            .set_sensitive(caps.merge_down_ids.contains(&base.id));
        self.merge_visible.set_sensitive(caps.can_merge_visible);
        self.flatten.set_sensitive(caps.can_flatten);
        self.delete.set_sensitive(!locked);
    }
}
