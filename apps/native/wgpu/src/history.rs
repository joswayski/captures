//! Capture History presentation, styled like the shipping `CaptureHistory` and
//! `HistoryCard` (`App.tsx`, `styles/windows.css`). Copy, card details, actions
//! and grid metrics come from `captures_app::history_view`, shared with AppKit.
//! This module only paints and reports input; `live` owns state and file work.
use captures_app::history_view::{self as shared, Card, CardAction};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2, text::LayoutJob,
};

use crate::tokens::Tokens;

pub enum Thumbnail {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

pub struct Item<'a> {
    pub id: &'a str,
    pub card: &'a Card,
    pub thumbnail: Option<&'a Thumbnail>,
    pub selected: bool,
    pub confirming_delete: bool,
    pub busy: Option<CardAction>,
    /// An action showing its shipping success label ("✓ Restored").
    pub done: Option<CardAction>,
    /// Shipping `.history-card-error` under the actions (a failed Restore).
    pub error: Option<&'a str>,
}

/// Card input, by index into the rendered item slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Select(usize),
    /// The thumbnail's open action (shipping: open in the editor).
    Open(usize),
    /// A thresholded primary-button gesture on the thumbnail.
    DragFile(usize),
    Action(usize, CardAction),
    Delete(usize),
    /// A visible card has no thumbnail request yet.
    NeedsThumbnail(usize),
}

/// Header state for the shipping two-step Delete all control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeleteAll {
    pub visible: bool,
    pub confirming: bool,
    pub busy: bool,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderEvent {
    DeleteAll,
    Cancel,
}

/// `.history-header`: eyebrow, title and lede, with Delete all on the right.
pub fn header(ui: &mut egui::Ui, t: &Tokens, delete_all: DeleteAll) -> Option<HeaderEvent> {
    let compact = captures_app::app_windows::compact(ui.ctx().content_rect().width());
    header_layout(ui, t, delete_all, compact).0
}

/// `.history-header-actions`: Cancel (while armed) before Delete all.
fn header_actions(ui: &mut egui::Ui, t: &Tokens, delete_all: DeleteAll) -> Option<HeaderEvent> {
    let copy = shared::copy();
    let mut event = None;
    let label = if delete_all.busy {
        copy.delete_all_busy
    } else if delete_all.confirming {
        copy.delete_all_confirm
    } else {
        copy.delete_all
    };
    let reversed = ui.layout().prefer_right_to_left();
    if delete_all.confirming && !reversed && cancel_button(ui, t, delete_all) {
        event = Some(HeaderEvent::Cancel);
    }
    let style = if delete_all.confirming {
        ButtonStyle::Confirm
    } else {
        ButtonStyle::Danger
    };
    let width =
        text_width(ui, label, t.number("text-sm")) + 15. + t.number("s-3") + 2. * t.number("s-5");
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, t.number("h-md")), Sense::hover());
    let enabled = delete_all.enabled && !delete_all.busy;
    let response = button(
        ui,
        t,
        rect,
        Id::unique("history-delete-all"),
        label,
        Some(Glyph::Trash),
        style,
        enabled,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            if delete_all.confirming {
                copy.delete_all_confirm_label
            } else {
                copy.delete_all_label
            },
        )
    });
    if response.clicked() {
        event = Some(HeaderEvent::DeleteAll);
    }
    if delete_all.confirming && reversed && cancel_button(ui, t, delete_all) {
        event = Some(HeaderEvent::Cancel);
    }
    event
}

fn cancel_button(ui: &mut egui::Ui, t: &Tokens, delete_all: DeleteAll) -> bool {
    let copy = shared::copy();
    let width = text_width(ui, copy.cancel, t.number("text-sm")) + 2. * t.number("s-5");
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, t.number("h-md")), Sense::hover());
    let cancel = button(
        ui,
        t,
        rect,
        Id::unique("history-delete-all-cancel"),
        copy.cancel,
        None,
        ButtonStyle::Ghost,
        !delete_all.busy,
    );
    cancel.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            !delete_all.busy,
            copy.cancel_label,
        )
    });
    cancel.clicked()
}

