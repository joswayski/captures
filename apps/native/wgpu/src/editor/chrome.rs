//! Shipping screenshot-editor chrome: the header bar, restored-draft and status
//! banners, the left tool rail with its Shapes flyout, and the floating
//! Recenter pill (`ScreenshotEditor.tsx`, `styles/editor-image.css`). Copy,
//! icon names and responsive rules come from `captures_app::editor_chrome`.
use super::*;
use captures_app::editor_chrome::{self as model, header as copy};
use eframe::egui::{
    Align, Color32, FontFamily, FontId, Layout, Pos2, Sense, Stroke, StrokeKind, UiBuilder, Vec2,
    pos2, vec2,
};

/// Stroke a shared shipping icon (24-unit grid) centred in `center`.
pub(super) fn icon(
    painter: &egui::Painter,
    name: &str,
    center: Pos2,
    side: f32,
    width: f32,
    color: Color32,
) {
    let scale = side / 24.;
    let origin = center - Vec2::splat(side / 2.);
    let stroke = Stroke::new(width * scale, color);
    for line in captures_app::icons::polylines(name).unwrap_or_default() {
        let points: Vec<Pos2> = line
            .iter()
            .map(|[x, y]| origin + vec2(x * scale, y * scale))
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    }
}

fn font(tokens: &Tokens, size: &str, semibold: bool) -> FontId {
    FontId::new(
        tokens.number(size),
        if semibold {
            FontFamily::Name("semibold".into())
        } else {
            FontFamily::Proportional
        },
    )
}

fn galley(ui: &egui::Ui, text: &str, font: FontId, color: Color32) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

/// Disabled chrome dims to `opacity: 0.32` like shipping.
fn dim(color: Color32, enabled: bool) -> Color32 {
    if enabled {
        color
    } else {
        color.gamma_multiply(0.32)
    }
}

fn accessible(response: &egui::Response, kind: egui::WidgetType, selected: bool, label: &str) {
    response.widget_info(|| egui::WidgetInfo::selected(kind, response.enabled(), selected, label));
}

/// Whether the document may accept a header/rail action now.
pub(super) fn edit_enabled(view: &View) -> bool {
    view.presented.is_some()
        && !view.pending
        && view.inline.is_none()
        && !view.closed
        && !view.close_requested
        && !view.confirm_discard
}

/// Laid-out header regions, for the no-overlap layout rule (read by tests).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Header {
    /// The left Canvas toolbar, clipped to the space the controls leave.
    pub canvas: egui::Rect,
    /// Union of the right-aligned history, zoom, image and draft controls.
    pub controls: egui::Rect,
    /// Undo/Redo are hidden at or below 1040 points, as in shipping.
    pub history_visible: bool,
}

/// Banners above the header row: a restored draft and confirmations. Errors
/// use the export status line, as in shipping.
pub(super) fn show_banners(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    if view.draft_restored {
        let mut discard = false;
        // Actions lay out right to left: add them in reverse reading order.
        banner(ui, tokens, "caution", copy::DRAFT_RESTORED, |ui| {
            let dismiss = banner_button(ui, tokens, copy::DRAFT_DISMISS, true, false);
            accessible(
                &dismiss,
                egui::WidgetType::Button,
                false,
                copy::DRAFT_DISMISS_LABEL,
            );
            if dismiss.clicked() {
                view.draft_restored = false;
            }
            discard =
                banner_button(ui, tokens, copy::DRAFT_DISCARD, edit_enabled(view), true).clicked();
        });
        if discard {
            // Shipping discards the restored draft immediately.
            view.draft_restored = false;
            view.submit(tx, Request::DiscardDraft);
        }
    }
    if view.close_requested && !view.pending {
        let mut action = None;
        banner(
            ui,
            tokens,
            "caution",
            "Save unsaved edits before closing?",
            |ui| {
                for (index, label) in ["Save and close", "Close without saving", "Cancel close"]
                    .into_iter()
                    .enumerate()
                    .rev()
                {
                    if banner_button(ui, tokens, label, true, index < 2).clicked() {
                        action = Some(index);
                    }
                }
            },
        );
        match action {
            Some(0) => {
                view.close_after_save = true;
                view.save_draft(tx);
            }
            Some(1) => view.closed = true,
            Some(_) => view.close_requested = false,
            None => {}
        }
    }
    if view.confirm_discard {
        let mut action = None;
        banner(
            ui,
            tokens,
            "caution",
            "Discard all edits and the saved draft? The original capture and exports stay unchanged.",
            |ui| {
                let enabled = !view.pending;
                if banner_button(ui, tokens, "Cancel discard", enabled, false).clicked() {
                    action = Some(false);
                }
                if banner_button(ui, tokens, "Discard edits", enabled, true).clicked() {
                    action = Some(true);
                }
            },
        );
        match action {
            Some(true) => {
                view.confirm_discard = false;
                view.submit(tx, Request::DiscardDraft);
            }
            Some(false) => view.confirm_discard = false,
            None => {}
        }
    }
}

/// Shipping `.screenshot-editor-draft-banner`: one text-sm row with actions on
/// the right, `padding: 6px 12px`, a subtle bottom rule.
fn banner(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    tone: &str,
    text: &str,
    actions: impl FnOnce(&mut egui::Ui),
) {
    let pad = vec2(tokens.number("s-5"), tokens.number("s-3"));
    let ink = tokens.color(&format!("{tone}-text"));
    let message = galley(ui, text, font(tokens, "text-sm", false), ink);
    // Lay the actions out first, right to left, to learn their height.
    let width = ui.available_width();
    let top = ui.cursor().top();
    let probe = egui::Rect::from_min_size(
        pos2(ui.max_rect().left() + pad.x, top + pad.y),
        vec2(width - 2. * pad.x, tokens.number("h-sm")),
    );
    let background = ui.painter().add(egui::Shape::Noop);
    // A detached child: it must not advance this Ui's cursor.
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(probe)
            .layout(Layout::right_to_left(Align::Center)),
    );
    row.spacing_mut().item_spacing.x = tokens.number("s-3");
    actions(&mut row);
    let actions_rect = row.min_rect();
    let has_actions = actions_rect.width() > 0.;
    let content = if has_actions {
        tokens.number("h-sm")
    } else {
        message.size().y
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, content + 2. * pad.y), Sense::hover());
    ui.painter().set(
        background,
        egui::Shape::rect_filled(rect, 0., tokens.color(&format!("{tone}-surface"))),
    );
    let right = if has_actions {
        actions_rect.left() - tokens.number("s-5")
    } else {
        rect.right() - pad.x
    };
    let text_rect = egui::Rect::from_min_max(
        pos2(rect.left() + pad.x, rect.top()),
        pos2(right.max(rect.left() + pad.x), rect.bottom()),
    );
    ui.painter().with_clip_rect(text_rect).galley(
        pos2(text_rect.left(), rect.center().y - message.size().y / 2.),
        message,
        ink,
    );
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1., tokens.color("border-subtle")),
    );
}

