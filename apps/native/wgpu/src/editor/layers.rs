//! Shipping sidebar Layers section (`.screenshot-layers`, ScreenshotEditor.tsx
//! 6251-6692): the heading, 54 px rows with a grip, live thumbnail, name and
//! kind, eye/lock quick actions and the ⋯ layer settings popover. Rows drag
//! to reorder (one undo step); double-clicking an image row renames it inline.
//! It is always shown above Properties, whatever the rail tool.
use super::chrome::{accessible, dim, galley, icon};
use super::*;
use captures_app::editor_chrome as model;
use captures_app::editor_layers::{self as shared, menu as copy};
use eframe::egui::{
    Color32, FontFamily, FontId, Pos2, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use std::collections::HashMap;

/// `.screenshot-layer-select` min-height.
pub(super) const ROW_HEIGHT: f32 = 54.;
/// `li` vertical margin.
const ROW_MARGIN: f32 = 2.;
/// `.screenshot-layers-heading` min-height.
pub(super) const HEADING_HEIGHT: f32 = 48.;
/// `.screenshot-layer-menu-panel` width.
const MENU_WIDTH: f32 = 280.;
/// Pointer travel before a row press becomes a reorder drag.
const DRAG_THRESHOLD: f32 = 4.;

/// Shipping `.screenshot-sidebar` rows: `minmax(188px, 40%) minmax(0, 1fr)`.
pub(super) fn section_height(panel: f32) -> f32 {
    (panel * 0.4).max(188.).min(panel)
}

/// A row press that may become a reorder drag.
pub(super) struct RowDrag {
    id: String,
    origin: Pos2,
    active: bool,
    /// Drop target row and whether the layer lands before (above) it.
    target: Option<(String, bool)>,
}

/// An inline rename field (image layers only, like shipping).
pub(super) struct Rename {
    id: String,
    pub(super) value: String,
    focus: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) drag: Option<RowDrag>,
    pub(super) rename: Option<Rename>,
    /// The layer whose ⋯ settings popover is open.
    pub(super) menu: Option<String>,
    textures: HashMap<String, (Arc<RgbaImage>, egui::TextureHandle)>,
}

impl State {
    /// Escape, tool changes and document swaps end row gestures.
    pub(super) fn cancel(&mut self) {
        self.drag = None;
        self.rename = None;
        self.menu = None;
    }

    pub(super) fn dragging(&self) -> Option<&str> {
        self.drag
            .as_ref()
            .filter(|drag| drag.active)
            .map(|drag| drag.id.as_str())
    }
}

fn semibold(tokens: &Tokens, size: &str) -> FontId {
    FontId::new(tokens.number(size), FontFamily::Name("semibold".into()))
}

fn proportional(tokens: &Tokens, size: &str) -> FontId {
    FontId::proportional(tokens.number(size))
}

/// Upload changed thumbnails and drop textures for removed layers.
fn sync_textures(ctx: &egui::Context, view: &mut View) {
    let Some(presented) = &view.presented else {
        view.layers.textures.clear();
        return;
    };
    let textures = &mut view.layers.textures;
    textures.retain(|id, _| presented.thumbnails.contains_key(id));
    for (id, image) in &presented.thumbnails {
        if textures
            .get(id)
            .is_some_and(|(current, _)| Arc::ptr_eq(current, image))
        {
            continue;
        }
        let texture = ctx.load_texture(
            format!("layer-thumbnail-{id}"),
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        );
        textures.insert(id.clone(), (image.clone(), texture));
    }
}

/// The Layers section: heading, then the scrolling row list in `height`.
pub(super) fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    height: f32,
) {
    sync_textures(ui.ctx(), view);
    let top = ui.cursor().top();
    heading(ui, tokens, view);
    ui.add_space(tokens.number("s-3"));
    let Some(presented) = &view.presented else {
        return;
    };
    let document = presented.document.clone();
    let list_height = (height - (ui.cursor().top() - top) - tokens.number("s-5")).max(ROW_HEIGHT);
    let enabled = ui.is_enabled() && !view.pending;
    let mut rows: Vec<(String, egui::Rect)> = Vec::new();
    let mut menu_anchor = None;
    crate::primitives::scroll_area(
        ui,
        tokens,
        egui::ScrollArea::vertical()
            .id_salt("layer-list")
            .max_height(list_height)
            .min_scrolled_height(list_height)
            .auto_shrink([false, false]),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            for element in document.elements.iter().rev() {
                let id = element.base().id.clone();
                let rect = ui
                    .push_id(&id, |ui| {
                        row(ui, tokens, view, tx, element, enabled, &mut menu_anchor)
                    })
                    .inner;
                rows.push((id, rect));
            }
            let empty = ui.allocate_response(
                vec2(
                    ui.available_width(),
                    (list_height - ui.min_rect().height()).max(1.),
                ),
                Sense::click(),
            );
            empty.context_menu(|ui| layer_context_menu(ui, view, tx, None));
        },
    );
    finish_drag(ui, view, tx, &rows);
    if let Some((id, anchor)) = menu_anchor {
        settings_menu(ui, tokens, view, tx, &document, &id, anchor);
    }
}