/// The header, plus the heading column's and the actions' rects. A compact
/// window stacks the actions under the heading (`flex-direction: column`).
fn header_layout(
    ui: &mut egui::Ui,
    t: &Tokens,
    delete_all: DeleteAll,
    compact: bool,
) -> (Option<HeaderEvent>, Rect, Rect) {
    let copy = shared::copy();
    let heading_ui = |ui: &mut egui::Ui, max_width: f32| {
        ui.vertical(|ui| {
            ui.set_max_width(max_width);
            ui.spacing_mut().item_spacing.y = t.number("s-3");
            ui.label(
                RichText::new(copy.eyebrow.to_uppercase())
                    .size(t.number("text-2xs"))
                    .color(t.color("text-subtle"))
                    .extra_letter_spacing(0.6)
                    .strong(),
            );
            ui.label(
                RichText::new(copy.title)
                    .size(t.number("text-3xl"))
                    .color(t.color("text"))
                    .strong(),
            );
            ui.label(
                RichText::new(copy.lede)
                    .size(t.number("text-md"))
                    .color(t.color("text-subtle")),
            );
        })
        .response
        .rect
    };
    let mut event = None;
    let mut actions = Rect::NOTHING;
    if compact {
        let heading = ui
            .vertical(|ui| {
                let heading = heading_ui(ui, ui.available_width());
                if delete_all.visible {
                    ui.add_space(t.number("s-5"));
                    let row = ui.horizontal(|ui| header_actions(ui, t, delete_all));
                    event = row.inner;
                    actions = row.response.rect;
                }
                heading
            })
            .inner;
        return (event, heading, actions);
    }
    // `.history-header` is a flex row: the heading column wraps inside the
    // space the actions leave, so the lede never runs under Delete all.
    let reserved = if delete_all.visible {
        let label = if delete_all.busy {
            copy.delete_all_busy
        } else if delete_all.confirming {
            copy.delete_all_confirm
        } else {
            copy.delete_all
        };
        let delete = text_width(ui, label, t.number("text-sm"))
            + 15.
            + t.number("s-3")
            + 2. * t.number("s-5");
        let cancel = if delete_all.confirming {
            ui.spacing().item_spacing.x
                + text_width(ui, copy.cancel, t.number("text-sm"))
                + 2. * t.number("s-5")
        } else {
            0.
        };
        delete + cancel + t.number("s-6")
    } else {
        0.
    };
    let heading_width = (ui.available_width() - reserved).max(0.);
    let mut heading = Rect::NOTHING;
    ui.horizontal(|ui| {
        heading = heading_ui(ui, heading_width);
        if delete_all.visible {
            let row = ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                let event = header_actions(ui, t, delete_all);
                (event, ui.min_rect())
            });
            (event, actions) = row.inner;
        }
    });
    (event, heading, actions)
}

/// `.history-filters` pills. `counts` pairs each label with its count.
pub fn filters(
    ui: &mut egui::Ui,
    t: &Tokens,
    options: &[(&str, usize, bool, bool)],
) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = t.number("s-2");
        for (index, (label, count, active, enabled)) in options.iter().copied().enumerate() {
            let count_text = count.to_string();
            let size = t.number("text-sm");
            let width = text_width(ui, label, size)
                + t.number("s-3")
                + text_width(ui, &count_text, size)
                + 2. * t.number("s-4");
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(width, t.number("h-sm")), Sense::hover());
            let sense = if enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let response = ui.interact(rect, Id::unique(("history-filter", index)), sense);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    enabled,
                    active,
                    format!("{label}, {count} captures"),
                )
            });
            let painter = ui.painter();
            let alpha = if enabled { 1. } else { 0.4 };
            let radius = CornerRadius::same(rect.height().min(255.) as u8 / 2);
            if active {
                painter.rect_filled(rect, radius, t.color("surface-raised"));
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1., t.color("border")),
                    StrokeKind::Inside,
                );
            } else if enabled && response.hovered() {
                painter.rect_filled(rect, radius, t.color("surface-hover"));
            }
            if response.has_focus() {
                crate::primitives::focus_indicated(ui.ctx());
                painter.rect_stroke(
                    rect.expand(1.),
                    radius,
                    Stroke::new(2., t.color("theme-accent")),
                    StrokeKind::Outside,
                );
            }
            let text = if active || (enabled && response.hovered()) {
                t.color("text")
            } else {
                t.color("text-subtle")
            };
            let left = rect.left() + t.number("s-4");
            let label_rect = painter.text(
                Pos2::new(left, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(size),
                text.gamma_multiply(alpha),
            );
            painter.text(
                Pos2::new(label_rect.right() + t.number("s-3"), rect.center().y),
                Align2::LEFT_CENTER,
                count_text,
                FontId::proportional(size),
                t.color("text-faint").gamma_multiply(alpha),
            );
            if response.clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
}