/// Banner action: a small control button (`min-height: var(--h-sm)`), or a
/// transparent one for dismissive actions.
fn banner_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    enabled: bool,
    filled: bool,
) -> egui::Response {
    let text = galley(
        ui,
        label,
        font(tokens, "text-sm", false),
        Color32::PLACEHOLDER,
    );
    let size = vec2(
        text.size().x + 2. * tokens.number("s-4"),
        tokens.number("h-sm"),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = if enabled {
        response
    } else {
        ui.interact(rect, response.id, Sense::hover())
    };
    let hovered = enabled && response.hovered();
    let radius = tokens.number("r-md");
    if filled {
        ui.painter().rect(
            rect,
            radius,
            tokens.color(if hovered { "control-hover" } else { "control" }),
            Stroke::new(1., tokens.color("control-border")),
            StrokeKind::Inside,
        );
    } else if hovered {
        ui.painter()
            .rect_filled(rect, radius, tokens.color("surface-hover"));
    }
    let ink = if filled {
        tokens.color("text")
    } else {
        ui.visuals()
            .override_text_color
            .unwrap_or(tokens.color("text"))
    };
    ui.painter()
        .galley(rect.center() - text.size() / 2., text, dim(ink, enabled));
    accessible(&response, egui::WidgetType::Button, false, label);
    if enabled {
        response
    } else {
        response.on_hover_cursor(egui::CursorIcon::Default)
    }
}

/// The 52 px header row: the Canvas toolbar on the left; Undo, Redo, the zoom
/// group, Add images and the native draft menu on the right.
///
/// Controls are laid out first, right to left. The Canvas toolbar gets the width
/// left over; like shipping's `overflow: hidden` it first drops its label and
/// then clips, so zoom stays reachable at the 760 px minimum.
pub(super) fn show_header(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
) -> Header {
    let layout = model::header_layout(f64::from(ui.ctx().content_rect().width()));
    let (row, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), model::HEADER_HEIGHT as f32),
        Sense::hover(),
    );
    ui.painter().hline(
        row.x_range(),
        row.bottom() - 0.5,
        Stroke::new(1., tokens.color("border-subtle")),
    );
    let inner = row.shrink2(vec2(tokens.number("s-5"), 0.));
    let enabled = edit_enabled(view);
    let controls = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = tokens.number("s-2");
                draft_menu(ui, tokens, view, tx, enabled);
                add_images(ui, tokens, view, enabled);
                zoom_group(ui, tokens, view, layout);
                if layout.show_history {
                    let presented = view.presented.as_ref();
                    let can_redo = enabled && presented.is_some_and(|p| p.can_redo);
                    let can_undo = enabled && presented.is_some_and(|p| p.can_undo);
                    if quiet_button(ui, tokens, "redo", copy::REDO, can_redo).clicked() {
                        view.submit(tx, Request::Redo);
                    }
                    if quiet_button(ui, tokens, "undo", copy::UNDO, can_undo).clicked() {
                        view.submit(tx, Request::Undo);
                    }
                }
                ui.min_rect()
            },
        )
        .inner;
    let left = egui::Rect::from_min_max(
        inner.left_top(),
        pos2(
            (controls.left() - tokens.number("s-5")).max(inner.left()),
            inner.bottom(),
        ),
    );
    let canvas = canvas_toolbar(ui, tokens, view, tx, left, enabled);
    Header {
        canvas,
        controls,
        history_visible: layout.show_history,
    }
}

/// Shipping `.screenshot-editor-history-actions > button`: a 34 px square,
/// muted icon that lifts to `--surface-hover` on hover.
fn quiet_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    name: &str,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let side = model::HEADER_CONTROL as f32;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let response = if enabled {
        response
    } else {
        ui.interact(rect, response.id, Sense::hover())
    };
    let hovered = enabled && response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, tokens.number("r-md"), tokens.color("surface-hover"));
    }
    let ink = tokens.color(if hovered { "text" } else { "text-muted" });
    icon(
        ui.painter(),
        name,
        rect.center(),
        16.,
        1.8,
        dim(ink, enabled),
    );
    accessible(&response, egui::WidgetType::Button, false, label);
    response
}

fn draft_menu(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    enabled: bool,
) {
    let response =
        quiet_button(ui, tokens, "more", copy::DRAFT_MENU, enabled).on_hover_text(copy::DRAFT_MENU);
    egui::Popup::menu(&response)
        .align(egui::RectAlign::BOTTOM_END)
        .show(|ui| {
            if ui.button(copy::SAVE_DRAFT).clicked() {
                view.save_draft(tx);
                ui.close();
            }
            if ui.button(copy::DISCARD_EDITS).clicked() {
                view.confirm_discard = true;
                ui.close();
            }
        });
}