/// Shipping `.screenshot-layers-heading`: title, count pill and Add image layer.
fn heading(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADING_HEIGHT), Sense::hover());
    ui.painter().hline(
        rect.expand2(vec2(8., 0.)).x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1., tokens.color("border-subtle")),
    );
    let title = galley(
        ui,
        model::layers::TITLE,
        semibold(tokens, "text-md"),
        tokens.color("text"),
    );
    let title_right = rect.left() + title.size().x;
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - title.size().y / 2.),
        title,
        tokens.color("text"),
    );
    let count = view
        .presented
        .as_ref()
        .map_or(0, |presented| presented.document.elements.len());
    let digits = galley(
        ui,
        &count.to_string(),
        FontId::monospace(tokens.number("text-2xs")),
        tokens.color("text-subtle"),
    );
    let pill = egui::Rect::from_min_size(
        pos2(title_right + tokens.number("s-3"), rect.center().y - 9.5),
        vec2((digits.size().x + 2. * tokens.number("s-2")).max(19.), 19.),
    );
    ui.painter().rect_filled(
        pill,
        tokens.number("r-pill"),
        tokens.color("surface-sunken"),
    );
    ui.painter().galley(
        pill.center() - digits.size() / 2.,
        digits,
        tokens.color("text-subtle"),
    );
    let add =
        egui::Rect::from_center_size(pos2(rect.right() - 15., rect.center().y), Vec2::splat(30.));
    let enabled = chrome::edit_enabled(view) && view.import_picker.is_none();
    let response = ui
        .interact(
            add,
            ui.scope_id().with("add-image-layer"),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        )
        .on_hover_text(model::layers::ADD);
    let hovered = enabled && response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(add, tokens.number("r-md"), tokens.color("surface-hover"));
    }
    let ink = tokens.color(if hovered { "text" } else { "text-muted" });
    icon(
        ui.painter(),
        "plus",
        add.center(),
        16.,
        1.8,
        dim(ink, enabled),
    );
    accessible(
        &response,
        egui::WidgetType::Button,
        false,
        model::layers::ADD,
    );
    if response.clicked() {
        view.choose_image(ui.ctx());
    }
}

/// Paint the shipping transparency checkerboard (10 px tiles) in `rect`.
fn checkerboard(painter: &egui::Painter, tokens: &Tokens, rect: egui::Rect) {
    painter.rect_filled(rect, 0., tokens.color("canvas-checker-b"));
    let tile = 5.;
    let columns = (rect.width() / tile).ceil() as usize;
    let rows = (rect.height() / tile).ceil() as usize;
    let dark = tokens.color("canvas-checker-a");
    for row in 0..rows {
        for column in (row % 2..columns).step_by(2) {
            let min = rect.min + vec2(column as f32 * tile, row as f32 * tile);
            let cell = egui::Rect::from_min_size(min, Vec2::splat(tile)).intersect(rect);
            painter.rect_filled(cell, 0., dark);
        }
    }
}