/// `.history-empty`: icon tile, title and optional body, centered.
pub fn empty(ui: &mut egui::Ui, t: &Tokens, title: &str, body: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.add_space(t.number("s-12"));
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(52.), Sense::hover());
        let painter = ui.painter();
        let radius = t.number("r-xl") as u8;
        painter.rect_filled(rect, radius, t.color("surface-raised"));
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1., t.color("border")),
            StrokeKind::Inside,
        );
        glyph(
            painter,
            Glyph::History,
            rect.shrink(13.),
            Stroke::new(1.6, t.color("text-subtle")),
        );
        ui.add_space(t.number("s-5"));
        ui.label(
            RichText::new(title)
                .size(t.number("text-lg"))
                .color(t.color("text"))
                .strong(),
        );
        if let Some(body) = body {
            ui.label(
                RichText::new(body)
                    .size(t.number("text-md"))
                    .color(t.color("text-subtle")),
            );
        }
    });
}

/// `.history-error` banner.
pub fn error(ui: &mut egui::Ui, t: &Tokens, message: &str) {
    egui::Frame::new()
        .fill(t.color("danger-surface"))
        .corner_radius(t.number("r-md") as u8)
        .inner_margin(egui::Margin::symmetric(
            t.number("s-5") as i8,
            t.number("s-4") as i8,
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(message)
                    .size(t.number("text-sm"))
                    .color(t.color("danger-text")),
            );
        });
}

pub struct GridOutput {
    pub events: Vec<Event>,
    /// Item indices laid out this frame (visible rows).
    pub visible: std::ops::Range<usize>,
    /// Content width the shared grid was computed for.
    pub width: f64,
    /// The window uses the shipping one-column compact grid.
    pub compact: bool,
}