/// Shipping `.screenshot-add-image`: a bordered control with the image icon.
fn add_images(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, enabled: bool) {
    let enabled = enabled && view.import_picker.is_none();
    let text = galley(
        ui,
        copy::ADD_IMAGES,
        font(tokens, "text-sm", false),
        Color32::PLACEHOLDER,
    );
    let gap = tokens.number("s-3");
    let size = vec2(
        2. * tokens.number("s-5") + 16. + gap + text.size().x,
        model::HEADER_CONTROL as f32,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = if enabled {
        response
    } else {
        ui.interact(rect, response.id, Sense::hover())
    };
    let hovered = enabled && response.hovered();
    ui.painter().rect(
        rect,
        tokens.number("r-md"),
        tokens.color(if hovered { "control-hover" } else { "control" }),
        Stroke::new(
            1.,
            tokens.color(if hovered {
                "border-strong"
            } else {
                "control-border"
            }),
        ),
        StrokeKind::Inside,
    );
    let ink = dim(tokens.color("text"), enabled);
    let left = rect.left() + tokens.number("s-5");
    icon(
        ui.painter(),
        "image",
        pos2(left + 8., rect.center().y),
        16.,
        1.8,
        ink,
    );
    ui.painter().galley(
        pos2(left + 16. + gap, rect.center().y - text.size().y / 2.),
        text,
        ink,
    );
    accessible(&response, egui::WidgetType::Button, false, copy::ADD_IMAGES);
    // `margin-left: var(--s-2)` on top of the group gap.
    ui.add_space(tokens.number("s-2"));
    if response.clicked() {
        view.choose_image(ui.ctx());
    }
}

/// Shipping `.screenshot-editor-zoom`: Fit, −, a log-scale slider, + and the
/// preset menu in one sunken, bordered group with 1 px dividers.
fn zoom_group(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, layout: model::HeaderLayout) {
    let button = model::ZOOM_BUTTON as f32;
    let slider = layout.zoom_slider_width as f32;
    let preset = layout.zoom_preset_width as f32;
    let width = 3. * button + slider + preset + 4. + 2.;
    let (group, _) =
        ui.allocate_exact_size(vec2(width, model::HEADER_CONTROL as f32), Sense::hover());
    ui.add_space(tokens.number("s-2"));
    let radius = tokens.number("r-lg");
    let border = tokens.color("border-subtle");
    ui.painter()
        .rect_filled(group, radius, tokens.color("surface-sunken"));
    let inner = group.shrink(1.);
    let mut x = inner.left();
    let mut next = |w: f32| {
        let rect = egui::Rect::from_min_size(pos2(x, inner.top()), vec2(w, inner.height()));
        x += w + 1.;
        rect
    };
    let fit_rect = next(button);
    let minus_rect = next(button);
    let slider_rect = next(slider);
    let plus_rect = next(button);
    let preset_rect = next(preset);
    let painter = ui.painter().with_clip_rect(inner);
    for rect in [fit_rect, minus_rect, slider_rect, plus_rect] {
        painter.vline(rect.right() + 0.5, inner.y_range(), Stroke::new(1., border));
    }
    let percent = displayed_zoom(view);
    let fit_active = view.viewport.zoom_percent == 0.;
    // Fills follow the group's rounded outer corners (`overflow: hidden`).
    let outer = (radius - 1.).max(0.) as u8;
    let first = egui::CornerRadius {
        nw: outer,
        sw: outer,
        ..Default::default()
    };
    let last = egui::CornerRadius {
        ne: outer,
        se: outer,
        ..Default::default()
    };
    if zoom_button(
        ui,
        tokens,
        fit_rect,
        first,
        "fit",
        copy::FIT,
        true,
        fit_active,
    )
    .on_hover_text(copy::FIT_TOOLTIP)
    .clicked()
    {
        view.reset_viewport();
    }
    let can_out = percent.is_some_and(|p| p > 5. + 0.05);
    if zoom_button(
        ui,
        tokens,
        minus_rect,
        egui::CornerRadius::ZERO,
        "minus",
        copy::ZOOM_OUT,
        can_out,
        false,
    )
    .on_hover_text(copy::ZOOM_OUT)
    .clicked()
    {
        change_viewport_zoom(view, 1. / 1.25, None);
    }
    zoom_slider(ui, tokens, view, slider_rect, layout, percent);
    let can_in = percent.is_some_and(|p| p < 800. - 0.05);
    if zoom_button(
        ui,
        tokens,
        plus_rect,
        egui::CornerRadius::ZERO,
        "plus",
        copy::ZOOM_IN,
        can_in,
        false,
    )
    .on_hover_text(copy::ZOOM_IN)
    .clicked()
    {
        change_viewport_zoom(view, 1.25, None);
    }
    zoom_presets(ui, tokens, view, preset_rect, last);
    ui.painter()
        .rect_stroke(group, radius, Stroke::new(1., border), StrokeKind::Inside);
}

#[allow(clippy::too_many_arguments)]
fn zoom_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    corners: egui::CornerRadius,
    name: &str,
    label: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let id = ui.scope_id().with(("zoom", name));
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let dark = ui.visuals().dark_mode;
    if active {
        ui.painter()
            .rect_filled(rect, corners, tokens.color("surface-selected"));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, corners, tokens.color("surface-hover"));
    }
    let ink = if active {
        tokens.color(if dark {
            "theme-accent-text"
        } else {
            "theme-accent-readable"
        })
    } else {
        tokens.color(if hovered { "text" } else { "text-muted" })
    };
    icon(
        ui.painter(),
        name,
        rect.center(),
        14.,
        1.8,
        dim(ink, enabled),
    );
    accessible(&response, egui::WidgetType::Button, active, label);
    response
}

/// Continuous log-scale zoom. The thumb centre travels the track inset by its
/// radius, so a click on either end pixel reaches 5% or 800%.
fn zoom_slider(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    rect: egui::Rect,
    layout: model::HeaderLayout,
    percent: Option<f64>,
) {
    let pad = tokens.number(if layout.show_history { "s-4" } else { "s-3" });
    let track = rect.shrink2(vec2(pad, 0.));
    let enabled = percent.is_some() && !view.pending;
    let response = ui.interact(
        track,
        ui.scope_id().with("zoom-slider"),
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let radius = 6.;
    let travel = (track.left() + radius)..=(track.right() - radius);
    if enabled
        && (response.dragged() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let span = travel.end() - travel.start();
        let position = f64::from(((pointer.x - travel.start()) / span).clamp(0., 1.));
        if let Some(zoom) = zoom_from_slider(position) {
            set_viewport_zoom(view, zoom, None);
        }
    }
    let position = displayed_zoom(view)
        .and_then(zoom_slider_position)
        .unwrap_or(0.) as f32;
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from_center_size(track.center(), vec2(track.width(), 3.)),
        tokens.number("r-pill"),
        dim(tokens.color("n-6"), enabled),
    );
    let thumb = pos2(
        travel.start() + (travel.end() - travel.start()) * position,
        track.center().y,
    );
    painter.circle(
        thumb,
        radius,
        tokens.color("surface-raised"),
        Stroke::new(1., dim(tokens.color("border-strong"), enabled)),
    );
    let label = match percent {
        Some(value) if view.viewport.zoom_percent == 0. => {
            format!("Fit ({})", model::zoom_label(value))
        }
        Some(value) => model::zoom_label(value),
        None => copy::ZOOM_SLIDER.into(),
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Slider,
            enabled,
            format!("{}: {label}", copy::ZOOM_SLIDER),
        )
    });
    response.on_hover_text(copy::ZOOM_SLIDER_TOOLTIP);
}