/// One `.screenshot-layer-list li`. Returns its rect for drop targeting.
fn row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    element: &Element,
    enabled: bool,
    menu_anchor: &mut Option<(String, egui::Rect)>,
) -> egui::Rect {
    let base = element.base();
    let id = base.id.clone();
    ui.add_space(ROW_MARGIN);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    ui.add_space(ROW_MARGIN);
    let selected = view.selected_layer.as_deref() == Some(id.as_str());
    let renaming = view
        .layers
        .rename
        .as_ref()
        .is_some_and(|rename| rename.id == id);
    let body = ui.interact(
        rect,
        ui.scope_id().with("row"),
        if enabled && !renaming {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let name = model::layer_name(element);
    let kind = model::layer_kind(element);
    let dragging = view.layers.dragging() == Some(id.as_str());
    let painter = ui.painter().clone();
    let painter = if dragging {
        let mut faded = painter;
        faded.set_opacity(0.42);
        faded
    } else {
        painter
    };
    let radius = tokens.number("r-lg");
    if selected {
        painter.rect(
            rect,
            radius,
            tokens.color("surface-selected"),
            Stroke::new(1., tokens.color("theme-accent").gamma_multiply(0.4)),
            StrokeKind::Inside,
        );
    } else if body.hovered() && view.layers.dragging().is_none() {
        painter.rect_filled(rect, radius, tokens.color("surface-hover"));
    }
    let faded = |color: Color32| {
        if base.visible {
            color
        } else {
            color.gamma_multiply(0.42)
        }
    };
    let pad = tokens.number("s-3");
    let gap = tokens.number("s-4");
    // Grip: a reorder cue, dimmed for locked layers.
    let grip = egui::Rect::from_min_size(
        pos2(rect.left() + pad, rect.top()),
        vec2(12., rect.height()),
    );
    let row_hovered = body.hovered() || body.dragged();
    let grip_ink = if base.locked {
        tokens.color("text-subtle").gamma_multiply(0.35)
    } else if row_hovered {
        tokens.color("text-muted")
    } else {
        tokens.color("text-subtle").gamma_multiply(0.5)
    };
    icon(&painter, "grip", grip.center(), 18., 1.6, grip_ink);
    // Live preview over the transparency checkerboard.
    let preview = egui::Rect::from_min_size(
        pos2(grip.right() + gap, rect.center().y - 16.),
        vec2(44., 32.),
    );
    let preview_painter = painter.with_clip_rect(preview.intersect(painter.clip_rect()));
    checkerboard(&preview_painter, tokens, preview);
    if let Some((image, texture)) = view.layers.textures.get(&id) {
        let size = vec2(image.width() as f32, image.height() as f32);
        let cover = matches!(element, Element::Image(_));
        let scale = if cover {
            (preview.width() / size.x).max(preview.height() / size.y)
        } else {
            (preview.width() / size.x).min(preview.height() / size.y)
        };
        let shown = egui::Rect::from_center_size(preview.center(), size * scale);
        preview_painter.image(
            texture.id(),
            shown,
            egui::Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            faded(Color32::WHITE),
        );
    } else {
        icon(
            &preview_painter,
            model::layer_icon(element),
            preview.center(),
            18.,
            1.7,
            faded(tokens.color("text-muted")),
        );
    }
    painter.rect_stroke(
        preview,
        tokens.number("r-sm"),
        Stroke::new(1., tokens.color("border")),
        StrokeKind::Inside,
    );
    // Quick actions: 25x28 buttons, 2 px apart, s-3 from the right edge.
    let action = vec2(25., 28.);
    let actions_left = rect.right() - pad - 3. * action.x - 2. * 2.;
    let copy_rect = egui::Rect::from_min_max(
        pos2(preview.right() + gap, rect.top()),
        pos2(actions_left - tokens.number("s-2"), rect.bottom()),
    );
    let ellipsized = |text: &str, font: FontId, color: Color32| {
        let mut job = egui::text::LayoutJob::single_section(
            text.to_owned(),
            egui::TextFormat::simple(font, color),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: copy_rect.width().max(0.),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        ui.painter().layout_job(job)
    };
    let kind_galley = ellipsized(
        kind,
        proportional(tokens, "text-xs"),
        faded(tokens.color("text-subtle")),
    );
    let name_top = rect.center().y - 1. - 16.;
    if renaming {
        rename_field(ui, tokens, view, tx, element, copy_rect, name_top);
    } else {
        let name_galley = ellipsized(
            &name,
            FontId::new(
                tokens.number("text-sm"),
                FontFamily::Name("semibold".into()),
            ),
            faded(tokens.color("text")),
        );
        painter.galley(
            pos2(copy_rect.left(), name_top + 1.),
            name_galley,
            faded(tokens.color("text")),
        );
    }
    painter.galley(
        pos2(copy_rect.left(), rect.center().y + 3.),
        kind_galley,
        faded(tokens.color("text-subtle")),
    );
    body.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            enabled,
            selected,
            format!("{name}, {kind}"),
        )
    });
    let body = if matches!(element, Element::Image(_)) {
        body.on_hover_text(model::layers::RENAME)
    } else if base.locked {
        body.on_hover_text(model::layers::LOCKED)
    } else {
        body.on_hover_text(model::layers::DRAG)
    };
    let menu_open = view.layers.menu.as_deref() == Some(id.as_str());
    let quick = |index: usize, on: bool, icon_name: &str, label: String, tooltip: &str| {
        let area = egui::Rect::from_min_size(
            pos2(
                actions_left + index as f32 * (action.x + 2.),
                rect.center().y - action.y / 2.,
            ),
            action,
        );
        let response = ui.interact(
            area,
            ui.scope_id().with(icon_name),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let hovered = enabled && response.hovered();
        let dark = ui.visuals().dark_mode;
        if on {
            painter.rect_filled(
                area,
                tokens.number("r-sm"),
                tokens.color("surface-selected"),
            );
        } else if hovered {
            painter.rect_filled(area, tokens.number("r-sm"), tokens.color("surface-active"));
        }
        // Shipping dims quick actions until the row is hovered or active.
        let ink = if on {
            tokens.color(if dark {
                "theme-accent-text"
            } else {
                "theme-accent-readable"
            })
        } else if hovered {
            tokens.color("text")
        } else {
            let color = tokens.color("text-subtle");
            if selected || row_hovered {
                color
            } else {
                color.gamma_multiply(0.5)
            }
        };
        icon(
            &painter,
            icon_name,
            area.center(),
            14.,
            1.8,
            dim(ink, enabled),
        );
        accessible(&response, egui::WidgetType::Button, on, &label);
        (response.clone().on_hover_text(tooltip).clicked(), area)
    };
    let (visibility, _) = quick(
        0,
        !base.visible,
        if base.visible { "eye" } else { "eye-off" },
        model::visibility_label(element),
        if base.visible {
            model::layers::HIDE
        } else {
            model::layers::SHOW
        },
    );
    let (lock, _) = quick(
        1,
        base.locked,
        if base.locked { "lock" } else { "unlock" },
        model::lock_label(element),
        if base.locked {
            model::layers::UNLOCK
        } else {
            model::layers::LOCK
        },
    );
    let (more, more_rect) = quick(
        2,
        menu_open,
        "more",
        copy::settings_label(&name),
        model::layers::MENU,
    );
    if menu_open {
        *menu_anchor = Some((id.clone(), more_rect));
    }
    if visibility {
        view.submit(
            tx,
            Request::Layer {
                id: id.clone(),
                edit: LayerEdit::Visibility {
                    visible: !base.visible,
                },
            },
        );
    } else if lock {
        // Shipping selects the row it locks or unlocks.
        view.select_layer(Some(id.clone()));
        view.submit(
            tx,
            Request::Layer {
                id: id.clone(),
                edit: LayerEdit::Lock {
                    locked: !base.locked,
                },
            },
        );
    } else if more {
        view.activate_tool(Section::Layers, None);
        view.select_layer(Some(id.clone()));
        view.layers.rename = None;
        view.layers.menu = if menu_open { None } else { Some(id.clone()) };
        if !menu_open {
            view.layer_opacity = base.opacity;
        }
    } else if body.double_clicked() && matches!(element, Element::Image(_)) {
        view.layers.menu = None;
        view.activate_tool(Section::Layers, None);
        view.select_layer(Some(id.clone()));
        view.layers.rename = Some(Rename {
            id: id.clone(),
            value: layer_label(element).to_owned(),
            focus: true,
        });
    } else if body.clicked() {
        view.activate_tool(Section::Layers, None);
        view.select_layer(Some(id.clone()));
    }
    if body.drag_started() && !base.locked {
        view.layers.menu = None;
        view.layers.drag = Some(RowDrag {
            id: id.clone(),
            origin: body.interact_pointer_pos().unwrap_or(rect.center()),
            active: false,
            target: None,
        });
    }
    body.context_menu(|ui| layer_context_menu(ui, view, tx, Some(id.clone())));
    // The drop indicator: a 2 px accent rule 4 px outside the target row.
    if let Some((target, before)) = view
        .layers
        .drag
        .as_ref()
        .filter(|drag| drag.active)
        .and_then(|drag| drag.target.clone())
        && target == id
    {
        let y = if before {
            rect.top() - 2.
        } else {
            rect.bottom() + 2.
        };
        let rule = egui::Rect::from_min_max(
            pos2(rect.left() + 3., y - 1.),
            pos2(rect.right() - 3., y + 1.),
        );
        ui.painter()
            .rect_filled(rule, tokens.number("r-pill"), tokens.color("theme-accent"));
    }
    rect
}

/// Shipping `.screenshot-layer-copy input`: Enter or blur renames, Escape cancels.
fn rename_field(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    element: &Element,
    copy_rect: egui::Rect,
    top: f32,
) {
    let Some(rename) = &mut view.layers.rename else {
        return;
    };
    let field = egui::Rect::from_min_size(
        pos2(copy_rect.left(), top - 3.),
        vec2(copy_rect.width(), 25.),
    );
    ui.painter().rect(
        field,
        tokens.number("r-sm"),
        tokens.color("surface-field"),
        Stroke::new(1., tokens.color("theme-accent")),
        StrokeKind::Inside,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(field.shrink2(vec2(tokens.number("s-3"), 1.)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let response = child.add(
        egui::TextEdit::singleline(&mut rename.value)
            .frame(egui::Frame::NONE)
            .font(FontId::new(
                tokens.number("text-sm"),
                FontFamily::Name("semibold".into()),
            ))
            .desired_width(field.width() - 2. * tokens.number("s-3")),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, copy::RENAME_LABEL)
    });
    if rename.focus {
        response.request_focus();
        rename.focus = false;
        return;
    }
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if response.lost_focus() || (!response.has_focus() && !escape) {
        let rename = view.layers.rename.take().expect("rename is active");
        if !escape && rename.value.trim() != layer_label(element) && !rename.value.trim().is_empty()
        {
            view.submit(
                tx,
                Request::Layer {
                    id: rename.id,
                    edit: LayerEdit::Rename { name: rename.value },
                },
            );
        }
    }
}

/// Track an active row drag and drop it as one Reorder on release.
fn finish_drag(ui: &egui::Ui, view: &mut View, tx: &Sender<Job>, rows: &[(String, egui::Rect)]) {
    let Some(drag) = &mut view.layers.drag else {
        return;
    };
    let (pointer, down, released) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
        )
    });
    if let Some(pointer) = pointer {
        if !drag.active && pointer.distance(drag.origin) >= DRAG_THRESHOLD {
            drag.active = true;
        }
        if drag.active {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let target = rows
                .iter()
                .filter(|(id, _)| *id != drag.id)
                .find(|(_, rect)| {
                    rect.expand2(vec2(0., ROW_MARGIN))
                        .y_range()
                        .contains(pointer.y)
                })
                .map(|(id, rect)| (id.clone(), pointer.y < rect.center().y));
            drag.target = target;
            // Auto-scroll is not needed for the short native list; keep repainting.
            ui.ctx().request_repaint();
        }
    }
    if released || !down {
        let drag = view.layers.drag.take().expect("drag is active");
        if drag.active
            && let Some((target_id, before)) = drag.target
        {
            view.submit(
                tx,
                Request::Layer {
                    id: drag.id,
                    edit: LayerEdit::Reorder {
                        target_id,
                        placement: if before {
                            LayerPlacement::Before
                        } else {
                            LayerPlacement::After
                        },
                    },
                },
            );
        }
    }
}