/// `.history-grid`: virtualized auto-fill cards. `scroll_to` reveals one card.
pub fn grid(
    ui: &mut egui::Ui,
    t: &Tokens,
    items: &[Item<'_>],
    enabled: bool,
    scroll_to: Option<usize>,
) -> GridOutput {
    let mut events = Vec::new();
    let mut visible = 0..0;
    let mut width = 0.;
    let compact = captures_app::app_windows::compact(ui.ctx().content_rect().width());
    crate::primitives::scroll_viewport(
        ui,
        t,
        egui::ScrollArea::vertical()
            .id_salt("history-grid")
            .auto_shrink([false, false]),
        |ui, viewport| {
            width = f64::from(ui.available_width());
            let layout = shared::grid_in_window(width, compact);
            // A card error grows its row, as shipping's CSS grid rows fit the
            // tallest card.
            let mut extras = shared::RowExtras::default();
            for (index, item) in items.iter().enumerate() {
                if let Some(error) = item.error {
                    let text = error_galley(ui, t, error, layout.card_width as f32);
                    extras.grow(
                        index / layout.columns,
                        shared::card_error_extra(f64::from(text.size().y)),
                    );
                }
            }
            ui.set_height(layout.content_height_with(items.len(), &extras) as f32);
            let origin = ui.max_rect().min;
            let rect_for = |index: usize| {
                let (x, y) = layout.origin_with(index, &extras);
                Rect::from_min_size(
                    origin + Vec2::new(x as f32, y as f32),
                    Vec2::new(
                        layout.card_width as f32,
                        layout.row_card_height(index / layout.columns, &extras) as f32,
                    ),
                )
            };
            let rows = layout.visible_rows_with(
                items.len(),
                f64::from(viewport.min.y),
                f64::from(viewport.height()),
                &extras,
            );
            visible = (rows.start * layout.columns).min(items.len())
                ..(rows.end * layout.columns).min(items.len());
            for row in rows {
                for column in 0..layout.columns {
                    let index = row * layout.columns + column;
                    let Some(item) = items.get(index) else { break };
                    let rect = rect_for(index);
                    // Shipping `.history-card:hover`: a 2 px lift and a stronger
                    // border over `--dur-3` `--ease-standard`.
                    let hover =
                        hover_progress(ui, t, item.id, enabled && ui.rect_contains_pointer(rect));
                    let lift = captures_app::motion::Pose {
                        translate_y: -2. * f64::from(hover),
                        ..captures_app::motion::Pose::REST
                    };
                    crate::motion::with_pose(ui, lift, rect, |ui| {
                        card(ui, t, rect, item, index, enabled, hover, &mut events);
                    });
                }
            }
            if let Some(index) = scroll_to.filter(|index| *index < items.len()) {
                ui.scroll_to_rect(rect_for(index), None);
            }
        },
    );
    GridOutput {
        events,
        visible,
        width,
        compact,
    }
}

/// The wrapped `.history-card-error` text for a card `card_width` wide.
fn error_galley(
    ui: &egui::Ui,
    t: &Tokens,
    error: &str,
    card_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let width = card_width - 2. * (t.number("s-5") + shared::CARD_ERROR_PADDING_X as f32);
    let size = t.number("text-xs");
    let mut job = LayoutJob::simple(
        error.to_owned(),
        FontId::proportional(size),
        t.color("danger-text"),
        width.max(1.),
    );
    job.first_row_min_height = size * 1.35;
    job.sections[0].format.line_height = Some(size * 1.35);
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// Eased 0...1 hover progress for one card. egui animates linearly and stops
/// requesting frames once settled; the shipping easing is applied on top.
fn hover_progress(ui: &egui::Ui, t: &Tokens, item: &str, hovered: bool) -> f32 {
    let tween = t.transition(captures_app::motion::Transition::HistoryCardHover);
    let seconds = if crate::motion::reduced(ui.ctx()) {
        0.
    } else {
        (tween.duration_ms / 1000.) as f32
    };
    let linear =
        ui.ctx()
            .animate_bool_with_time(Id::unique(("history-card-hover", item)), hovered, seconds);
    tween.easing.ease(f64::from(linear)) as f32
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    t: &Tokens,
    rect: Rect,
    item: &Item<'_>,
    index: usize,
    enabled: bool,
    hover: f32,
    events: &mut Vec<Event>,
) {
    let id = Id::unique(("history-card", item.id));
    let card = item.card;
    let idle = enabled && item.busy.is_none();
    let gap = t.number("s-3");
    let height = t.number("h-md");
    let trash_rect = Rect::from_min_size(
        Pos2::new(rect.right() - gap - height, rect.top() + gap),
        Vec2::splat(height),
    );
    // Selection stays available while actions are busy, like keyboard arrows.
    let body = ui.interact(rect, id, Sense::click());
    body.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Other,
            enabled,
            item.selected,
            format!("{} · {} · {}", card.kind_label, card.date, card.details),
        )
    });
    let radius = t.number("r-xl");
    let r = radius as u8;
    // `box-shadow: var(--shadow-sm)`, easing to `var(--shadow-md)` on hover
    // with the lift. The two token shadows cross-fade, so their cached masks
    // serve every frame of the transition.
    crate::effects::paint_box_shadows(
        ui.painter(),
        rect,
        radius,
        t.shadow("shadow-sm"),
        1. - hover,
    );
    crate::effects::paint_box_shadows(ui.painter(), rect, radius, t.shadow("shadow-md"), hover);
    let painter = ui.painter_at(rect.expand(3.));
    painter.rect_filled(rect, r, t.color("surface-raised"));

    let image_rect = Rect::from_min_size(
        rect.min,
        Vec2::new(rect.width(), shared::CARD_IMAGE_HEIGHT as f32),
    );
    let top = CornerRadius {
        nw: r,
        ne: r,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(image_rect, top, t.color("surface-sunken"));
    let can_open = idle && card.open_label.is_some();
    let can_drag = can_open && !card.missing;
    let open = ui.interact(
        image_rect,
        id.with("open"),
        if can_open {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    open.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            can_open,
            card.open_label.unwrap_or(card.image_label),
        )
    });
    if can_open && open.hovered() {
        painter.rect_filled(
            image_rect,
            top,
            t.color("theme-accent").gamma_multiply(0.12),
        );
    }
    match item.thumbnail {
        Some(Thumbnail::Ready(texture)) => {
            let fitted = contain(texture.size_vec2(), image_rect);
            let tint = if card.missing {
                Color32::from_gray(160).gamma_multiply(0.3)
            } else {
                Color32::WHITE
            };
            let corner = |touches: bool| if touches { r } else { 0 };
            let top_edge = (fitted.top() - image_rect.top()).abs() < 0.5;
            egui::Image::from_texture(texture)
                .tint(tint)
                .corner_radius(CornerRadius {
                    nw: corner(top_edge && (fitted.left() - image_rect.left()).abs() < 0.5),
                    ne: corner(top_edge && (fitted.right() - image_rect.right()).abs() < 0.5),
                    sw: 0,
                    se: 0,
                })
                .paint_at(ui, fitted);
        }
        Some(Thumbnail::Loading) => {}
        Some(Thumbnail::Failed) => {
            painter.text(
                image_rect.center(),
                Align2::CENTER_CENTER,
                "Preview unavailable",
                FontId::proportional(t.number("text-sm")),
                t.color("text-faint"),
            );
        }
        None => events.push(Event::NeedsThumbnail(index)),
    }
    if card.missing {
        let label = shared::copy().missing;
        let size = t.number("text-sm");
        let width = text_width(ui, label, size) + 2. * t.number("s-4");
        let chip = Rect::from_center_size(
            image_rect.center(),
            Vec2::new(width, size + 2. * t.number("s-3")),
        );
        let chip_radius = t.number("r-md") as u8;
        painter.rect_filled(chip, chip_radius, t.color("surface-raised"));
        painter.rect_stroke(
            chip,
            chip_radius,
            Stroke::new(1., t.color("border")),
            StrokeKind::Inside,
        );
        painter.text(
            chip.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(size),
            t.color("text-muted"),
        );
    }
    painter.hline(
        rect.x_range(),
        image_rect.bottom() + 0.5,
        Stroke::new(1., t.color("border-subtle")),
    );

    let pad = t.number("s-5");
    let inner = rect.width() - 2. * pad;
    let mut y = image_rect.bottom() + shared::CARD_DIVIDER as f32 + pad;
    let line = |text: &str, y: f32, size: f32, color: Color32| {
        let mut job =
            LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(inner);
        let galley = painter.layout_job(job);
        painter.galley(Pos2::new(rect.left() + pad, y), galley, color);
    };
    line(&card.date, y, t.number("text-md"), t.color("text"));
    y += 18. + t.number("s-2");
    line(
        &card.details,
        y,
        t.number("text-sm"),
        t.color("text-subtle"),
    );
    if let Some(warning) = &card.warning {
        y += 16. + t.number("s-2");
        line(warning, y, t.number("text-sm"), t.color("caution-text"));
    }
    let width = (inner - gap) / 2.;
    // Stretched cards in a grown row keep their actions in place.
    let actions_top = rect.top() + shared::CARD_ACTIONS_TOP as f32;
    for (slot, action) in card.actions.iter().copied().enumerate() {
        let button_rect = Rect::from_min_size(
            Pos2::new(rect.left() + pad + slot as f32 * (width + gap), actions_top),
            Vec2::new(width, height),
        );
        let done = item
            .done
            .filter(|done| *done == action)
            .and_then(CardAction::done_label);
        let (label, icon) = if let Some(done) = done {
            (done, Some(Glyph::Named("check")))
        } else if item.busy == Some(action) {
            (action.busy_label(), action.icon().map(Glyph::for_icon))
        } else {
            (action.label(), action.icon().map(Glyph::for_icon))
        };
        let restore_unavailable = action == CardAction::Restore
            && ui
                .ctx()
                .data(|data| data.get_temp::<bool>(Id::unique("wayland-surface")))
                == Some(true);
        let action_enabled = idle && !restore_unavailable;
        let mut response = button(
            ui,
            t,
            button_rect,
            id.with(("action", slot)),
            label,
            icon,
            if action == CardAction::Edit {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            },
            action_enabled,
        );
        if std::env::var_os("CAPTURES_NATIVE_LAYOUT_PROBE").is_some() {
            println!(
                "{}",
                serde_json::json!({"event":"history-action-layout", "detail":{
                "id":item.id, "action":action.label(), "enabled":action_enabled,
                "rect":[button_rect.min.x, button_rect.min.y, button_rect.max.x, button_rect.max.y]}})
            );
        }
        if restore_unavailable {
            response = response
                .on_hover_text("Floating previews are not available on Wayland. Use Edit instead.");
        } else if let Some(tooltip) = action.tooltip() {
            response = response.on_hover_text(tooltip);
        }
        if response.clicked() {
            events.push(Event::Action(index, action));
        }
    }

    if let Some(error) = item.error {
        let text = error_galley(ui, t, error, rect.width());
        let padding = Vec2::new(
            shared::CARD_ERROR_PADDING_X as f32,
            shared::CARD_ERROR_PADDING_Y as f32,
        );
        let box_rect = Rect::from_min_size(
            Pos2::new(
                rect.left() + pad,
                actions_top + height + shared::CARD_ERROR_MARGIN as f32,
            ),
            Vec2::new(inner, text.size().y + 2. * padding.y),
        );
        painter.rect_filled(box_rect, t.number("r-sm") as u8, t.color("danger-surface"));
        let alert = ui.interact(box_rect, id.with("error"), Sense::hover());
        alert.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, error));
        painter.galley(box_rect.min + padding, text, t.color("danger-text"));
    }

    let trash = button(
        ui,
        t,
        trash_rect,
        id.with("delete"),
        "",
        Some(Glyph::Trash),
        if item.confirming_delete {
            ButtonStyle::ConfirmOverlay
        } else {
            ButtonStyle::Overlay
        },
        idle,
    );
    let delete_label = if item.confirming_delete {
        card.delete_confirm_label
    } else {
        card.delete_label
    };
    trash.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, idle, delete_label));
    let trash = trash.on_hover_text(if item.confirming_delete {
        card.delete_confirm_title
    } else {
        card.delete_label
    });

    if body.has_focus() || open.has_focus() {
        crate::primitives::focus_indicated(ui.ctx());
    }
    let stroke = if item.selected || body.has_focus() || open.has_focus() {
        Stroke::new(2., t.color("theme-accent"))
    } else if hover > 0. {
        Stroke::new(
            1.,
            t.color("border-subtle")
                .lerp_to_gamma(t.color("border-strong"), hover),
        )
    } else {
        Stroke::new(1., t.color("border-subtle"))
    };
    painter.rect_stroke(rect, r, stroke, StrokeKind::Inside);

    let started_outside_trash = ui
        .input(|input| input.pointer.press_origin())
        .is_some_and(|origin| !trash_rect.contains(origin));
    if can_drag && started_outside_trash && open.drag_started() {
        events.push(Event::DragFile(index));
    } else if open.clicked() {
        events.push(Event::Open(index));
    } else if body.clicked() {
        events.push(Event::Select(index));
    }
    if trash.clicked() {
        events.push(Event::Delete(index));
    }
    if enabled {
        for response in [&body, &open] {
            response.context_menu(|ui| {
                ui.spacing_mut().item_spacing.y = t.number("s-2");
                for action in card.menu.iter().copied() {
                    if ui
                        .add_enabled(idle, egui::Button::new(action.label()))
                        .clicked()
                    {
                        events.push(Event::Action(index, action));
                        ui.close();
                    }
                }
                if !card.menu.is_empty() {
                    ui.separator();
                }
                if ui
                    .add_enabled(idle, egui::Button::new(card.delete_label))
                    .clicked()
                {
                    events.push(Event::Delete(index));
                    ui.close();
                }
            });
        }
    }
}

