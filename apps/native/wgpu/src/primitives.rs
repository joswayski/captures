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
            let Some(response) = read_response_keeping_focus(&ctx, id) else {
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

/// `Context::read_response` is not a pure read: it replays this pass's
/// pointer presses against the widget and surrenders its focus when the
/// press landed elsewhere. At end of pass that would drop focus a widget
/// requested during the very pass that was clicked (e.g. the canvas text
/// composer opened by a click on the canvas), so keystrokes and Escape
/// would never reach it. Suspend focus surrender for this lookup only.
fn read_response_keeping_focus(ctx: &egui::Context, id: egui::Id) -> Option<egui::Response> {
    let surrender = ctx.options_mut(|options| {
        std::mem::replace(
            &mut options.input_options.surrender_focus_on,
            egui::SurrenderFocusOn::Never,
        )
    });
    let response = ctx.read_response(id);
    ctx.options_mut(|options| options.input_options.surrender_focus_on = surrender);
    response
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
    /// The listbox's visible area while open (the rows' clip rect).
    pub list_clip: Rect,
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
        let mut list_clip = Rect::NOTHING;
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
                                    list_clip = ui.clip_rect();
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
            list_clip,
            chosen: chosen.and_then(|index| {
                let value = options[index].value.clone();
                (value != *selected).then_some(value)
            }),
            rows,
        }
    }
}

/// Format `value` for a number field: integers plainly, decimals at the
/// step's precision.
fn number_text<N: egui::emath::Numeric>(value: N, step: f64) -> String {
    if N::INTEGRAL {
        format!("{}", value.to_f64().round() as i64)
    } else {
        captures_app::controls::number::format_stepped(value.to_f64(), step)
    }
}

/// Shipping `NumberInput`: a mono field with custom Increase/Decrease
/// steppers (hidden while disabled) and ArrowUp/ArrowDown stepping through
/// `captures_app::controls::number`. Typing updates the value as it parses,
/// clamped to the bounds, unless `commit_on_enter` defers it to Enter or
/// focus loss.
pub struct NumberInput<'a> {
    id_salt: egui::IdSalt,
    label: &'a str,
    width: f32,
    min: Option<f64>,
    max: Option<f64>,
    step: f64,
    commit_on_enter: bool,
}

impl<'a> NumberInput<'a> {
    /// `label` names the field and its steppers ("Increase {label}").
    pub fn new(
        id_salt: impl std::hash::Hash + std::fmt::Debug,
        label: &'a str,
        width: f32,
    ) -> Self {
        Self {
            id_salt: egui::IdSalt::new(id_salt),
            label,
            width,
            min: None,
            max: None,
            step: 1.,
            commit_on_enter: false,
        }
    }
    pub fn range(mut self, range: std::ops::RangeInclusive<f64>) -> Self {
        self.min = Some(*range.start());
        self.max = Some(*range.end());
        self
    }
    pub fn commit_on_enter(mut self) -> Self {
        self.commit_on_enter = true;
        self
    }

    fn clamp(&self, value: f64) -> f64 {
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }

