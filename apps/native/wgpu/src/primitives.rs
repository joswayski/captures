//! Shared shipping UI primitives drawn from design tokens, used by every
//! wgpu surface: the focus ring (`--focus-ring-tight`) and scroll bars
//! (`styles/base.css`).

use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind};

use crate::tokens::Tokens;

/// `--focus-ring-tight`: a 2 px translucent accent ring just outside `rect`.
fn paint_ring(painter: &egui::Painter, accent: Color32, rect: Rect, radius: f32) {
    painter.rect_stroke(
        rect.expand(1.),
        radius + 1.,
        Stroke::new(2., accent.gamma_multiply(0.45)),
        StrokeKind::Outside,
    );
}

fn indicated_key(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("captures-focus-indicated").with(ctx.viewport_id())
}

/// Record that the focused widget drew its own focus indicator this pass,
/// so the global ring (see [`install_focus_ring`]) does not repeat it.
pub fn focus_indicated(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    let key = indicated_key(ctx);
    ctx.data_mut(|data| data.insert_temp(key, pass));
}

/// The token focus ring around a custom control's `rect`.
pub fn focus_ring(ui: &egui::Ui, t: &Tokens, rect: Rect, radius: f32) {
    paint_ring(ui.painter(), t.color("theme-accent"), rect, radius);
    focus_indicated(ui.ctx());
}

/// Draw the token focus ring around whichever stock egui control (button,
/// ComboBox, DragValue, text edit, checkbox…) holds keyboard focus at the end
/// of every pass, in every viewport. Custom controls that paint their own
/// indicator call [`focus_ring`] or [`focus_indicated`] instead.
pub fn install_focus_ring(ctx: &egui::Context) {
    ctx.on_end_pass(
        "captures-focus-ring",
        Arc::new(|ui: &mut egui::Ui| {
            let ctx = ui.ctx().clone();
            let Some(id) = ctx.memory(|memory| memory.focused()) else {
                return;
            };
            let key = indicated_key(&ctx);
            let pass = ctx.cumulative_pass_nr();
            if ctx.data(|data| data.get_temp::<u64>(key)) == Some(pass) {
                return;
            }
            let Some(response) = ctx.read_response(id) else {
                return;
            };
            if !response.rect.is_positive() || !response.interact_rect.is_positive() {
                return;
            }
            let style = ctx.global_style();
            let radius = f32::from(style.visuals.widgets.inactive.corner_radius.nw);
            let painter = ctx
                .layer_painter(response.layer_id)
                .with_clip_rect(response.interact_rect.expand(4.));
            paint_ring(
                &painter,
                style.visuals.selection.stroke.color,
                response.rect,
                radius,
            );
        }),
    );
}

/// Shipping scroll bars: a thin pill thumb over a transparent track. The
/// 10 px bar keeps a 3 px transparent border, leaving a 4 px thumb; bars
/// overlay content so they do not reflow any layout.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        bar_width: 4.,
        floating_width: 4.,
        floating_allocated_width: 0.,
        bar_inner_margin: 3.,
        bar_outer_margin: 3.,
        handle_min_length: 20.,
        foreground_color: false,
        dormant_background_opacity: 0.,
        active_background_opacity: 0.,
        interact_background_opacity: 0.,
        dormant_handle_opacity: 1.,
        active_handle_opacity: 1.,
        interact_handle_opacity: 1.,
        ..egui::style::ScrollStyle::solid()
    }
}

/// egui paints a scroll thumb with the `bg_fill` of the surrounding Ui's
/// widget visuals: `--border-strong`, lifting to `--text-faint` on hover
/// (the glass palette's equivalents over media).
fn scrollbar_visuals(widgets: &mut egui::style::Widgets, t: &Tokens, glass: bool) {
    let (idle, hover) = if glass {
        ("glass-border-strong", "glass-text-subtle")
    } else {
        ("border-strong", "text-faint")
    };
    widgets.inactive.bg_fill = t.color(idle);
    widgets.hovered.bg_fill = t.color(hover);
    widgets.active.bg_fill = t.color(hover);
    let pill = (t.number("r-pill").min(255.) as u8).into();
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        widget.corner_radius = pill;
    }
}