/// `object-fit: contain` inside `area`.
pub fn contain(size: Vec2, area: Rect) -> Rect {
    if size.x <= 0. || size.y <= 0. {
        return Rect::from_center_size(area.center(), Vec2::ZERO);
    }
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::WHITE)
        .size()
        .x
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    /// `.history-edit`: the accent action.
    Primary,
    Secondary,
    /// Quiet destructive action (`.history-clear-all`).
    Danger,
    /// Awaiting confirmation (`.history-clear-all-confirm`).
    Confirm,
    Ghost,
    /// Translucent trash control over the thumbnail (`.history-delete`).
    Overlay,
    ConfirmOverlay,
}

#[derive(Clone, Copy)]
enum Glyph {
    Edit,
    Save,
    Trash,
    History,
    /// Any other shared shipping icon (`captures_app::icons`).
    Named(&'static str),
}

impl Glyph {
    fn for_icon(name: &'static str) -> Self {
        match name {
            "edit" => Self::Edit,
            "save" => Self::Save,
            name => Self::Named(name),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn button(
    ui: &mut egui::Ui,
    t: &Tokens,
    rect: Rect,
    id: Id,
    label: &str,
    icon: Option<Glyph>,
    style: ButtonStyle,
    enabled: bool,
) -> egui::Response {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if !label.is_empty() {
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    }
    let hovered = enabled && response.hovered();
    let (fill, border, text) = match (style, hovered) {
        (ButtonStyle::Primary, false) => (
            t.color("theme-accent"),
            Color32::TRANSPARENT,
            t.color("theme-accent-ink"),
        ),
        (ButtonStyle::Primary, true) => (
            t.color("theme-accent-hover"),
            Color32::TRANSPARENT,
            t.color("theme-accent-ink"),
        ),
        (ButtonStyle::Secondary, false) => (
            t.color("control"),
            t.color("control-border"),
            t.color("text"),
        ),
        (ButtonStyle::Secondary, true) => (
            t.color("control-hover"),
            t.color("border-strong"),
            t.color("text"),
        ),
        (ButtonStyle::Danger, false) => (
            Color32::TRANSPARENT,
            Color32::TRANSPARENT,
            t.color("text-subtle"),
        ),
        (ButtonStyle::Danger, true) => (
            t.color("danger-surface"),
            Color32::TRANSPARENT,
            t.color("danger-text"),
        ),
        (ButtonStyle::Confirm | ButtonStyle::ConfirmOverlay, false) => (
            t.color("theme-signal-strong"),
            Color32::TRANSPARENT,
            t.color("theme-signal-ink"),
        ),
        (ButtonStyle::Confirm | ButtonStyle::ConfirmOverlay, true) => (
            t.color("theme-signal-deep"),
            Color32::TRANSPARENT,
            t.color("theme-signal-ink"),
        ),
        (ButtonStyle::Ghost, false) => (
            Color32::TRANSPARENT,
            Color32::TRANSPARENT,
            t.color("text-muted"),
        ),
        (ButtonStyle::Ghost, true) => (
            t.color("surface-hover"),
            Color32::TRANSPARENT,
            t.color("text"),
        ),
        (ButtonStyle::Overlay, false) => (
            t.color("surface-raised").gamma_multiply(0.88),
            t.color("border-subtle"),
            t.color("text"),
        ),
        (ButtonStyle::Overlay, true) => (
            t.color("theme-signal"),
            Color32::TRANSPARENT,
            t.color("theme-signal-ink"),
        ),
    };
    let alpha = if enabled { 1. } else { 0.45 };
    let painter = ui.painter();
    let radius = t.number("r-md") as u8;
    painter.rect_filled(rect, radius, fill.gamma_multiply(alpha));
    if border != Color32::TRANSPARENT {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1., border.gamma_multiply(alpha)),
            StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        crate::primitives::focus_indicated(ui.ctx());
        painter.rect_stroke(
            rect.expand(1.),
            radius,
            Stroke::new(2., t.color("theme-accent")),
            StrokeKind::Outside,
        );
    }
    let color = text.gamma_multiply(alpha);
    let size = t.number("text-sm");
    let font = FontId::proportional(size);
    let icon_size = if matches!(style, ButtonStyle::Overlay | ButtonStyle::ConfirmOverlay) {
        16.
    } else {
        15.
    };
    let gap = if label.is_empty() {
        0.
    } else {
        t.number("s-3")
    };
    let label_width = if label.is_empty() {
        0.
    } else {
        painter
            .layout_no_wrap(label.to_owned(), font.clone(), color)
            .size()
            .x
    };
    let content = label_width + icon.map_or(0., |_| icon_size + gap);
    let mut x = rect.center().x - content.min(rect.width() - 8.) / 2.;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_min_size(
            Pos2::new(x, rect.center().y - icon_size / 2.),
            Vec2::splat(icon_size),
        );
        glyph(painter, icon, icon_rect, Stroke::new(1.7, color));
        x += icon_size + gap;
    }
    if !label.is_empty() {
        let mut job = LayoutJob::simple_singleline(label.to_owned(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - 4. - x).max(0.));
        let galley = painter.layout_job(job);
        let y = rect.center().y - galley.size().y / 2.;
        painter.galley(Pos2::new(x, y), galley, color);
    }
    response
}

/// Shipping 24-unit Lucide-style paths, scaled into `rect`.
fn glyph(painter: &egui::Painter, glyph: Glyph, rect: Rect, stroke: Stroke) {
    let p = |x: f32, y: f32| rect.min + Vec2::new(x, y) * (rect.width() / 24.);
    let name = match glyph {
        Glyph::Trash => "trash",
        Glyph::Edit => "edit",
        Glyph::Save => "save",
        Glyph::History => "history",
        Glyph::Named(name) => name,
    };
    for line in captures_app::icons::polylines(name).expect("shared History icon") {
        let points = line.iter().map(|[x, y]| p(*x, *y)).collect();
        painter.add(egui::Shape::line(points, stroke));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumbnail_gesture(
        enabled: bool,
        missing: bool,
        busy: bool,
        start: Pos2,
        end: Pos2,
    ) -> (bool, bool) {
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(300., 400.));
        let thumbnail = Thumbnail::Ready(ctx.load_texture(
            "gesture-image",
            egui::ColorImage::filled([20, 10], Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ));
        let model = shared::Card {
            kind_label: "Screenshot",
            date: "Today".into(),
            details: "20 × 10".into(),
            warning: None,
            missing,
            image_label: "Screenshot",
            open_label: (!missing).then_some("Open screenshot in editor"),
            delete_label: "Delete",
            delete_confirm_label: "Confirm delete",
            delete_confirm_title: "Delete forever",
            delete_requires_confirmation: true,
            actions: vec![CardAction::Edit, CardAction::Restore],
            menu: vec![],
        };
        let item = Item {
            id: "gesture",
            card: &model,
            thumbnail: Some(&thumbnail),
            selected: false,
            confirming_delete: false,
            busy: busy.then_some(CardAction::Edit),
            done: None,
            error: None,
        };
        let mut clicked = false;
        let mut dragged = false;
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    events,
                    ..Default::default()
                },
                |ui| {
                    // Render the production card in its scroll container;
                    // overlapping selection/Trash and scrolling own input too.
                    let output = grid(ui, &tokens, std::slice::from_ref(&item), enabled, None);
                    clicked |= output.events.contains(&Event::Open(0));
                    dragged |= output.events.contains(&Event::DragFile(0));
                },
            );
            output.textures_delta.clear();
        };
        // Register the interaction before the synthetic press, as a native
        // pointer event arrives against the shapes from the preceding frame.
        frame(Vec::new());
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        (clicked, dragged)
    }