    /// Returns the whole field's response, marked changed when `value` changed.
    pub fn show<N: egui::emath::Numeric>(
        self,
        ui: &mut egui::Ui,
        t: &Tokens,
        value: &mut N,
    ) -> egui::Response {
        use captures_app::controls::number;
        let id = ui.make_persistent_id(self.id_salt);
        let text_id = id.with("text");
        let buffer_id = id.with("buffer");
        let enabled = ui.is_enabled();
        let (rect, mut response) = ui.allocate_exact_size(
            egui::vec2(self.width, t.number("h-md")),
            egui::Sense::hover(),
        );
        let focused_before = ui.memory(|memory| memory.has_focus(text_id));
        let mut buffer = if focused_before {
            ui.data(|data| data.get_temp::<String>(buffer_id))
                .unwrap_or_else(|| number_text(*value, self.step))
        } else {
            number_text(*value, self.step)
        };
        let mut changed = false;
        let mut set = |value: &mut N, next: f64| {
            let next = N::from_f64(next);
            if next != *value {
                *value = next;
                changed = true;
            }
        };
        // ArrowUp/ArrowDown step before the text edit sees them.
        if enabled && focused_before {
            for (key, up) in [(egui::Key::ArrowUp, true), (egui::Key::ArrowDown, false)] {
                while ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                    let next = number::step_from(&buffer, self.step, up, self.min, self.max);
                    set(value, next);
                    buffer = number_text(N::from_f64(next), self.step);
                }
            }
        }
        let hovered = enabled && ui.rect_contains_pointer(rect);
        let radius = t.number("r-md");
        ui.painter().rect(
            rect,
            radius,
            t.color("surface-field"),
            Stroke::new(
                1.,
                t.color(if enabled && focused_before {
                    "theme-accent"
                } else if hovered {
                    "border-strong"
                } else {
                    "control-border"
                }),
            ),
            StrokeKind::Inside,
        );
        let steppers = enabled;
        let stepper_width = 26.;
        let text_rect = Rect::from_min_max(
            rect.min,
            egui::pos2(
                rect.right() - if steppers { stepper_width } else { 0. },
                rect.bottom(),
            ),
        );
        let pad_x = t.number("s-4");
        let mut text_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text_rect.shrink2(egui::vec2(pad_x, 1.)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let text = text_ui.add(
            egui::TextEdit::singleline(&mut buffer)
                .id(text_id)
                .frame(egui::Frame::NONE)
                .font(egui::FontId::monospace(t.number("text-sm")))
                .desired_width(text_rect.width() - pad_x - t.number("s-2")),
        );
        text.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, enabled, self.label);
            info.current_text_value = Some(buffer.clone());
            info
        });
        let parsed = buffer.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        if text.changed()
            && !self.commit_on_enter
            && let Some(parsed) = parsed
        {
            set(value, self.clamp(parsed));
        }
        if text.lost_focus()
            && !ui.input(|input| input.key_pressed(egui::Key::Escape))
            && let Some(parsed) = parsed
        {
            set(value, self.clamp(parsed));
        }
        if text.has_focus() {
            ui.data_mut(|data| data.insert_temp(buffer_id, buffer.clone()));
            // Keep vertical arrows for stepping instead of moving focus.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    text_id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            focus_ring(ui, t, rect, radius);
        } else {
            ui.data_mut(|data| data.remove::<String>(buffer_id));
        }

        if steppers {
            let (at_min, at_max) = number::at_bounds(&buffer, self.min, self.max);
            let column = Rect::from_min_max(egui::pos2(text_rect.right(), rect.top()), rect.max);
            let painter = ui.painter();
            painter.rect_filled(
                column.shrink(1.),
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: (radius - 1.).max(0.) as u8,
                    se: (radius - 1.).max(0.) as u8,
                },
                t.color("surface-hover"),
            );
            let divider = Stroke::new(1., t.color("border-subtle"));
            painter.vline(column.left(), column.y_range().shrink(1.), divider);
            painter.hline(column.x_range().shrink(1.), column.center().y, divider);
            for (index, (name, up, disabled)) in
                [("Increase", true, at_max), ("Decrease", false, at_min)]
                    .into_iter()
                    .enumerate()
            {
                let half = column.height() / 2.;
                let step_rect = Rect::from_min_size(
                    egui::pos2(column.left(), column.top() + half * index as f32),
                    egui::vec2(column.width(), half),
                );
                // `tabIndex={-1}`: steppers are clickable but not focusable.
                let step = ui.interact(step_rect, id.with(name), egui::Sense::CLICK);
                let accessible = format!("{name} {}", self.label);
                step.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, !disabled, &accessible)
                });
                let hot = !disabled && step.hovered();
                if hot {
                    ui.painter()
                        .rect_filled(step_rect.shrink(1.), 0., t.color("surface-active"));
                }
                let color = t
                    .color(if hot { "text" } else { "text-subtle" })
                    .gamma_multiply(if disabled { 0.3 } else { 1. });
                // Shipping 12-unit chevrons `M3 7.5 6 4.5 9 7.5` / `M3 4.5 6 7.5 9 4.5`.
                let glyph = Rect::from_center_size(step_rect.center(), egui::Vec2::splat(11.));
                let s = glyph.width() / 12.;
                let p = |x: f32, y: f32| glyph.min + egui::vec2(x * s, y * s);
                let points = if up {
                    vec![p(3., 7.5), p(6., 4.5), p(9., 7.5)]
                } else {
                    vec![p(3., 4.5), p(6., 7.5), p(9., 4.5)]
                };
                ui.painter()
                    .add(egui::Shape::line(points, Stroke::new(2. * s, color)));
                if step.clicked() && !disabled {
                    let next = number::step_from(&buffer, self.step, up, self.min, self.max);
                    set(value, next);
                    if text.has_focus() {
                        ui.data_mut(|data| {
                            data.insert_temp(buffer_id, number_text(N::from_f64(next), self.step));
                        });
                    }
                }
            }
        }
        if changed {
            response.mark_changed();
        }
        response
    }
}