fn scrolled<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    glass: bool,
    show: impl FnOnce(&mut egui::Ui, &egui::style::Widgets) -> R,
) -> R {
    let content = ui.visuals().widgets.clone();
    scrollbar_visuals(&mut ui.visuals_mut().widgets, t, glass);
    let result = show(ui, &content);
    ui.visuals_mut().widgets = content;
    result
}

/// Show `area` with shipping scroll bars; `add` sees the surrounding visuals.
pub fn scroll_area<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show(ui, |ui| {
            ui.visuals_mut().widgets = content.clone();
            add(ui)
        })
    })
}

/// [`scroll_area`] over the glass media palette.
pub fn glass_scroll_area<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, true, |ui, content| {
        area.show(ui, |ui| {
            ui.visuals_mut().widgets = content.clone();
            add(ui)
        })
    })
}

/// [`scroll_area`] for `ScrollArea::show_viewport`.
pub fn scroll_viewport<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui, Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show_viewport(ui, |ui, viewport| {
            ui.visuals_mut().widgets = content.clone();
            add(ui, viewport)
        })
    })
}

/// [`scroll_area`] for `ScrollArea::show_rows`.
pub fn scroll_rows<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    row_height: f32,
    rows: usize,
    add: impl FnOnce(&mut egui::Ui, std::ops::Range<usize>) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show_rows(ui, row_height, rows, |ui, range| {
            ui.visuals_mut().widgets = content.clone();
            add(ui, range)
        })
    })
}

/// `--shadow-lg` for the current appearance.
fn shadow_lg(dark: bool) -> egui::Shadow {
    if dark {
        egui::Shadow {
            offset: [0, 18],
            blur: 44,
            spread: 0,
            color: Color32::from_black_alpha(117),
        }
    } else {
        egui::Shadow {
            offset: [0, 16],
            blur: 40,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(19, 19, 24, 31),
        }
    }
}

/// The shipping 16-unit chevron `m4 6 4 4 4-4`, flipped while open.
fn chevron(painter: &egui::Painter, center: egui::Pos2, side: f32, open: bool, color: Color32) {
    let glyph = Rect::from_center_size(center, egui::Vec2::splat(side));
    let s = glyph.width() / 16.;
    let p = |x: f32, y: f32| {
        let y = if open { 16. - y } else { y };
        glyph.min + egui::vec2(x * s, y * s)
    };
    painter.add(egui::Shape::line(
        vec![p(4., 6.), p(8., 10.), p(12., 6.)],
        Stroke::new(1.7 * s, color),
    ));
}

/// One option of a shipping `CustomSelect`.
pub struct SelectOption<'a, T> {
    pub value: T,
    pub label: &'a str,
    /// Secondary copy under the label in the listbox (`<small>`).
    pub description: &'a str,
    pub disabled: bool,
}

impl<'a, T> SelectOption<'a, T> {
    pub fn new(value: T, label: &'a str) -> Self {
        Self {
            value,
            label,
            description: "",
            disabled: false,
        }
    }
    pub fn description(mut self, description: &'a str) -> Self {
        self.description = description;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// How a select's trigger is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectStyle {
    /// `.custom-select-trigger`: a field with the selected label and a chevron.
    Field,
    /// The capture menu's media-palette trigger and `.custom-select-listbox-glass`.
    Glass,
    /// `.filename-format-select`: borderless inside another field.
    Inline,
}

/// The trigger response, whether the listbox is open, a newly chosen value,
/// and each listed option's row (in option order) while it is open.
pub struct SelectOutput<T> {
    pub response: egui::Response,
    /// The listbox is open (shipping `onOpen` fires as it opens).
    pub open: bool,
    pub chosen: Option<T>,
    pub rows: Vec<Rect>,
}

#[derive(Clone, Copy, Default)]
struct SelectMemory {
    state: captures_app::controls::select::State,
    menu: egui::Vec2,
}

/// Shipping `CustomSelect`: a field-style trigger and a listbox with option
/// descriptions. The focused trigger (or an open listbox) takes ArrowUp/Down,
/// Home/End, Enter/Space and Escape through `captures_app::controls::select`.
pub struct Select<'a> {
    id_salt: egui::IdSalt,
    label: &'a str,
    width: f32,
    height: Option<f32>,
    style: SelectStyle,
    trigger_text: Option<&'a str>,
}