    #[test]
    fn thumbnail_click_and_thresholded_drag_are_distinct() {
        assert_eq!(
            thumbnail_gesture(true, false, false, Pos2::new(20., 20.), Pos2::new(20., 20.)),
            (true, false)
        );
        assert_eq!(
            thumbnail_gesture(true, false, false, Pos2::new(20., 20.), Pos2::new(22., 20.)),
            (true, false)
        );
        assert_eq!(
            thumbnail_gesture(true, false, false, Pos2::new(20., 20.), Pos2::new(50., 20.)),
            (false, true)
        );
    }

    #[test]
    fn thumbnail_drag_respects_disabled_missing_busy_and_trash_boundaries() {
        let start = Pos2::new(20., 20.);
        let end = Pos2::new(50., 50.);
        // Disabled and busy cards have no open interaction; missing cards can
        // still preserve their click behavior but cannot export a file.
        assert_eq!(
            thumbnail_gesture(false, false, false, start, end),
            (false, false)
        );
        assert_eq!(
            thumbnail_gesture(true, false, true, start, end),
            (false, false)
        );
        assert_eq!(
            thumbnail_gesture(true, true, false, start, end),
            (false, false)
        );
        assert_eq!(
            thumbnail_gesture(
                true,
                false,
                false,
                Pos2::new(280., 15.),
                Pos2::new(40., 50.)
            ),
            (false, false)
        );
    }