/// Shipping `.screenshot-editor-zoom-presets`: a mono, tabular value with a
/// chevron opening Fit, the current custom zoom, 50%, 100% and 200%.
fn zoom_presets(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    rect: egui::Rect,
    corners: egui::CornerRadius,
) {
    let response = ui
        .interact(
            rect,
            ui.scope_id().with("viewport-zoom-preset"),
            Sense::click(),
        )
        .on_hover_text(copy::ZOOM_PRESET_TOOLTIP);
    let current = view.viewport.zoom_percent;
    let label = if current == 0. {
        "Fit".to_owned()
    } else {
        model::zoom_label(current)
    };
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, corners, tokens.color("surface-hover"));
    }
    let ink = tokens.color("text");
    let text = galley(ui, &label, FontId::monospace(tokens.number("text-xs")), ink);
    ui.painter().galley(
        pos2(
            rect.left() + tokens.number("s-4"),
            rect.center().y - text.size().y / 2.,
        ),
        text,
        ink,
    );
    icon(
        ui.painter(),
        "chevron-down",
        pos2(rect.right() - 10., rect.center().y),
        12.,
        1.8,
        tokens.color("text-subtle"),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ComboBox,
            true,
            format!("{}: {label}", copy::ZOOM_PRESET),
        )
    });
    let mut chosen = None;
    egui::Popup::menu(&response)
        .align(egui::RectAlign::BOTTOM_END)
        .show(|ui| {
            if ui.selectable_label(current == 0., "Fit").clicked() {
                chosen = Some(0.);
            }
            if current != 0.
                && !copy::ZOOM_PRESETS.contains(&current)
                && ui
                    .selectable_label(true, model::zoom_label(current))
                    .clicked()
            {
                chosen = Some(current);
            }
            for percent in copy::ZOOM_PRESETS {
                if ui
                    .selectable_label(current == percent, model::zoom_label(percent))
                    .clicked()
                {
                    chosen = Some(percent);
                }
            }
        });
    match chosen {
        Some(0.) => view.reset_viewport(),
        Some(percent) => set_viewport_zoom(view, percent, None),
        None => {}
    }
}

/// Shipping `.screenshot-canvas-toolbar`: Canvas W × H fields, Trim edges and
/// the canvas background picker. When the controls leave too little room it
/// drops the "Canvas" label, then shows Trim and Background as icon buttons
/// (keeping their tooltips), and only then clips.
fn canvas_toolbar(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    available: egui::Rect,
    enabled: bool,
) -> egui::Rect {
    let subtle = tokens.color("text-subtle");
    let xs = font(tokens, "text-xs", false);
    let sm = font(tokens, "text-sm", false);
    let label = galley(ui, copy::CANVAS, xs.clone(), subtle);
    let w = galley(ui, "W", xs.clone(), subtle);
    let h = galley(ui, "H", xs.clone(), subtle);
    let times = galley(ui, "×", xs, subtle);
    let trim_text = galley(ui, copy::TRIM, sm.clone(), Color32::PLACEHOLDER);
    let background_text = galley(ui, BACKGROUND, sm, Color32::PLACEHOLDER);
    let field = model::CANVAS_FIELD as f32;
    let gap = tokens.number("s-2");
    let pad = tokens.number("s-4");
    let icon_gap = tokens.number("s-3");
    let label_width = pad + label.size().x + tokens.number("s-3");
    // Tool buttons: `padding: 0 8px; gap: 6px`; Background adds a chevron and
    // `padding-right: 6px`.
    let trim_full = 2. * pad + 13. + icon_gap + trim_text.size().x;
    let background_full =
        pad + 14. + icon_gap + background_text.size().x + icon_gap + 11. + tokens.number("s-3");
    let compact_tool = 28.;
    let dims = w.size().x + gap + field + 2. + times.size().x + 2. + h.size().x + gap + field;
    let split = 2. + 1. + 2. * gap + 2.;
    let total = |show_label: bool, full: bool| {
        3. + if show_label { label_width + 2. } else { 0. }
            + dims
            + split
            + if full {
                trim_full + 2. + background_full
            } else {
                2. * compact_tool + 2.
            }
            + 3.
    };
    let (show_label, full) = if total(true, true) <= available.width() {
        (true, true)
    } else if total(false, true) <= available.width() {
        (false, true)
    } else {
        (false, false)
    };
    let width = total(show_label, full).min(available.width()).max(0.);
    let height = model::HEADER_CONTROL as f32;
    let rect = egui::Rect::from_min_size(
        pos2(available.left(), available.center().y - height / 2.),
        vec2(width, height),
    );
    if width <= 0. {
        return rect;
    }
    let clip = rect.intersect(ui.clip_rect());
    let radius = tokens.number("r-lg");
    ui.painter()
        .rect_filled(rect, radius, tokens.color("surface-sunken"));
    let painter = ui.painter().with_clip_rect(clip);
    let mut x = rect.left() + 3.;
    let center = rect.center().y;
    if show_label {
        painter.galley(pos2(x + pad, center - label.size().y / 2.), label, subtle);
        x += label_width + 2.;
    }
    let document = view
        .presented
        .as_ref()
        .map(|presented| [presented.document.width, presented.document.height]);
    for (axis, letter) in [(0, w), (1, h)] {
        painter.galley(
            pos2(x, center - letter.size().y / 2.),
            letter.clone(),
            subtle,
        );
        x += letter.size().x + gap;
        let field_rect = egui::Rect::from_min_size(pos2(x, center - 14.), vec2(field, 28.));
        canvas_field(
            ui, tokens, view, tx, axis, field_rect, clip, document, enabled,
        );
        x += field;
        if axis == 0 {
            x += 2.;
            painter.galley(pos2(x, center - times.size().y / 2.), times.clone(), subtle);
            x += times.size().x + 2.;
        }
    }
    painter.vline(
        x + 2. + gap + 0.5,
        (center - 8.)..=(center + 8.),
        Stroke::new(1., tokens.color("border")),
    );
    x += split;
    let trim = egui::Rect::from_min_size(
        pos2(x, center - 14.),
        vec2(if full { trim_full } else { compact_tool }, 28.),
    );
    let response = canvas_tool(
        ui,
        tokens,
        &painter,
        trim,
        clip,
        "canvas-trim",
        enabled,
        false,
    );
    let ink = canvas_tool_ink(tokens, &response, enabled, false);
    let icon_center = if full {
        pos2(trim.left() + pad + 6.5, center)
    } else {
        trim.center()
    };
    icon(&painter, "trim", icon_center, 13., 1.8, ink);
    if full {
        painter.galley(
            pos2(
                icon_center.x + 6.5 + icon_gap,
                center - trim_text.size().y / 2.,
            ),
            trim_text,
            ink,
        );
    }
    accessible(&response, egui::WidgetType::Button, false, copy::TRIM);
    if response.on_hover_text(copy::TRIM_TOOLTIP).clicked() {
        view.submit(tx, Request::TrimCanvas);
    }
    x = trim.right() + 2.;
    let background = egui::Rect::from_min_size(
        pos2(x, center - 14.),
        vec2(if full { background_full } else { compact_tool }, 28.),
    );
    canvas_background(
        ui,
        tokens,
        view,
        tx,
        &painter,
        background,
        clip,
        full.then_some(background_text),
        enabled,
    );
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1., tokens.color("border-subtle")),
        StrokeKind::Inside,
    );
    rect
}