/// A popover section title (`.screenshot-layer-menu-section-title`).
fn section_title(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(semibold(tokens, "text-2xs"))
            .color(tokens.color("text-subtle")),
    );
}

fn field_label(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(semibold(tokens, "text-sm"))
            .color(tokens.color("text-muted")),
    );
}

/// How a `.screenshot-layer-menu-action` row is drawn: a plain item, the
/// bordered footer control, or the footer's danger variant.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Item,
    Footer,
    Danger,
}

/// `.screenshot-layer-menu-action`: an 18 px icon and a label.
fn menu_action(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    icon_name: &str,
    label: &str,
    tooltip: &str,
    enabled: bool,
    tone: Tone,
) -> bool {
    let footer = tone != Tone::Item;
    let danger = tone == Tone::Danger;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), 34.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let radius = tokens.number("r-md");
    let (fill, border) = match (footer, danger) {
        (true, true) => (
            tokens.color("danger-surface"),
            tokens.color("danger-border"),
        ),
        (true, false) => (
            tokens.color(if hovered { "control-hover" } else { "control" }),
            tokens.color(if hovered {
                "border-strong"
            } else {
                "border-subtle"
            }),
        ),
        (false, true) if hovered => (tokens.color("danger-surface"), Color32::TRANSPARENT),
        (false, _) if hovered => (tokens.color("surface-hover"), Color32::TRANSPARENT),
        _ => (Color32::TRANSPARENT, Color32::TRANSPARENT),
    };
    ui.painter().rect(
        rect,
        radius,
        fill,
        Stroke::new(1., border),
        StrokeKind::Inside,
    );
    let alpha = if enabled { 1. } else { 0.4 };
    let text = tokens
        .color(if danger { "danger-text" } else { "text" })
        .gamma_multiply(alpha);
    let glyph = if danger {
        text
    } else {
        tokens.color("text-muted").gamma_multiply(alpha)
    };
    let pad = tokens.number("s-4");
    icon(
        ui.painter(),
        icon_name,
        pos2(rect.left() + pad + 9., rect.center().y),
        15.,
        1.8,
        glyph,
    );
    let label_galley = galley(ui, label, semibold(tokens, "text-md"), text);
    ui.painter().galley(
        pos2(
            rect.left() + pad + 18. + pad,
            rect.center().y - label_galley.size().y / 2.,
        ),
        label_galley,
        text,
    );
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, rect, radius);
    }
    accessible(&response, egui::WidgetType::Button, false, label);
    response.on_hover_text(tooltip).clicked()
}