    #[test]
    fn header_keeps_delete_all_clear_of_the_heading_down_to_the_minimum_window() {
        // Token fonts differ by platform; compare rects, not pixel positions.
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        let history = captures_app::app_windows::HISTORY;
        for width in [history.width, history.min_width] {
            for confirming in [false, true] {
                let mut layout = (Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
                for _ in 0..2 {
                    // The first pass loads the fonts.
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                egui::vec2(width, history.min_height),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let margin = tokens.number("s-8");
                            egui::Frame::new().inner_margin(margin).show(ui, |ui| {
                                let available = ui.max_rect();
                                let (_, heading, actions) = header_layout(
                                    ui,
                                    &tokens,
                                    DeleteAll {
                                        visible: true,
                                        confirming,
                                        busy: false,
                                        enabled: true,
                                    },
                                    captures_app::app_windows::compact(width),
                                );
                                layout = (available, heading, actions);
                            });
                        },
                    )
                    .textures_delta
                    .clear();
                }
                let (available, heading, actions) = layout;
                assert!(
                    heading.right() <= actions.left() || heading.bottom() <= actions.top(),
                    "{width}: heading {heading:?} runs under {actions:?}"
                );
                assert!(heading.right() <= available.right() + 0.5);
                assert!(
                    actions.right() <= available.right() + 0.5,
                    "{width}: actions {actions:?} run past {available:?}"
                );
            }
        }
    }

    #[test]
    fn thumbnails_fit_inside_the_card_image_like_object_fit_contain() {
        let area = Rect::from_min_size(Pos2::new(10., 20.), Vec2::new(300., 168.));
        let wide = contain(Vec2::new(568., 160.), area);
        assert!((wide.width() - 300.).abs() < 1e-3);
        assert!((wide.center() - area.center()).length() < 1e-3);
        let tall = contain(Vec2::new(100., 400.), area);
        assert!((tall.height() - 168.).abs() < 1e-3);
        assert!(tall.width() < 50.);
        assert_eq!(contain(Vec2::ZERO, area).size(), Vec2::ZERO);
    }
}