impl<'a> Select<'a> {
    /// `label` is the accessible name (shipping `ariaLabel`).
    pub fn new(
        id_salt: impl std::hash::Hash + std::fmt::Debug,
        label: &'a str,
        width: f32,
    ) -> Self {
        Self {
            id_salt: egui::IdSalt::new(id_salt),
            label,
            width,
            height: None,
            style: SelectStyle::Field,
            trigger_text: None,
        }
    }
    pub fn style(mut self, style: SelectStyle) -> Self {
        self.style = style;
        self
    }
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }
    /// Text shown in the trigger instead of the selected label (`triggerLabel`).
    pub fn trigger_text(mut self, text: &'a str) -> Self {
        self.trigger_text = Some(text);
        self
    }

    pub fn show<T: PartialEq + Clone>(
        self,
        ui: &mut egui::Ui,
        t: &Tokens,
        options: &[SelectOption<'_, T>],
        selected: &T,
    ) -> SelectOutput<T> {
        use captures_app::controls::select;
        let glass = self.style == SelectStyle::Glass;
        let id = ui.make_persistent_id(self.id_salt);
        let memory_id = id.with("select");
        let mut memory: SelectMemory = ui.data(|data| data.get_temp(memory_id)).unwrap_or_default();
        let height = self.height.unwrap_or_else(|| match self.style {
            SelectStyle::Field => t.number("h-md"),
            SelectStyle::Glass => t.number("h-lg"),
            // `height: 100%` of the field it sits in.
            SelectStyle::Inline => ui.max_rect().height(),
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(self.width, height), egui::Sense::hover());
        // The trigger owns the select's id, so focus and the listbox agree.
        let response = ui.interact(rect, id, egui::Sense::click());
        let enabled = ui.is_enabled();
        let disabled: Vec<bool> = options.iter().map(|option| option.disabled).collect();
        let selected_index = options
            .iter()
            .position(|option| option.value == *selected)
            .unwrap_or(0);
        let selected_label = options
            .get(selected_index)
            .map_or("", |option| option.label);
        response.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, self.label);
            info.current_text_value = Some(selected_label.to_owned());
            info
        });
        let focused = response.has_focus();
        let mut state = memory.state;
        let mut chosen = None;
        if !enabled {
            state.open = false;
        }
        // Keyboard: the focused trigger, or any open listbox (a pointer opens
        // without taking focus, like WebKit buttons).
        if enabled && (focused || state.open) {
            if focused {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            vertical_arrows: true,
                            escape: state.open,
                            ..Default::default()
                        },
                    );
                });
            }
            for (key, logical) in [
                (select::Key::ArrowDown, egui::Key::ArrowDown),
                (select::Key::ArrowUp, egui::Key::ArrowUp),
                (select::Key::Home, egui::Key::Home),
                (select::Key::End, egui::Key::End),
                (select::Key::Enter, egui::Key::Enter),
                (select::Key::Space, egui::Key::Space),
                (select::Key::Escape, egui::Key::Escape),
            ] {
                if !ui.input(|input| input.key_pressed(logical)) {
                    continue;
                }
                let outcome = select::key(state, &disabled, selected_index, key);
                if outcome.handled {
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, logical));
                    state = outcome.state;
                    if let Some(index) = outcome.chosen {
                        chosen = Some(index);
                    }
                }
            }
        }
        let keyboard_click = focused
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            });
        if enabled && response.clicked() && !keyboard_click {
            state = if state.open {
                select::State {
                    open: false,
                    ..state
                }
            } else {
                select::open(&disabled, selected_index)
            };
        }
        // Tabbing to another control closes the listbox.
        if state.open
            && ui
                .memory(|memory| memory.focused())
                .is_some_and(|other| other != id)
        {
            state.open = false;
        }

        // Trigger.
        let open = state.open;
        let hovered = enabled && response.hovered();
        let radius = t.number("r-md");
        let painter = ui.painter();
        let (fill, border, ink, glyph) = match self.style {
            SelectStyle::Field => (
                t.color("surface-field"),
                t.color(if focused || open {
                    "theme-accent"
                } else if hovered {
                    "border-strong"
                } else {
                    "control-border"
                }),
                t.color("text"),
                t.color("text-subtle"),
            ),
            SelectStyle::Glass => (
                Color32::from_black_alpha(77),
                t.color(if focused || open {
                    "theme-accent"
                } else if hovered {
                    "glass-border-strong"
                } else {
                    "glass-border"
                }),
                t.color("glass-text"),
                t.color("glass-text-subtle"),
            ),
            SelectStyle::Inline => (
                if hovered {
                    t.color("surface-hover")
                } else {
                    Color32::TRANSPARENT
                },
                Color32::TRANSPARENT,
                t.color(if hovered || focused || open {
                    "text"
                } else {
                    "text-subtle"
                }),
                t.color(if hovered || focused || open {
                    "text"
                } else {
                    "text-subtle"
                }),
            ),
        };
        // The inline trigger rounds only the field's right end.
        let corners = if self.style == SelectStyle::Inline {
            let r = radius.min(255.) as u8;
            egui::CornerRadius {
                nw: 0,
                sw: 0,
                ne: r,
                se: r,
            }
        } else {
            egui::CornerRadius::from(radius)
        };
        painter.rect(
            rect,
            corners,
            fill,
            Stroke::new(1., border),
            StrokeKind::Inside,
        );
        let padding = if self.style == SelectStyle::Inline {
            t.number("s-3")
        } else {
            t.number("s-4")
        };
        let text = self.trigger_text.unwrap_or(selected_label);
        let font = egui::FontId::proportional(t.number("text-sm"));
        let text_width = (rect.width() - padding * 2. - 14. - t.number("s-3")).max(0.);
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), ink);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let text_left = if self.style == SelectStyle::Inline {
            rect.left() + t.number("s-4")
        } else {
            rect.left() + padding
        };
        painter.galley(
            egui::pos2(text_left, rect.center().y - galley.size().y / 2.),
            galley,
            ink,
        );
        chevron(
            painter,
            egui::pos2(rect.right() - padding - 7., rect.center().y),
            14.,
            open,
            glyph,
        );
        if (focused || open) && self.style != SelectStyle::Inline {
            focus_ring(ui, t, rect, radius);
        } else if focused {
            focus_indicated(ui.ctx());
        }

        // Listbox.
        let mut rows = Vec::new();
        if open {
            let pad = t.number("s-2");
            let row_x = t.number("s-4");
            let row_y = t.number("s-3");
            let check_width = 12.;
            let gap = t.number("s-6");
            let label_font = egui::FontId::proportional(t.number("text-sm"));
            let small_font = egui::FontId::proportional(t.number("text-xs"));
            let natural = options
                .iter()
                .map(|option| {
                    let label = ui.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(option.label.to_owned(), label_font.clone(), ink)
                            .size()
                            .x
                    });
                    let small = if option.description.is_empty() {
                        0.
                    } else {
                        ui.fonts_mut(|fonts| {
                            fonts
                                .layout_no_wrap(
                                    option.description.to_owned(),
                                    small_font.clone(),
                                    ink,
                                )
                                .size()
                                .x
                        })
                    };
                    label.max(small)
                })
                .fold(0., f32::max)
                + row_x * 2.
                + gap
                + check_width
                + pad * 2.
                + 2.;
            let viewport = ui.ctx().content_rect();
            let layout = select::layout(
                select::Rect {
                    left: f64::from(rect.left() - viewport.left()),
                    top: f64::from(rect.top() - viewport.top()),
                    width: f64::from(rect.width()),
                    height: f64::from(rect.height()),
                },
                f64::from(natural),
                f64::from(memory.menu.y),
                f64::from(viewport.width()),
                f64::from(viewport.height()),
                options.len(),
            );
            let menu_width = layout.width as f32;
            let origin = viewport.min + egui::vec2(layout.left as f32, layout.top as f32);
            let dark = ui.visuals().dark_mode;
            let area = egui::Area::new(id.with("listbox"))
                .order(egui::Order::Foreground)
                .fixed_pos(origin)
                .constrain(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(t.color(if glass {
                            "glass-raised"
                        } else {
                            "surface-overlay"
                        }))
                        .stroke(Stroke::new(
                            1.,
                            t.color(if glass { "glass-border" } else { "border" }),
                        ))
                        .corner_radius(t.number("r-lg"))
                        .shadow(shadow_lg(dark))
                        .inner_margin(pad)
                        .show(ui, |ui| {
                            let inner = menu_width - pad * 2. - 2.;
                            ui.set_width(inner);
                            let area = egui::ScrollArea::vertical()
                                .id_salt(id.with("listbox-scroll"))
                                .max_height((layout.max_height as f32 - pad * 2. - 2.).max(1.));
                            scrolled(ui, t, glass, |ui, content| {
                                area.show(ui, |ui: &mut egui::Ui| {
                                    ui.visuals_mut().widgets = content.clone();
                                    ui.spacing_mut().item_spacing.y = 0.;
                                    for (index, option) in options.iter().enumerate() {
                                        let is_selected = index == selected_index
                                            && options[index].value == *selected;
                                        let copy_width = inner - row_x * 2. - gap - check_width;
                                        let label = ui.fonts_mut(|fonts| {
                                            fonts.layout(
                                                option.label.to_owned(),
                                                label_font.clone(),
                                                Color32::PLACEHOLDER,
                                                copy_width,
                                            )
                                        });
                                        let small = (!option.description.is_empty()).then(|| {
                                            ui.fonts_mut(|fonts| {
                                                fonts.layout(
                                                    option.description.to_owned(),
                                                    small_font.clone(),
                                                    Color32::PLACEHOLDER,
                                                    copy_width,
                                                )
                                            })
                                        });
                                        let copy_height = label.size().y
                                            + small
                                                .as_ref()
                                                .map_or(0., |small| small.size().y + 2.);
                                        let row_height =
                                            (copy_height + row_y * 2.).max(t.number("h-sm"));
                                        let (row, row_response) = ui.allocate_exact_size(
                                            egui::vec2(inner, row_height),
                                            if option.disabled {
                                                egui::Sense::hover()
                                            } else {
                                                egui::Sense::click()
                                            },
                                        );
                                        row_response.widget_info(|| {
                                            egui::WidgetInfo::selected(
                                                egui::WidgetType::SelectableLabel,
                                                !option.disabled,
                                                is_selected,
                                                option.label,
                                            )
                                        });
                                        if !option.disabled && row_response.hovered() {
                                            state.active = index;
                                        }
                                        if row_response.clicked() {
                                            chosen = Some(index);
                                        }
                                        let active = state.active == index && !option.disabled;
                                        if active {
                                            ui.painter().rect_filled(
                                                row,
                                                t.number("r-sm"),
                                                t.color(if glass {
                                                    "glass-hover"
                                                } else {
                                                    "surface-hover"
                                                }),
                                            );
                                        }
                                        let color = t.color(
                                            match (glass, option.disabled, active || is_selected) {
                                                (false, true, _) => "text-faint",
                                                (true, true, _) => "glass-text-subtle",
                                                (false, false, true) => "text",
                                                (true, false, true) => "glass-text",
                                                (false, false, false) => "text-muted",
                                                (true, false, false) => "glass-text-muted",
                                            },
                                        );
                                        let top = row.center().y - copy_height / 2.;
                                        let label_height = label.size().y;
                                        ui.painter().galley(
                                            egui::pos2(row.left() + row_x, top),
                                            label,
                                            color,
                                        );
                                        if let Some(small) = small {
                                            ui.painter().galley(
                                                egui::pos2(
                                                    row.left() + row_x,
                                                    top + label_height + 2.,
                                                ),
                                                small,
                                                t.color(if glass {
                                                    "glass-text-subtle"
                                                } else {
                                                    "text-faint"
                                                }),
                                            );
                                        }
                                        if is_selected {
                                            let check = t.color(if glass {
                                                "glass-text"
                                            } else if dark {
                                                "theme-accent"
                                            } else {
                                                "theme-accent-readable"
                                            });
                                            crate::preferences_widgets::icon(
                                                ui.painter(),
                                                "check",
                                                Rect::from_center_size(
                                                    egui::pos2(
                                                        row.right() - row_x - 6.,
                                                        row.center().y,
                                                    ),
                                                    egui::Vec2::splat(12.),
                                                ),
                                                check,
                                            );
                                        }
                                        rows.push(row);
                                    }
                                });
                            });
                        });
                });
            memory.menu = area.response.rect.size();
            // A press outside the trigger and the listbox closes it.
            let outside = ui.input(|input| {
                input.pointer.any_pressed()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| !rect.contains(pos) && !area.response.rect.contains(pos))
            });
            if outside {
                state.open = false;
            }
            ui.ctx().request_repaint();
        }
        if let Some(index) = chosen {
            state.open = false;
            memory.menu = egui::Vec2::ZERO;
            chosen = options
                .get(index)
                .filter(|option| !option.disabled)
                .map(|_| index);
        }
        if !state.open {
            memory.menu = egui::Vec2::ZERO;
        }
        memory.state = state;
        ui.data_mut(|data| data.insert_temp(memory_id, memory));
        SelectOutput {
            response,
            open: state.open,
            chosen: chosen.and_then(|index| {
                let value = options[index].value.clone();
                (value != *selected).then_some(value)
            }),
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (egui::Context, Tokens) {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        (ctx, tokens)
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut add: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 400.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| add(ui));
            },
        );
        output.textures_delta.clear();
        output
    }

    fn strokes(output: &egui::FullOutput) -> Vec<Stroke> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) => Some(rect.stroke),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn focused_stock_controls_get_the_token_ring_once() {
        let (ctx, t) = setup();
        install_focus_ring(&ctx);
        let ring = Stroke::new(2., t.color("theme-accent").gamma_multiply(0.45));
        let mut button_id = None;
        for _ in 0..2 {
            frame(&ctx, vec![], |ui| {
                button_id = Some(ui.button("Send feedback").id);
            });
        }
        let id = button_id.unwrap();
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&ctx, vec![], |ui| {
            let _ = ui.button("Send feedback");
        });
        let output = frame(&ctx, vec![], |ui| {
            let _ = ui.button("Send feedback");
        });
        let rings = strokes(&output).into_iter().filter(|s| *s == ring).count();
        assert_eq!(rings, 1, "the focused stock button gets one token ring");

        // A custom control that draws its own indicator is not ringed twice.
        let custom = egui::Id::unique("custom-focus");
        let draw = |ui: &mut egui::Ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40., 20.), egui::Sense::hover());
            let response = ui.interact(rect, custom, egui::Sense::click());
            if response.has_focus() {
                focus_ring(ui, &t, rect, 4.);
            }
        };
        frame(&ctx, vec![], draw);
        ctx.memory_mut(|memory| memory.request_focus(custom));
        frame(&ctx, vec![], draw);
        let output = frame(&ctx, vec![], draw);
        let rings = strokes(&output).into_iter().filter(|s| *s == ring).count();
        assert_eq!(rings, 1, "a custom indicator suppresses the global ring");
    }

    #[test]
    fn scroll_bars_use_thumb_tokens_but_content_keeps_its_visuals() {
        let (ctx, t) = setup();
        let control = ctx.global_style().visuals.widgets.inactive.bg_fill;
        let mut inside = None;
        let mut after = None;
        frame(&ctx, vec![], |ui| {
            scroll_area(ui, &t, egui::ScrollArea::vertical().max_height(50.), |ui| {
                inside = Some(ui.visuals().widgets.inactive.bg_fill);
                ui.allocate_space(egui::vec2(100., 400.));
            });
            after = Some(ui.visuals().widgets.inactive.bg_fill);
        });
        assert_eq!(inside, Some(control));
        assert_eq!(after, Some(control));
        let style = scroll_style();
        assert!(style.floating);
        assert_eq!(style.bar_width, 4.);
        assert_eq!(style.dormant_background_opacity, 0.);
        assert_eq!(style.dormant_handle_opacity, 1.);
        let mut widgets = ctx.global_style().visuals.widgets.clone();
        scrollbar_visuals(&mut widgets, &t, false);
        assert_eq!(widgets.inactive.bg_fill, t.color("border-strong"));
        assert_eq!(widgets.hovered.bg_fill, t.color("text-faint"));
    }

    #[test]
    fn select_keyboard_opens_moves_skips_disabled_and_chooses() {
        let (ctx, t) = setup();
        let options = [
            SelectOption::new(1, "One"),
            SelectOption::new(2, "Two").disabled(true),
            SelectOption::new(3, "Three").description("The third option"),
        ];
        let mut value = 1;
        let run = |events: Vec<egui::Event>, value: &mut i32| {
            let mut seen = (None, 0);
            frame(&ctx, events, |ui| {
                let output =
                    Select::new("test-select", "Numbers", 160.).show(ui, &t, &options, value);
                seen = (Some(output.response.id), output.rows.len());
                if let Some(chosen) = output.chosen {
                    *value = chosen;
                }
            });
            seen
        };
        let (trigger, _) = run(vec![], &mut value);
        ctx.memory_mut(|memory| memory.request_focus(trigger.unwrap()));
        run(vec![], &mut value);
        run(vec![], &mut value);
        run(vec![key(egui::Key::ArrowDown)], &mut value);
        let (_, rows) = run(vec![], &mut value);
        assert_eq!(rows, 3, "ArrowDown opens the listbox");
        run(vec![key(egui::Key::ArrowDown)], &mut value);
        run(vec![key(egui::Key::Enter)], &mut value);
        assert_eq!(value, 3, "the disabled option is skipped");
        let (_, rows) = run(vec![], &mut value);
        assert_eq!(rows, 0, "choosing closes the listbox");
        run(vec![key(egui::Key::Space)], &mut value);
        run(vec![key(egui::Key::Home)], &mut value);
        run(vec![key(egui::Key::Escape)], &mut value);
        let (_, rows) = run(vec![], &mut value);
        assert_eq!((value, rows), (3, 0), "Escape closes without choosing");
    }

    #[test]
    fn select_pointer_chooses_a_row_and_outside_press_closes() {
        let (ctx, t) = setup();
        let options = [
            SelectOption::new('a', "Alpha"),
            SelectOption::new('b', "Beta"),
        ];
        let mut value = 'a';
        let run = |events: Vec<egui::Event>, value: &mut char| {
            let mut seen = (Rect::NOTHING, Vec::new());
            frame(&ctx, events, |ui| {
                let output =
                    Select::new("pointer-select", "Letters", 160.).show(ui, &t, &options, value);
                seen = (output.response.rect, output.rows.clone());
                if let Some(chosen) = output.chosen {
                    *value = chosen;
                }
            });
            seen
        };
        let (trigger, _) = run(vec![], &mut value);
        let center = trigger.center();
        run(
            vec![egui::Event::PointerMoved(center), press(center, true)],
            &mut value,
        );
        run(vec![press(center, false)], &mut value);
        let (_, rows) = run(vec![], &mut value);
        assert_eq!(rows.len(), 2);
        assert!(
            rows[0].top() > trigger.bottom(),
            "the listbox opens below the trigger"
        );
        let beta = rows[1].center();
        run(
            vec![egui::Event::PointerMoved(beta), press(beta, true)],
            &mut value,
        );
        run(vec![press(beta, false)], &mut value);
        assert_eq!(value, 'b');
        run(
            vec![egui::Event::PointerMoved(center), press(center, true)],
            &mut value,
        );
        run(vec![press(center, false)], &mut value);
        let (_, rows) = run(vec![], &mut value);
        assert_eq!(rows.len(), 2);
        let outside = egui::pos2(580., 380.);
        run(
            vec![egui::Event::PointerMoved(outside), press(outside, true)],
            &mut value,
        );
        run(vec![press(outside, false)], &mut value);
        let (_, rows) = run(vec![], &mut value);
        assert!(rows.is_empty(), "a press outside closes the listbox");
        assert_eq!(value, 'b');
    }
}