const BACKGROUND: &str = "Background color";
const BACKGROUND_TOOLTIP: &str = "Canvas background color";

/// Shipping `.screenshot-canvas-tool`: transparent until hover or open.
#[allow(clippy::too_many_arguments)]
fn canvas_tool(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    painter: &egui::Painter,
    rect: egui::Rect,
    clip: egui::Rect,
    id: &str,
    enabled: bool,
    open: bool,
) -> egui::Response {
    let response = ui.interact(
        rect.intersect(clip),
        ui.scope_id().with(id),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if (enabled && response.hovered()) || open {
        painter.rect_filled(rect, tokens.number("r-sm"), tokens.color("surface-hover"));
    }
    response
}

fn canvas_tool_ink(
    tokens: &Tokens,
    response: &egui::Response,
    enabled: bool,
    open: bool,
) -> Color32 {
    let ink = tokens.color(if (enabled && response.hovered()) || open {
        "text"
    } else {
        "text-muted"
    });
    if enabled {
        ink
    } else {
        ink.gamma_multiply(0.4)
    }
}

/// Shipping `CanvasBackgroundPicker`: a color chip trigger opening the canvas
/// background card. Native keeps its explicit hex field and Apply.
#[allow(clippy::too_many_arguments)]
fn canvas_background(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    painter: &egui::Painter,
    rect: egui::Rect,
    clip: egui::Rect,
    text: Option<std::sync::Arc<egui::Galley>>,
    enabled: bool,
) {
    let popup_id = ui.scope_id().with("canvas-background-menu");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = canvas_tool(
        ui,
        tokens,
        painter,
        rect,
        clip,
        "canvas-background",
        enabled,
        open,
    );
    let ink = canvas_tool_ink(tokens, &response, enabled, open);
    let background = view
        .presented
        .as_ref()
        .and_then(|presented| presented.document.background.clone());
    let chip = egui::Rect::from_center_size(
        if text.is_some() {
            pos2(rect.left() + tokens.number("s-4") + 7., rect.center().y)
        } else {
            rect.center()
        },
        Vec2::splat(14.),
    );
    match background
        .as_deref()
        .and_then(|color| Color32::from_hex(color).ok())
    {
        Some(color) => {
            painter.rect_filled(chip, tokens.number("r-xs"), color);
        }
        None => {
            // Transparent: the shipping 4 px checkerboard.
            painter.rect_filled(chip, tokens.number("r-xs"), tokens.color("n-5"));
            for row in 0..4 {
                for column in 0..4 {
                    if (row + column) % 2 == 0 {
                        let cell = egui::Rect::from_min_size(
                            chip.min + vec2(column as f32 * 3.5, row as f32 * 3.5),
                            Vec2::splat(3.5),
                        );
                        painter.rect_filled(cell, 0., tokens.color("surface-raised"));
                    }
                }
            }
        }
    }
    painter.rect_stroke(
        chip,
        tokens.number("r-xs"),
        Stroke::new(1., tokens.color("border-strong")),
        StrokeKind::Inside,
    );
    if let Some(text) = text {
        let text_left = chip.right() + tokens.number("s-3");
        let text_width = text.size().x;
        painter.galley(
            pos2(text_left, rect.center().y - text.size().y / 2.),
            text,
            ink,
        );
        icon(
            painter,
            "chevron-down",
            pos2(
                text_left + text_width + tokens.number("s-3") + 5.5,
                rect.center().y,
            ),
            11.,
            1.8,
            tokens.color("text-subtle"),
        );
    }
    let label = format!(
        "Background color: {}",
        background.as_deref().unwrap_or("transparent")
    );
    accessible(&response, egui::WidgetType::Button, open, &label);
    let response = response.on_hover_text(BACKGROUND_TOOLTIP);
    egui::Popup::menu(&response)
        .id(popup_id)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(tokens.number("s-3"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .frame(
            egui::Frame::NONE
                .fill(tokens.color("surface-overlay"))
                .stroke(Stroke::new(1., tokens.color("border")))
                .corner_radius(tokens.number("r-xl"))
                .inner_margin(tokens.number("s-5"))
                .shadow(ui.visuals().popup_shadow),
        )
        .show(|ui| {
            ui.set_width(236.);
            ui.add_enabled_ui(enabled, |ui| {
                ui.checkbox(&mut view.background_solid, "Solid background");
                ui.add_enabled(
                    view.background_solid,
                    egui::TextEdit::singleline(&mut view.background_color)
                        .desired_width(ui.available_width())
                        .hint_text("#RRGGBB or #RRGGBBAA"),
                );
                ui.horizontal(|ui| {
                    if ui.button("Apply background").clicked() {
                        view.submit(
                            tx,
                            Request::SetBackground {
                                color: view.background_solid.then(|| view.background_color.clone()),
                            },
                        );
                    }
                    if ui.button("Reset fields").clicked() {
                        view.reset_background_fields();
                    }
                });
                ui.small("Changes the canvas fill, not an image layer's background.");
            });
        });
}

/// One compact canvas dimension. It commits a canvas resize when it loses focus
/// (Enter or a click away) with a changed, valid value; Escape restores the
/// document value. Unfocused fields always show the published document size.
#[allow(clippy::too_many_arguments)]
fn canvas_field(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    axis: usize,
    rect: egui::Rect,
    clip: egui::Rect,
    document: Option<[f64; 2]>,
    enabled: bool,
) {
    let rect = rect.intersect(clip);
    if !rect.is_positive() {
        return;
    }
    let id = ui.scope_id().with(("canvas-dimension", axis));
    let focused = ui.memory(|memory| memory.has_focus(id));
    if !focused && let Some(document) = document {
        view.canvas_text[axis] = format!("{}", document[axis].round());
    }
    if focused {
        ui.painter().rect(
            rect,
            tokens.number("r-sm"),
            tokens.color("surface-field"),
            Stroke::new(1., tokens.color("theme-accent")),
            StrokeKind::Inside,
        );
    }
    let label = if axis == 0 {
        copy::CANVAS_WIDTH
    } else {
        copy::CANVAS_HEIGHT
    };
    let response = ui
        .put(
            rect.shrink2(vec2(tokens.number("s-3"), 0.)),
            egui::TextEdit::singleline(&mut view.canvas_text[axis])
                .id(id)
                .frame(egui::Frame::NONE)
                .font(FontId::proportional(tokens.number("text-sm")))
                .align(egui::Align2::LEFT_CENTER)
                .interactive(enabled),
        )
        .on_hover_text(label);
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::text_edit(enabled, "", &view.canvas_text[axis], "");
        info.label = Some(label.into());
        info
    });
    if !response.lost_focus() {
        return;
    }
    let Some(document) = document else {
        return;
    };
    let cancelled = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let value = view.canvas_text[axis]
        .trim()
        .parse::<f64>()
        .ok()
        .map(f64::round);
    view.canvas_text[axis] = format!("{}", document[axis].round());
    let Some(value) = value.filter(|value| !cancelled && (1. ..=16_384.).contains(value)) else {
        return;
    };
    if enabled && value != document[axis].round() {
        let mut size = document;
        size[axis] = value;
        view.submit(
            tx,
            Request::ResizeCanvas {
                width: size[0],
                height: size[1],
            },
        );
    }
}

/// Shipping `.screenshot-canvas-recenter`: a fixed-glass pill at the top of
/// the viewport while free pan leaves the canvas mostly off screen.
pub(super) fn recenter(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    viewport: egui::Rect,
    canvas: egui::Rect,
) {
    let rect = |r: egui::Rect| Rect {
        x: r.left().into(),
        y: r.top().into(),
        width: r.width().into(),
        height: r.height().into(),
    };
    if !model::canvas_mostly_offscreen(rect(viewport), rect(canvas)) {
        return;
    }
    let text = galley(
        ui,
        copy::RECENTER,
        font(tokens, "text-sm", true),
        tokens.color("glass-text"),
    );
    let size = vec2(
        text.size().x + 2. * tokens.number("s-5"),
        tokens.number("h-sm"),
    );
    let pill = egui::Rect::from_min_size(
        pos2(
            viewport.center().x - size.x / 2.,
            viewport.top() + tokens.number("s-5"),
        ),
        size,
    );
    let response = ui.interact(pill, ui.scope_id().with("canvas-recenter"), Sense::click());
    ui.painter().rect(
        pill,
        tokens.number("r-pill"),
        tokens.color("glass-strong"),
        Stroke::new(1., tokens.color("glass-border")),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        pill.center() - text.size() / 2.,
        text,
        tokens.color("glass-text"),
    );
    accessible(&response, egui::WidgetType::Button, false, copy::RECENTER);
    if response.clicked() {
        view.cancel_edit_gestures();
        view.viewport.recenter();
        view.viewport_pan = None;
    }
}

/// Shipping `.screenshot-tool-rail`: 38 px neutral icon buttons with an accent
/// active state, glass hover tips and the grouped Shapes flyout.
pub(super) fn show_tool_rail(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    let button = model::RAIL_BUTTON as f32;
    let width = model::RAIL_WIDTH as f32;
    egui::Panel::left("editor-tool-rail")
        .resizable(false)
        .exact_size(width)
        .show_separator_line(false)
        .frame(
            egui::Frame::NONE
                .fill(tokens.color("surface-raised"))
                .inner_margin(egui::Margin {
                    left: ((width - button) / 2.) as i8,
                    right: ((width - button) / 2.) as i8,
                    top: tokens.number("s-4") as i8,
                    bottom: tokens.number("s-4") as i8,
                }),
        )
        .show(ui, |ui| {
            let panel = ui.max_rect().expand2(vec2((width - button) / 2., 0.));
            ui.painter().vline(
                panel.right() - 0.5,
                ui.clip_rect().y_range(),
                Stroke::new(1., tokens.color("border-subtle")),
            );
            ui.spacing_mut().item_spacing.y = tokens.number("s-1");
            let enabled =
                edit_enabled(view) && view.import_picker.is_none() && view.folder_picker.is_none();
            let mut tip = None;
            for tool in &model::RAIL_TOOLS {
                let (section, shape) = match tool.key {
                    "v" => (Section::Layers, None),
                    "c" => (Section::Geometry, None),
                    "t" => (Section::Draw, Some(DrawShape::Text)),
                    model::SHAPES_KEY => (Section::Draw, Some(view.last_grouped_shape)),
                    "a" => (Section::Draw, Some(DrawShape::Arrow)),
                    "p" => (Section::Draw, Some(DrawShape::Freehand)),
                    _ => (Section::Draw, Some(view.last_background_tool)),
                };
                let active = view.section == section
                    && match tool.key {
                        "c" => view.crop_previous.is_some(),
                        model::SHAPES_KEY => view.draw_shape.is_grouped(),
                        "b" => matches!(
                            view.draw_shape,
                            DrawShape::Wand | DrawShape::Erase | DrawShape::Restore
                        ),
                        _ => shape.is_none_or(|shape| view.draw_shape == shape),
                    };
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(button), Sense::click());
                let response = if enabled {
                    response
                } else {
                    ui.interact(rect, response.id, Sense::hover())
                };
                let name = if tool.key == model::SHAPES_KEY {
                    model::shapes_tooltip(shape_key(if view.draw_shape.is_grouped() {
                        view.draw_shape
                    } else {
                        view.last_grouped_shape
                    }))
                } else {
                    tool.name()
                };
                accessible(&response, egui::WidgetType::Button, active, &name);
                let hovered = enabled && response.hovered();
                let radius = tokens.number("r-lg");
                if active {
                    ui.painter()
                        .rect_filled(rect, radius, tokens.color("theme-accent"));
                } else if hovered {
                    ui.painter()
                        .rect_filled(rect, radius, tokens.color("surface-hover"));
                }
                let ink = tokens.color(if active {
                    "theme-accent-ink"
                } else if hovered {
                    "text"
                } else {
                    "text-muted"
                });
                let ink = dim(ink, enabled);
                icon(ui.painter(), tool.icon, rect.center(), 18., 1.75, ink);
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.expand(2.),
                        radius + 2.,
                        Stroke::new(2., tokens.color("theme-accent")),
                        StrokeKind::Outside,
                    );
                }
                let mut flyout_open = false;
                if tool.key == model::SHAPES_KEY {
                    // Corner cue for the grouped flyout.
                    let corner = rect.right_bottom() - vec2(5., 5.);
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![corner, corner - vec2(7., 0.), corner - vec2(0., 7.)],
                        ink.gamma_multiply(0.72),
                        Stroke::NONE,
                    ));
                    flyout_open = shapes_flyout(ui, tokens, view, &response);
                }
                if (hovered || response.has_focus()) && !flyout_open {
                    tip = Some((rect, tool.label));
                }
                if response.clicked() {
                    view.activate_tool(section, shape);
                }
            }
            if let Some((rect, label)) = tip {
                rail_tip(ui, tokens, rect, label);
            }
        });
}