/// `.screenshot-layer-menu-tile`: a bordered 36 px transform tile.
fn menu_tile(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    icon_name: &str,
    label: &str,
    tooltip: &str,
    width: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 36.), Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        tokens.number("r-md"),
        tokens.color(if hovered { "control-hover" } else { "control" }),
        Stroke::new(
            1.,
            tokens.color(if hovered {
                "border-strong"
            } else {
                "border-subtle"
            }),
        ),
        StrokeKind::Inside,
    );
    let ink = tokens.color(if hovered { "text" } else { "text-muted" });
    let pad = tokens.number("s-4");
    icon(
        ui.painter(),
        icon_name,
        pos2(rect.left() + pad + 8., rect.center().y),
        15.,
        1.8,
        ink,
    );
    let mut job = egui::text::LayoutJob::single_section(
        label.to_owned(),
        egui::TextFormat::simple(semibold(tokens, "text-sm"), ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 2. * pad - 16. - tokens.number("s-3")).max(0.),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let text = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            rect.left() + pad + 16. + tokens.number("s-3"),
            rect.center().y - text.size().y / 2.,
        ),
        text,
        ink,
    );
    accessible(&response, egui::WidgetType::Button, false, label);
    response.on_hover_text(tooltip).clicked()
}

/// Shipping `.screenshot-layer-menu-panel`: Appearance (blend mode and
/// opacity), Transform tiles for images, Arrange, Combine, then Duplicate and
/// Delete. It sits beside the sidebar at the ⋯ button, over the canvas.
fn settings_menu(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    document: &Document,
    id: &str,
    anchor: egui::Rect,
) {
    let Some(element) = document
        .elements
        .iter()
        .find(|element| element.base().id == id)
    else {
        view.layers.menu = None;
        return;
    };
    let base = element.base().clone();
    let name = model::layer_name(element);
    let content = ui.ctx().content_rect();
    let sidebar_left = ui.max_rect().left() - 8.;
    let left = (sidebar_left - MENU_WIDTH - 8.).max(content.left() + 8.);
    // Keep the whole popover on screen: clamp with last frame's measured
    // height, and scroll the body once it is taller than the window.
    let area_id = ui.scope_id().with("layer-settings");
    let max_height = content.height() - 16.;
    let height = ui
        .ctx()
        .memory(|memory| memory.area_rect(area_id))
        .map_or(0., |rect| rect.height().min(max_height));
    let top = anchor
        .top()
        .min(content.bottom() - 8. - height)
        .max(content.top() + 8.);
    let enabled = !view.pending && view.inline.is_none();
    let mut select_open = false;
    let mut close = false;
    let dark = ui.visuals().dark_mode;
    let area =
        egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(left, top))
            .constrain(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(tokens.color("surface-overlay"))
                    .stroke(Stroke::new(1., tokens.color("border")))
                    .corner_radius(tokens.number("r-lg"))
                    .shadow(crate::primitives::shadow_lg(dark))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(max_height - 2.)
                            .min_scrolled_height(max_height - 2.)
                            .show(ui, |ui| {
                                ui.set_width(MENU_WIDTH);
                                ui.spacing_mut().item_spacing = vec2(0., tokens.number("s-4"));
                                let pad = tokens.number("s-5");
                                let inner = MENU_WIDTH - 2. * pad;
                                let section =
                                    |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                                        egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
                                            ui.set_width(inner);
                                            add(ui);
                                        });
                                        let y = ui.cursor().top();
                                        ui.painter().hline(
                                            ui.max_rect().x_range(),
                                            y - 0.5,
                                            Stroke::new(1., tokens.color("border-subtle")),
                                        );
                                    };
                                ui.add_enabled_ui(enabled, |ui| {
                                    section(ui, &mut |ui| {
                                        section_title(ui, tokens, copy::APPEARANCE);
                                        field_label(ui, tokens, copy::BLEND_MODE);
                                        let options: Vec<_> = shared::BLEND_MODES
                                            .iter()
                                            .map(|(value, label)| {
                                                crate::primitives::SelectOption::new(
                                                    value.to_string(),
                                                    label,
                                                )
                                            })
                                            .collect();
                                        let output = crate::primitives::Select::new(
                                            "layer-blend-mode",
                                            copy::BLEND_MODE,
                                            inner,
                                        )
                                        .show(ui, tokens, &options, &base.blend_mode);
                                        select_open |= output.open;
                                        if let Some(blend_mode) = output.chosen {
                                            view.submit(
                                                tx,
                                                Request::Layer {
                                                    id: base.id.clone(),
                                                    edit: LayerEdit::BlendMode { blend_mode },
                                                },
                                            );
                                        }
                                        field_label(ui, tokens, copy::OPACITY);
                                        let mut opacity = view.layer_opacity;
                                        let slider = crate::primitives::RangeSlider::new(
                                            "layer-opacity",
                                            copy::OPACITY_LABEL,
                                            inner,
                                            0. ..=100.,
                                            format!("{}%", opacity.round()),
                                        )
                                        .show(ui, tokens, &mut opacity);
                                        if slider.changed() {
                                            view.layer_opacity = opacity;
                                            view.live_edit(
                                                tx,
                                                format!("opacity:{}", base.id),
                                                Request::Layer {
                                                    id: base.id.clone(),
                                                    edit: LayerEdit::Opacity { opacity },
                                                },
                                            );
                                        }
                                    });
                                    if let Element::Image(_) = element {
                                        section(ui, &mut |ui| {
                                            section_title(ui, tokens, copy::TRANSFORM);
                                            let gap = tokens.number("s-3");
                                            let width = (inner - gap) / 2.;
                                            let tiles = [
                                                (
                                                    "rotate-counterclockwise",
                                                    copy::ROTATE_LEFT,
                                                    copy::ROTATE_LEFT_TIP,
                                                    ImageTransform::RotateCounterclockwise,
                                                ),
                                                (
                                                    "rotate-clockwise",
                                                    copy::ROTATE_RIGHT,
                                                    copy::ROTATE_RIGHT_TIP,
                                                    ImageTransform::RotateClockwise,
                                                ),
                                                (
                                                    "flip-horizontal",
                                                    copy::FLIP_HORIZONTAL,
                                                    copy::FLIP_HORIZONTAL_TIP,
                                                    ImageTransform::FlipHorizontal,
                                                ),
                                                (
                                                    "flip-vertical",
                                                    copy::FLIP_VERTICAL,
                                                    copy::FLIP_VERTICAL_TIP,
                                                    ImageTransform::FlipVertical,
                                                ),
                                            ];
                                            for pair in tiles.chunks(2) {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = gap;
                                                    for (glyph, label, tip, transform) in pair {
                                                        if menu_tile(
                                                            ui, tokens, glyph, label, tip, width,
                                                        ) {
                                                            view.submit(
                                                                tx,
                                                                Request::Layer {
                                                                    id: base.id.clone(),
                                                                    edit:
                                                                        LayerEdit::ImageTransform {
                                                                            transform: *transform,
                                                                        },
                                                                },
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                    }
                                    section(ui, &mut |ui| {
                                        section_title(ui, tokens, copy::ARRANGE);
                                        ui.spacing_mut().item_spacing.y = 2.;
                                        for (glyph, label, tip, front) in [
                                            (
                                                "bring-front",
                                                copy::BRING_FRONT,
                                                copy::BRING_FRONT_TIP,
                                                true,
                                            ),
                                            (
                                                "send-back",
                                                copy::SEND_BACK,
                                                copy::SEND_BACK_TIP,
                                                false,
                                            ),
                                        ] {
                                            if menu_action(
                                                ui,
                                                tokens,
                                                glyph,
                                                label,
                                                tip,
                                                shared::can_arrange(document, &base.id, front),
                                                Tone::Item,
                                            ) {
                                                view.submit(
                                                    tx,
                                                    Request::Layer {
                                                        id: base.id.clone(),
                                                        edit: LayerEdit::Arrange { front },
                                                    },
                                                );
                                            }
                                        }
                                    });
                                    section(ui, &mut |ui| {
                                        section_title(ui, tokens, copy::COMBINE);
                                        ui.spacing_mut().item_spacing.y = 2.;
                                        for (glyph, label, tip, action) in [
                                            (
                                                "merge-down",
                                                copy::MERGE_DOWN,
                                                copy::MERGE_DOWN_TIP,
                                                LayerAction::MergeDown,
                                            ),
                                            (
                                                "merge-visible",
                                                copy::MERGE_VISIBLE,
                                                copy::MERGE_VISIBLE_TIP,
                                                LayerAction::MergeVisible,
                                            ),
                                            (
                                                "flatten",
                                                copy::FLATTEN,
                                                copy::FLATTEN_TIP,
                                                LayerAction::Flatten,
                                            ),
                                        ] {
                                            let allowed =
                                                layer_action_enabled(view, action, Some(&base.id));
                                            if menu_action(
                                                ui,
                                                tokens,
                                                glyph,
                                                label,
                                                tip,
                                                allowed,
                                                Tone::Item,
                                            ) {
                                                dispatch_layer_action(
                                                    view,
                                                    tx,
                                                    action,
                                                    Some(base.id.clone()),
                                                );
                                                close = true;
                                            }
                                        }
                                    });
                                    egui::Frame::NONE
                                        .fill(tokens.color("surface-sunken"))
                                        .inner_margin(tokens.number("s-4"))
                                        .corner_radius(egui::CornerRadius {
                                            nw: 0,
                                            ne: 0,
                                            sw: tokens.number("r-lg") as u8,
                                            se: tokens.number("r-lg") as u8,
                                        })
                                        .show(ui, |ui| {
                                            ui.set_width(MENU_WIDTH - 2. * tokens.number("s-4"));
                                            ui.spacing_mut().item_spacing.y = tokens.number("s-2");
                                            if menu_action(
                                                ui,
                                                tokens,
                                                "duplicate",
                                                copy::DUPLICATE,
                                                copy::DUPLICATE_TIP,
                                                true,
                                                Tone::Footer,
                                            ) {
                                                dispatch_layer_action(
                                                    view,
                                                    tx,
                                                    LayerAction::Duplicate,
                                                    Some(base.id.clone()),
                                                );
                                                close = true;
                                            }
                                            let deletable = layer_action_enabled(
                                                view,
                                                LayerAction::Delete,
                                                Some(&base.id),
                                            );
                                            if menu_action(
                                                ui,
                                                tokens,
                                                "trash",
                                                copy::DELETE,
                                                copy::DELETE_TIP,
                                                deletable,
                                                Tone::Danger,
                                            ) {
                                                dispatch_layer_action(
                                                    view,
                                                    tx,
                                                    LayerAction::Delete,
                                                    Some(base.id.clone()),
                                                );
                                                close = true;
                                            }
                                        });
                                });
                            });
                    });
            });
    area.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, copy::settings_label(&name))
    });
    let clicked_outside = ui.input(|input| {
        input.pointer.any_pressed()
            && input.pointer.interact_pos().is_some_and(|pointer| {
                !area.response.rect.contains(pointer) && !anchor.contains(pointer)
            })
    });
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if close || (clicked_outside && !select_open) || (escape && !select_open) {
        view.layers.menu = None;
    }
}
