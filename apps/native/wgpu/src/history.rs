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
}

/// Card input, by index into the rendered item slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Select(usize),
    /// The thumbnail's open action (shipping: open in the editor).
    Open(usize),
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
    let copy = shared::copy();
    let mut event = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
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
        });
        if !delete_all.visible {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
            let label = if delete_all.busy {
                copy.delete_all_busy
            } else if delete_all.confirming {
                copy.delete_all_confirm
            } else {
                copy.delete_all
            };
            let style = if delete_all.confirming {
                ButtonStyle::Confirm
            } else {
                ButtonStyle::Danger
            };
            let width = text_width(ui, label, t.number("text-sm"))
                + 15.
                + t.number("s-3")
                + 2. * t.number("s-5");
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(width, t.number("h-md")), Sense::hover());
            let response = button(
                ui,
                t,
                rect,
                Id::unique("history-delete-all"),
                label,
                Some(Glyph::Trash),
                style,
                delete_all.enabled && !delete_all.busy,
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    delete_all.enabled && !delete_all.busy,
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
            if delete_all.confirming {
                let width = text_width(ui, copy.cancel, t.number("text-sm")) + 2. * t.number("s-5");
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(width, t.number("h-md")), Sense::hover());
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
                if cancel.clicked() {
                    event = Some(HeaderEvent::Cancel);
                }
            }
        });
    });
    event
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
    egui::ScrollArea::vertical()
        .id_salt("history-grid")
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            width = f64::from(ui.available_width());
            let layout = shared::grid(width);
            ui.set_height(layout.content_height(items.len()) as f32);
            let origin = ui.max_rect().min;
            let rect_for = |index: usize| {
                let (x, y) = layout.origin(index);
                Rect::from_min_size(
                    origin + Vec2::new(x as f32, y as f32),
                    Vec2::new(layout.card_width as f32, layout.card_height as f32),
                )
            };
            let rows = layout.visible_rows(
                items.len(),
                f64::from(viewport.min.y),
                f64::from(viewport.height()),
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
        });
    GridOutput {
        events,
        visible,
        width,
    }
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
    let open = ui.interact(
        image_rect,
        id.with("open"),
        if can_open {
            Sense::click()
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
    let gap = t.number("s-3");
    let height = t.number("h-md");
    let width = (inner - gap) / 2.;
    for (slot, action) in card.actions.iter().copied().enumerate() {
        let button_rect = Rect::from_min_size(
            Pos2::new(
                rect.left() + pad + slot as f32 * (width + gap),
                rect.bottom() - pad - height,
            ),
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
            idle,
        );
        if let Some(tooltip) = action.tooltip() {
            response = response.on_hover_text(tooltip);
        }
        if response.clicked() {
            events.push(Event::Action(index, action));
        }
    }

    let trash_rect = Rect::from_min_size(
        Pos2::new(rect.right() - gap - height, rect.top() + gap),
        Vec2::splat(height),
    );
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

    if open.clicked() {
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
    let path = |points: &[(f32, f32)]| {
        for pair in points.windows(2) {
            painter.line_segment([p(pair[0].0, pair[0].1), p(pair[1].0, pair[1].1)], stroke);
        }
    };
    match glyph {
        Glyph::Trash => {
            path(&[(4., 7.), (20., 7.)]);
            path(&[(9., 7.), (9., 4.), (15., 4.), (15., 7.)]);
            path(&[(18., 7.), (17., 20.), (7., 20.), (6., 7.)]);
            path(&[(10., 11.), (10., 16.)]);
            path(&[(14., 11.), (14., 16.)]);
        }
        Glyph::Edit => {
            path(&[
                (4., 16.),
                (3., 21.),
                (8., 20.),
                (19., 9.),
                (15., 5.),
                (4., 16.),
            ]);
            path(&[(13.5, 6.5), (17.5, 10.5)]);
        }
        Glyph::Save => {
            path(&[
                (5., 4.),
                (17., 4.),
                (19., 6.),
                (19., 20.),
                (5., 20.),
                (5., 4.),
            ]);
            path(&[(8., 4.), (8., 10.), (16., 10.), (16., 4.)]);
            path(&[(8., 20.), (8., 14.), (16., 14.), (16., 20.)]);
        }
        Glyph::History => {
            painter.circle_stroke(p(12., 12.), 9. * rect.width() / 24., stroke);
            path(&[(3., 3.), (3., 8.), (8., 8.)]);
            path(&[(12., 7.), (12., 12.), (15., 14.)]);
        }
        Glyph::Named(name) => {
            for line in captures_app::icons::polylines(name).unwrap_or_default() {
                let points = line.iter().map(|[x, y]| p(*x, *y)).collect();
                painter.add(egui::Shape::line(points, stroke));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