fn shape_key(shape: DrawShape) -> &'static str {
    match shape {
        DrawShape::Ellipse => "ellipse",
        DrawShape::Line => "line",
        DrawShape::Triangle => "triangle",
        DrawShape::Diamond => "diamond",
        DrawShape::Star => "star",
        _ => "rectangle",
    }
}

fn shape_for_key(key: &str) -> DrawShape {
    match key {
        "ellipse" => DrawShape::Ellipse,
        "line" => DrawShape::Line,
        "triangle" => DrawShape::Triangle,
        "diamond" => DrawShape::Diamond,
        "star" => DrawShape::Star,
        _ => DrawShape::Rectangle,
    }
}

/// Shipping `.screenshot-tool-flyout`: a three-column grid of 44 px shape
/// buttons beside the rail. Returns whether it is open.
fn shapes_flyout(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    anchor: &egui::Response,
) -> bool {
    let current = if view.draw_shape.is_grouped() {
        view.draw_shape
    } else {
        view.last_grouped_shape
    };
    let side = model::SHAPE_FLYOUT_BUTTON as f32;
    let gap = tokens.number("s-2");
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::RIGHT)
        .gap(10.)
        .frame(
            egui::Frame::NONE
                .fill(tokens.color("surface-overlay"))
                .stroke(Stroke::new(1., tokens.color("border-subtle")))
                .corner_radius(tokens.number("r-lg"))
                .inner_margin(tokens.number("s-3"))
                .shadow(ui.visuals().popup_shadow),
        )
        .show(|ui| {
            egui::Grid::new("shape-flyout")
                .spacing(Vec2::splat(gap))
                .show(ui, |ui| {
                    for (index, item) in model::SHAPE_TOOLS.iter().enumerate() {
                        let shape = shape_for_key(item.key);
                        let active = current == shape;
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(side), Sense::click());
                        let name = item.name();
                        accessible(&response, egui::WidgetType::RadioButton, active, &name);
                        let hovered = response.hovered();
                        let radius = tokens.number("r-md");
                        if active {
                            ui.painter()
                                .rect_filled(rect, radius, tokens.color("theme-accent"));
                        } else if hovered {
                            ui.painter()
                                .rect_filled(rect, radius, tokens.color("surface-hover"));
                        }
                        let ink = tokens.color(if active {
                            "theme-accent-ink"
                        } else if hovered {
                            "text"
                        } else {
                            "text-muted"
                        });
                        icon(ui.painter(), item.icon, rect.center(), 22., 1.75, ink);
                        if response.on_hover_text(&name).clicked() {
                            view.activate_tool(Section::Draw, Some(shape));
                            ui.close();
                        }
                        if (index + 1) % model::SHAPE_FLYOUT_COLUMNS == 0 {
                            ui.end_row();
                        }
                    }
                });
        })
        .is_some()
}