/// One labelled position on a [`RangeSlider`] track.
pub struct RangeMark<'a> {
    pub value: f64,
    pub label: &'a str,
}

/// Shipping `RangeSlider`: a value readout above a 4 pt accent track with a
/// 14 pt thumb, optional ticks and labels under it, and an optional
/// description (`NotchedSlider`). Arrow keys step, Page keys take ten steps,
/// Home/End jump to the bounds.
pub struct RangeSlider<'a> {
    id_salt: egui::IdSalt,
    label: &'a str,
    width: f32,
    min: f64,
    max: f64,
    step: f64,
    value_text: String,
    marks: &'a [RangeMark<'a>],
    description: &'a str,
}

impl<'a> RangeSlider<'a> {
    /// `label` is the accessible name; `value_text` the readout and value text.
    pub fn new(
        id_salt: impl std::hash::Hash + std::fmt::Debug,
        label: &'a str,
        width: f32,
        range: std::ops::RangeInclusive<f64>,
        value_text: String,
    ) -> Self {
        Self {
            id_salt: egui::IdSalt::new(id_salt),
            label,
            width,
            min: *range.start(),
            max: *range.end(),
            step: 1.,
            value_text,
            marks: &[],
            description: "",
        }
    }
    /// Ticks and labels under the track (shipping `marks`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "shipping RangeSlider API; no native surface passes marks yet"
        )
    )]
    pub fn marks(mut self, marks: &'a [RangeMark<'a>]) -> Self {
        self.marks = marks;
        self
    }
    /// Copy under the slider (shipping `NotchedSlider` description).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "shipping RangeSlider API; no native surface passes one yet"
        )
    )]
    pub fn description(mut self, description: &'a str) -> Self {
        self.description = description;
        self
    }

    /// Returns the track's response (probe it for the thumb's travel),
    /// marked changed when `value` changed.
    pub fn show(self, ui: &mut egui::Ui, t: &Tokens, value: &mut f64) -> egui::Response {
        use captures_app::controls::range;
        let id = ui.make_persistent_id(self.id_salt);
        let enabled = ui.is_enabled();
        let alpha = if enabled { 1. } else { 0.45 };
        let has_marks = !self.marks.is_empty();
        let readout_height = 14.;
        let track_height = 20.;
        let gap = 2.;
        let labels_height = if has_marks { gap + 14. } else { 0. };
        let (bounds, _) = ui.allocate_exact_size(
            egui::vec2(
                self.width,
                readout_height + gap + track_height + labels_height,
            ),
            egui::Sense::hover(),
        );
        let track = Rect::from_min_size(
            egui::pos2(bounds.left(), bounds.top() + readout_height + gap),
            egui::vec2(bounds.width(), track_height),
        );
        let mut response = ui.interact(track, id, egui::Sense::click_and_drag());
        let travel = track.x_range().shrink(7.);
        let mut next = *value;
        if enabled {
            if response.is_pointer_button_down_on()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let fraction = f64::from((pointer.x - travel.min) / travel.span().max(1.));
                next = range::value_at(fraction, self.min, self.max, self.step);
                // A native range input takes focus when pressed.
                response.request_focus();
            }
            if response.has_focus() {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            ..Default::default()
                        },
                    );
                });
                let big = self.step * 10.;
                for (key, delta) in [
                    (egui::Key::ArrowRight, Some(self.step)),
                    (egui::Key::ArrowUp, Some(self.step)),
                    (egui::Key::ArrowLeft, Some(-self.step)),
                    (egui::Key::ArrowDown, Some(-self.step)),
                    (egui::Key::PageUp, Some(big)),
                    (egui::Key::PageDown, Some(-big)),
                    (egui::Key::Home, None),
                    (egui::Key::End, None),
                ] {
                    // Every press counts, even several in one frame.
                    while ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                        next = match (key, delta) {
                            (egui::Key::Home, _) => self.min,
                            (egui::Key::End, _) => self.max,
                            (_, Some(delta)) => (next + delta).clamp(self.min, self.max),
                            _ => next,
                        };
                    }
                }
            }
        }
        if next != *value {
            *value = next;
            response.mark_changed();
        }
        let value_now = *value;
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::slider(enabled, value_now, self.label);
            info.current_text_value = Some(self.value_text.clone());
            info
        });

        let painter = ui.painter();
        // `.range-slider-value output`.
        let readout = painter.layout_no_wrap(
            self.value_text.clone(),
            egui::FontId::proportional(t.number("text-xs")),
            t.color("text-subtle"),
        );
        painter.galley(
            egui::pos2(
                bounds.right() - readout.size().x,
                bounds.top() + (readout_height - readout.size().y) / 2.,
            ),
            readout,
            t.color("text-subtle").gamma_multiply(alpha),
        );
        let fraction = range::fraction(value_now, self.min, self.max) as f32;
        let bar = Rect::from_min_max(
            egui::pos2(track.left(), track.center().y - 2.),
            egui::pos2(track.right(), track.center().y + 2.),
        );
        let x = travel.min + travel.span() * fraction;
        painter.rect_filled(bar, 2., t.color("n-6").gamma_multiply(alpha));
        painter.rect_filled(
            Rect::from_min_max(bar.min, egui::pos2(x, bar.bottom())),
            2.,
            t.color("theme-accent").gamma_multiply(alpha),
        );
        for mark in self.marks {
            let mark_x =
                travel.min + travel.span() * range::fraction(mark.value, self.min, self.max) as f32;
            painter.circle_filled(
                egui::pos2(mark_x, track.center().y),
                1.,
                t.color("border-strong").gamma_multiply(alpha),
            );
        }
        let hovered = enabled && (response.hovered() || response.dragged());
        let thumb_radius = if hovered { 7. * 1.08 } else { 7. };
        let thumb = Rect::from_center_size(
            egui::pos2(x, track.center().y),
            egui::Vec2::splat(thumb_radius * 2.),
        );
        painter.add(
            crate::preferences_widgets::shadow_sm(ui.visuals().dark_mode)
                .as_shape(thumb, thumb_radius),
        );
        painter.circle(
            thumb.center(),
            thumb_radius - 0.5,
            t.color("surface-raised"),
            Stroke::new(1., t.color("border-strong").gamma_multiply(alpha)),
        );
        if response.has_focus() {
            focus_ring(ui, t, thumb, thumb_radius);
        }
        if has_marks {
            let top = track.bottom() + gap + 1.;
            let last = self.marks.len() - 1;
            for (index, mark) in self.marks.iter().enumerate() {
                let label = ui.painter().layout_no_wrap(
                    mark.label.to_owned(),
                    egui::FontId::proportional(t.number("text-2xs")),
                    t.color("text-faint"),
                );
                let mark_x = travel.min
                    + travel.span() * range::fraction(mark.value, self.min, self.max) as f32;
                let left = match index {
                    0 => mark_x,
                    index if index == last => mark_x - label.size().x,
                    _ => mark_x - label.size().x / 2.,
                };
                ui.painter().galley(
                    egui::pos2(left, top),
                    label,
                    t.color("text-faint").gamma_multiply(alpha),
                );
            }
        }
        if !self.description.is_empty() {
            ui.add_space(t.number("s-2") - ui.spacing().item_spacing.y);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(self.description)
                        .size(t.number("text-xs"))
                        .color(t.color("text-subtle")),
                )
                .wrap(),
            );
        }
        response
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
    fn focus_ring_keeps_focus_requested_during_a_click_elsewhere() {
        // The canvas text composer requests focus in the pass that handles
        // the canvas click; the ring's end-of-pass lookup must not replay
        // that click against the new field and take its focus away.
        let (ctx, _) = setup();
        install_focus_ring(&ctx);
        let field = egui::Id::unique("composer-field");
        let mut text = String::new();
        let mut draw = |ui: &mut egui::Ui, focus: bool| {
            let response = ui.add(egui::TextEdit::singleline(&mut text).id(field));
            if focus {
                response.request_focus();
            }
        };
        frame(&ctx, vec![], |ui| draw(ui, false));
        let pos = egui::pos2(500., 350.);
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(pos), press(true), press(false)],
            |ui| draw(ui, true),
        );
        assert!(ctx.memory(|memory| memory.has_focus(field)));
        assert_eq!(
            ctx.options(|options| options.input_options.surrender_focus_on),
            egui::SurrenderFocusOn::default()
        );
        frame(&ctx, vec![egui::Event::Text("Text".into())], |ui| {
            draw(ui, false)
        });
        assert_eq!(text, "Text");
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

    #[test]
    fn number_input_steps_with_arrows_and_steppers_within_bounds() {
        let (ctx, t) = setup();
        let mut value: u32 = 9;
        let run = |events: Vec<egui::Event>, value: &mut u32| {
            let mut field = Rect::NOTHING;
            frame(&ctx, events, |ui| {
                field = NumberInput::new("test-number", "Crop X", 120.)
                    .range(0. ..=10.)
                    .show(ui, &t, value)
                    .rect;
            });
            field
        };
        let field = run(vec![], &mut value);
        // The Increase stepper is the top half of the 26 pt column.
        let increase = egui::pos2(field.right() - 13., field.top() + field.height() / 4.);
        run(
            vec![egui::Event::PointerMoved(increase), press(increase, true)],
            &mut value,
        );
        run(vec![press(increase, false)], &mut value);
        assert_eq!(value, 10);
        run(
            vec![egui::Event::PointerMoved(increase), press(increase, true)],
            &mut value,
        );
        run(vec![press(increase, false)], &mut value);
        assert_eq!(value, 10, "Increase is disabled at the maximum");
        let text = egui::pos2(field.left() + 20., field.center().y);
        run(
            vec![egui::Event::PointerMoved(text), press(text, true)],
            &mut value,
        );
        run(vec![press(text, false)], &mut value);
        run(vec![key(egui::Key::ArrowDown)], &mut value);
        run(vec![key(egui::Key::ArrowDown)], &mut value);
        assert_eq!(value, 8, "ArrowDown steps the focused field");
    }

    #[test]
    fn range_slider_follows_pointer_and_keys_and_lays_out_marks() {
        let (ctx, t) = setup();
        let marks = [
            RangeMark {
                value: 0.,
                label: "Low",
            },
            RangeMark {
                value: 100.,
                label: "High",
            },
        ];
        let mut value = 100.;
        let run = |events: Vec<egui::Event>, value: &mut f64| {
            let mut track = Rect::NOTHING;
            frame(&ctx, events, |ui| {
                let text = format!("{value}%");
                track = RangeSlider::new("test-range", "Volume", 214., 0. ..=200., text)
                    .marks(&marks)
                    .description("Louder than the source above 100%.")
                    .show(ui, &t, value)
                    .rect;
            });
            track
        };
        let track = run(vec![], &mut value);
        assert_eq!(track.height(), 20.);
        // The thumb travels 7 pt inside each end: 200 pt for 0–200.
        let quarter = egui::pos2(track.left() + 7. + 50., track.center().y);
        run(
            vec![egui::Event::PointerMoved(quarter), press(quarter, true)],
            &mut value,
        );
        run(vec![press(quarter, false)], &mut value);
        assert_eq!(value, 50.);
        run(vec![key(egui::Key::ArrowRight)], &mut value);
        assert_eq!(value, 51.);
        run(vec![key(egui::Key::PageUp)], &mut value);
        assert_eq!(value, 61.);
        run(vec![key(egui::Key::End)], &mut value);
        assert_eq!(value, 200.);
        run(vec![key(egui::Key::Home)], &mut value);
        assert_eq!(value, 0.);
    }
}