/// Shipping rail hover tip: fixed glass, 10 px right of the button, centred.
fn rail_tip(ui: &egui::Ui, tokens: &Tokens, anchor: egui::Rect, label: &str) {
    let text = galley(
        ui,
        label,
        font(tokens, "text-xs", false),
        tokens.color("glass-text"),
    );
    let size = text.size() + vec2(2. * tokens.number("s-4"), 10.);
    let rect = egui::Rect::from_min_size(
        pos2(anchor.right() + 10., anchor.center().y - size.y / 2.),
        size,
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("editor-rail-tip"),
    ));
    painter.rect(
        rect,
        tokens.number("r-sm"),
        tokens.color("glass-strong"),
        Stroke::new(1., tokens.color("glass-border")),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.center() - text.size() / 2.,
        text,
        tokens.color("glass-text"),
    );
}

/// Shipping draw-tool key for [`model::tool_label`].
fn draw_key(shape: DrawShape) -> &'static str {
    match shape {
        DrawShape::Text => "t",
        DrawShape::Arrow => "a",
        DrawShape::Freehand => "pen",
        DrawShape::Wand => "wand",
        DrawShape::Erase => "erase",
        DrawShape::Restore => "restore",
        shape => shape_key(shape),
    }
}

/// The inspector section heading: shipping's Layers heading (title, count pill
/// and add-image button) for Select, and the tool name like
/// `.screenshot-properties-heading` for Crop and drawing tools. It keeps the
/// height of the former section headings so controls below stay in place.
/// For Layers it returns the native Combine menu button, beside Add image.
pub(super) fn section_heading(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
) -> Option<egui::Response> {
    let height = 30.;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().hline(
        rect.expand2(vec2(8., 0.)).x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1., tokens.color("border-subtle")),
    );
    let title = match view.section {
        Section::Layers => model::layers::TITLE,
        Section::Geometry => model::tool_label("c"),
        Section::Draw => model::tool_label(draw_key(view.draw_shape)),
    };
    let text = galley(
        ui,
        title,
        font(tokens, "text-md", true),
        tokens.color("text"),
    );
    let text_right = rect.left() + text.size().x;
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - text.size().y / 2.),
        text,
        tokens.color("text"),
    );
    if view.section != Section::Layers {
        return None;
    }
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
        pos2(text_right + tokens.number("s-3"), rect.center().y - 9.5),
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
    let add = egui::Rect::from_center_size(
        pos2(rect.right() - 15., rect.center().y - 1.),
        Vec2::splat(28.),
    );
    let enabled = edit_enabled(view) && view.import_picker.is_none();
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
    // Native document-wide Combine actions: a quiet menu button beside Add.
    let combine = add.translate(vec2(-30., 0.));
    let response = ui
        .interact(
            combine,
            ui.scope_id().with("combine-layers"),
            Sense::click(),
        )
        .on_hover_text("Combine layers");
    if response.hovered()
        || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response))
    {
        ui.painter().rect_filled(
            combine,
            tokens.number("r-md"),
            tokens.color("surface-hover"),
        );
    }
    let ink = tokens.color(if response.hovered() {
        "text"
    } else {
        "text-muted"
    });
    icon(
        ui.painter(),
        "merge-visible",
        combine.center(),
        16.,
        1.8,
        ink,
    );
    accessible(&response, egui::WidgetType::Button, false, "Combine layers");
    Some(response)
}

/// What a layer row asked for this frame.
pub(super) struct LayerRow {
    /// The row body: click selects, secondary click opens the context menu.
    pub body: egui::Response,
    pub visibility: bool,
    pub lock: bool,
}

/// Shipping `.screenshot-layer-list li`: a kind icon, the shipping layer name,
/// the muted kind label and eye/lock quick actions. Hidden layers fade; the
/// active row takes the selected surface and an accent rule.
pub(super) fn layer_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    element: &Element,
    selected: bool,
    enabled: bool,
) -> LayerRow {
    let (rect, body) = ui.allocate_exact_size(vec2(ui.available_width(), 30.), Sense::click());
    let base = element.base();
    let name = model::layer_name(element);
    let kind = model::layer_kind(element);
    let radius = tokens.number("r-lg");
    if selected {
        ui.painter().rect(
            rect,
            radius,
            tokens.color("surface-selected"),
            Stroke::new(1., tokens.color("theme-accent").gamma_multiply(0.4)),
            StrokeKind::Inside,
        );
    } else if body.hovered() {
        ui.painter()
            .rect_filled(rect, radius, tokens.color("surface-hover"));
    }
    let faded = |color: Color32| {
        if base.visible {
            color
        } else {
            color.gamma_multiply(0.42)
        }
    };
    let icon_center = pos2(rect.left() + 14., rect.center().y);
    icon(
        ui.painter(),
        model::layer_icon(element),
        icon_center,
        16.,
        1.7,
        faded(tokens.color("text-muted")),
    );
    let action = 22.;
    let actions_left = rect.right() - 2. * action - 4.;
    let kind_text = galley(
        ui,
        kind,
        FontId::proportional(tokens.number("text-xs")),
        Color32::PLACEHOLDER,
    );
    let name_font = FontId::proportional(tokens.number("text-sm"));
    let name_left = icon_center.x + 14.;
    let name_width = galley(ui, &name, name_font.clone(), Color32::PLACEHOLDER)
        .size()
        .x;
    // The name has priority in the 30 px row; the kind shows when both fit.
    let kind_left = actions_left - tokens.number("s-3") - kind_text.size().x;
    if name_left + name_width + tokens.number("s-3") <= kind_left {
        ui.painter().galley(
            pos2(kind_left, rect.center().y - kind_text.size().y / 2.),
            kind_text,
            faded(tokens.color("text-subtle")),
        );
    }
    let name_rect = egui::Rect::from_min_max(
        pos2(name_left, rect.top()),
        pos2(actions_left - tokens.number("s-3"), rect.bottom()),
    );
    let mut job = egui::text::LayoutJob::single_section(
        name.clone(),
        egui::TextFormat::simple(name_font, faded(tokens.color("text"))),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: name_rect.width().max(0.),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let name_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            name_rect.left(),
            rect.center().y - name_galley.size().y / 2.,
        ),
        name_galley,
        faded(tokens.color("text")),
    );
    body.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            enabled,
            selected,
            format!("{name}, {kind}"),
        )
    });
    let body = body.on_hover_text(&name);
    let quick = |index: usize, on: bool, icon_name: &str, label: String, tooltip: &str| {
        let area = egui::Rect::from_center_size(
            pos2(
                actions_left + action * (index as f32 + 0.5),
                rect.center().y,
            ),
            vec2(action, 26.),
        );
        let response = ui.interact(
            area,
            ui.scope_id().with((base.id.as_str(), icon_name)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let hovered = enabled && response.hovered();
        let dark = ui.visuals().dark_mode;
        if on {
            ui.painter().rect_filled(
                area,
                tokens.number("r-sm"),
                tokens.color("surface-selected"),
            );
        } else if hovered {
            ui.painter()
                .rect_filled(area, tokens.number("r-sm"), tokens.color("surface-active"));
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
            if selected || body.hovered() {
                color
            } else {
                color.gamma_multiply(0.5)
            }
        };
        icon(
            ui.painter(),
            icon_name,
            area.center(),
            14.,
            1.8,
            dim(ink, enabled),
        );
        accessible(&response, egui::WidgetType::Button, on, &label);
        response.on_hover_text(tooltip).clicked()
    };
    let visibility = quick(
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
    let lock = quick(
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
    LayerRow {
        body,
        visibility,
        lock,
    }
}
