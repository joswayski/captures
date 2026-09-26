//! Shipping editor pickers drawn by the wgpu host: the `ColorField` swatch row
//! with its custom-color tile, and the text style preview chips
//! (`ScreenshotEditor.tsx`, `styles/editor-image.css`). The palette, copy and
//! grid math come from `captures_app::editor_chrome::colors`.
use super::*;
use captures_app::{editor_chrome::colors as model, editor_text::TextStylePreset};
use eframe::egui::{
    Color32, FontFamily, FontId, Pos2, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use eframe::egui::ecolor::Hsva;

fn seed(value: &str) -> Hsva {
    Hsva::from(Color32::from_hex(&model::custom_seed(value)).unwrap_or(Color32::BLACK))
}

/// The custom color picker, sized to its column: a saturation/brightness
/// square and a hue bar (the system picker's role in shipping). Returns true
/// when the user changed `hsva`.
fn compact_picker(ui: &mut egui::Ui, tokens: &Tokens, hsva: &mut Hsva) -> bool {
    // Stay clear of a scrolling inspector's bar.
    let width = (ui.available_width() - tokens.number("s-4")).max(48.);
    let mut changed = false;
    let (square, response) = ui.allocate_exact_size(
        vec2(width, (width * 0.55).clamp(72., 132.)),
        Sense::click_and_drag(),
    );
    if let Some(pointer) = response.interact_pointer_pos() {
        let s = ((pointer.x - square.left()) / square.width()).clamp(0., 1.);
        let v = 1. - ((pointer.y - square.top()) / square.height()).clamp(0., 1.);
        if (s, v) != (hsva.s, hsva.v) {
            hsva.s = s;
            hsva.v = v;
            changed = true;
        }
    }
    let painter = ui.painter_at(square.expand(8.));
    let steps = 12;
    let mut mesh = egui::Mesh::default();
    for row in 0..=steps {
        for column in 0..=steps {
            let (x, y) = (column as f32 / steps as f32, row as f32 / steps as f32);
            let color = Color32::from(Hsva::new(hsva.h, x, 1. - y, 1.));
            mesh.colored_vertex(
                square.min + vec2(x * square.width(), y * square.height()),
                color,
            );
        }
    }
    let stride = steps + 1;
    for row in 0..steps {
        for column in 0..steps {
            let index = row * stride + column;
            mesh.add_triangle(index, index + 1, index + stride);
            mesh.add_triangle(index + 1, index + stride + 1, index + stride);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.rect_stroke(
        square,
        tokens.number("r-xs"),
        Stroke::new(1., tokens.color("border-strong")),
        StrokeKind::Inside,
    );
    let handle = pos2(
        square.left() + hsva.s * square.width(),
        square.top() + (1. - hsva.v) * square.height(),
    );
    painter.circle_stroke(handle, 5., Stroke::new(2., Color32::WHITE));
    painter.circle_stroke(handle, 6.5, Stroke::new(1., Color32::BLACK));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Saturation and brightness")
    });

    ui.add_space(tokens.number("s-3"));
    let (bar, response) = ui.allocate_exact_size(vec2(width, 14.), Sense::click_and_drag());
    if let Some(pointer) = response.interact_pointer_pos() {
        let h = ((pointer.x - bar.left()) / bar.width()).clamp(0., 1.);
        if h != hsva.h {
            hsva.h = h;
            changed = true;
        }
    }
    let painter = ui.painter_at(bar.expand(8.));
    let mut mesh = egui::Mesh::default();
    let segments = 36;
    for segment in 0..=segments {
        let t = segment as f32 / segments as f32;
        let color = Color32::from(Hsva::new(t, 1., 1., 1.));
        let x = bar.left() + t * bar.width();
        mesh.colored_vertex(pos2(x, bar.top()), color);
        mesh.colored_vertex(pos2(x, bar.bottom()), color);
    }
    for segment in 0..segments as u32 {
        let index = segment * 2;
        mesh.add_triangle(index, index + 1, index + 2);
        mesh.add_triangle(index + 1, index + 3, index + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.rect_stroke(
        bar,
        tokens.number("r-xs"),
        Stroke::new(1., tokens.color("border-strong")),
        StrokeKind::Inside,
    );
    let marker = egui::Rect::from_center_size(
        pos2(bar.left() + hsva.h * bar.width(), bar.center().y),
        vec2(4., bar.height() + 4.),
    );
    painter.rect(
        marker,
        2.,
        Color32::WHITE,
        Stroke::new(1., Color32::BLACK),
        StrokeKind::Outside,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Hue"));
    changed
}

fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// Shipping's `conic-gradient(#f44, #ff4, #4f4, #4ff, #44f, #f4f, #f44)` at
/// `turn` (0 = top, clockwise).
fn conic(turn: f32) -> Color32 {
    let stops: [[u8; 3]; 7] = [
        [0xff, 0x44, 0x44],
        [0xff, 0xff, 0x44],
        [0x44, 0xff, 0x44],
        [0x44, 0xff, 0xff],
        [0x44, 0x44, 0xff],
        [0xff, 0x44, 0xff],
        [0xff, 0x44, 0x44],
    ];
    let position = turn.rem_euclid(1.) * 6.;
    let index = (position.floor() as usize).min(5);
    let t = position - index as f32;
    let [a, b] = [stops[index], stops[index + 1]];
    let mix = |channel: usize| {
        (f32::from(a[channel]) + (f32::from(b[channel]) - f32::from(a[channel])) * t).round() as u8
    };
    Color32::from_rgb(mix(0), mix(1), mix(2))
}

/// Outline of a rounded square, clockwise from the top centre.
fn rounded_outline(rect: egui::Rect, radius: f32) -> Vec<Pos2> {
    let radius = radius.min(rect.width() / 2.).min(rect.height() / 2.);
    let corners = [
        (pos2(rect.right() - radius, rect.top() + radius), -90_f32),
        (pos2(rect.right() - radius, rect.bottom() - radius), 0.),
        (pos2(rect.left() + radius, rect.bottom() - radius), 90.),
        (pos2(rect.left() + radius, rect.top() + radius), 180.),
    ];
    let mut points = vec![pos2(rect.center().x, rect.top())];
    for (center, start) in corners {
        for step in 0..=6 {
            let angle = (start + step as f32 * 15.).to_radians();
            points.push(center + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points.push(pos2(rect.center().x, rect.top()));
    points
}

/// The shipping custom-color tile: a conic hue square with an inset panel.
fn paint_custom_tile(painter: &egui::Painter, tokens: &Tokens, rect: egui::Rect, enabled: bool) {
    let radius = tokens.number("r-sm");
    let center = rect.center();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, Color32::WHITE);
    for point in rounded_outline(rect, radius) {
        let offset = point - center;
        // atan2 from the top, clockwise, as CSS conic gradients measure.
        let turn = offset.x.atan2(-offset.y) / std::f32::consts::TAU;
        let color = conic(turn);
        mesh.colored_vertex(
            point,
            if enabled {
                color
            } else {
                color.gamma_multiply(0.4)
            },
        );
    }
    for index in 1..mesh.vertices.len() as u32 - 1 {
        mesh.add_triangle(0, index, index + 1);
    }
    painter.add(egui::Shape::mesh(mesh));
    let inset = rect.shrink(model::CUSTOM_INSET as f32);
    painter.rect(
        inset,
        3.,
        tokens.color("surface-raised"),
        Stroke::new(1., tokens.color("border-strong")),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1., tokens.color("border-strong")),
        StrokeKind::Inside,
    );
}

fn focus_ring(painter: &egui::Painter, tokens: &Tokens, center: Pos2, radius: f32) {
    painter.circle_stroke(
        center,
        radius + 3.,
        Stroke::new(2., tokens.color("theme-accent")),
    );
}

/// Shipping `ColorField`: eight swatches and a custom-color tile. `compact` is
/// the canvas background card's tighter grid with a visually hidden legend.
/// Returns the chosen `#rrggbb`, once per click or custom-picker change.
pub(super) fn color_field(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    value: &str,
    compact: bool,
    enabled: bool,
) -> Option<String> {
    let mut chosen = None;
    ui.push_id(("color-field", label), |ui| {
        if !compact {
            ui.label(
                egui::RichText::new(label)
                    .size(tokens.number("text-sm"))
                    .color(tokens.color("text-muted")),
            );
        }
        let (row_gap, padding) = if compact {
            (tokens.number("s-3"), tokens.number("s-2"))
        } else {
            (tokens.number("s-4"), tokens.number("s-3"))
        };
        let width = ui.available_width();
        let grid = model::grid(
            f64::from(width),
            if compact {
                model::COMPACT_CELL
            } else {
                model::CELL
            },
        );
        let (rect, _) = ui.allocate_exact_size(
            vec2(
                width,
                grid.height(f64::from(row_gap), f64::from(padding)) as f32,
            ),
            Sense::hover(),
        );
        let painter = ui.painter_at(rect.expand(6.));
        let tile = model::TILE as f32;
        let custom_id = ui.scope_id().with("custom-open");
        let draft_id = ui.scope_id().with("custom-draft");
        let mut custom_open = ui.data(|data| data.get_temp::<bool>(custom_id).unwrap_or(false));
        for index in 0..model::TILE_COUNT {
            let (x, y) = grid.center(index, f64::from(row_gap), f64::from(padding));
            let center = rect.min + vec2(x as f32, y as f32);
            let bounds = egui::Rect::from_center_size(center, Vec2::splat(tile));
            let sense = if enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let response = ui.interact(bounds, ui.scope_id().with(("tile", index)), sense);
            if let Some(swatch) = model::SWATCHES.get(index) {
                let active = model::swatch_active(value, swatch);
                let fill = Color32::from_hex(swatch).unwrap_or(Color32::BLACK);
                if active {
                    painter.circle_filled(center, tile / 2. + 2., tokens.color("surface-raised"));
                    painter.circle_stroke(
                        center,
                        tile / 2. + 3.,
                        Stroke::new(2., tokens.color("theme-accent")),
                    );
                }
                painter.circle(
                    center,
                    tile / 2. - 0.5,
                    if enabled {
                        fill
                    } else {
                        fill.gamma_multiply(0.4)
                    },
                    Stroke::new(1., tokens.color("border-strong")),
                );
                if response.has_focus() && !active {
                    focus_ring(&painter, tokens, center, tile / 2.);
                }
                let name = model::swatch_label(label, swatch);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::RadioButton,
                        enabled,
                        active,
                        &name,
                    )
                });
                if response.clicked() {
                    chosen = Some((*swatch).to_owned());
                }
            } else {
                paint_custom_tile(&painter, tokens, bounds, enabled);
                if response.has_focus() {
                    focus_ring(&painter, tokens, center, tile / 2.);
                }
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        enabled,
                        custom_open,
                        model::CUSTOM_COLOR,
                    )
                });
                let response = response.on_hover_text(model::CUSTOM_COLOR);
                if response.clicked() {
                    custom_open = !custom_open;
                    let seed =
                        Color32::from_hex(&model::custom_seed(value)).unwrap_or(Color32::BLACK);
                    ui.data_mut(|data| {
                        data.insert_temp(custom_id, custom_open);
                        data.insert_temp(draft_id, seed);
                    });
                }
            }
        }
        if custom_open && enabled {
            // The draft keeps the picker steady (and keeps hue for grays) while a
            // live change is applied.
            let mut draft = ui
                .data(|data| data.get_temp::<Hsva>(draft_id))
                .unwrap_or_else(|| seed(value));
            if compact_picker(ui, tokens, &mut draft) {
                chosen = Some(hex(Color32::from(draft)));
            }
            ui.data_mut(|data| data.insert_temp(draft_id, draft));
        }
    });
    chosen
}

/// Shipping `.screenshot-text-style-preview` width and height.
pub(super) const CHIP: Vec2 = Vec2::new(72., 24.);

/// One text style preview chip ("Text") for `preset`, centred in `rect`.
pub(super) fn paint_text_chip(
    painter: &egui::Painter,
    tokens: &Tokens,
    rect: egui::Rect,
    preset: &TextStylePreset,
) {
    let chip = egui::Rect::from_center_size(rect.center(), CHIP);
    let boxed = preset.background.is_some();
    let mono = preset.font_family == "mono";
    let font = if mono {
        FontId::new(15., FontFamily::Monospace)
    } else {
        FontId::new(17., FontFamily::Name(crate::ui_fonts::SEMIBOLD.into()))
    };
    let (ink, fill) = if boxed {
        (tokens.color("solid-ink"), Some(tokens.color("solid")))
    } else {
        (tokens.color("text"), None)
    };
    if let Some(fill) = fill {
        let radius = if preset.rounded_background {
            tokens.number("r-lg")
        } else {
            0.
        };
        painter.rect_filled(chip, radius, fill);
    }
    let galley = painter.layout_no_wrap("Text".into(), font.clone(), ink);
    let origin = chip.center() - galley.size() / 2.;
    if preset.outlined {
        // `-webkit-text-stroke: 1.2px var(--text)` over transparent fill.
        for (dx, dy) in [
            (-1., 0.),
            (1., 0.),
            (0., -1.),
            (0., 1.),
            (-0.7, -0.7),
            (0.7, -0.7),
            (-0.7, 0.7),
            (0.7, 0.7),
        ] {
            painter.galley(origin + vec2(dx, dy), galley.clone(), ink);
        }
        let hole = painter.layout_no_wrap("Text".into(), font, tokens.color("surface-overlay"));
        painter.galley(origin, hole, tokens.color("surface-overlay"));
    } else {
        painter.galley(origin, galley, ink);
    }
}

/// A text style menu row: preview chip, then the label. `preset` is `None`
/// for the native "Plain" row, which has no shipping chip.
pub(super) fn text_style_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    preset: Option<&TextStylePreset>,
    label: &str,
    selected: bool,
) -> egui::Response {
    let height = 38_f32;
    // Menu rows fill their popup, but never stretch an unbounded menu.
    let width = ui.available_width().clamp(180., 248.);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter_at(rect);
    let hovered = response.hovered() || response.has_focus();
    if selected || hovered {
        painter.rect(
            rect,
            tokens.number("r-md"),
            tokens.color(if selected {
                "surface-selected"
            } else {
                "control-hover"
            }),
            Stroke::new(
                1.,
                tokens.color(if selected {
                    "theme-accent"
                } else {
                    "border-strong"
                }),
            ),
            StrokeKind::Inside,
        );
    }
    let chip_rect = egui::Rect::from_min_size(
        rect.min + vec2(tokens.number("s-4"), (height - CHIP.y) / 2.),
        vec2(82., CHIP.y),
    );
    if let Some(preset) = preset {
        paint_text_chip(&painter, tokens, chip_rect, preset);
    }
    let text = painter.layout_no_wrap(
        label.to_owned(),
        FontId::proportional(tokens.number("text-sm")),
        tokens.color(if selected || hovered {
            "text"
        } else {
            "text-muted"
        }),
    );
    painter.galley(
        pos2(
            chip_rect.right() + tokens.number("s-4"),
            rect.center().y - text.size().y / 2.,
        ),
        text,
        Color32::PLACEHOLDER,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label)
    });
    response
}

/// Shipping `TextStylePicker`: a trigger showing the current chip and label
/// that opens the chip menu. `None` is the native "Plain" choice. Returns
/// whether `value` changed.
pub(super) fn text_style_picker(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    presets: &[TextStylePreset],
    value: &mut Option<String>,
) -> bool {
    let mut changed = false;
    let current = presets
        .iter()
        .find(|preset| Some(preset.id) == value.as_deref());
    let title = current.map_or("Plain", |preset| preset.label);
    let width = ui.available_width().max(180.);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 38.), Sense::click());
    let popup_id = ui.make_persistent_id(("text-style-picker", label));
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let hovered = response.hovered() || response.has_focus();
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        tokens.number("r-md"),
        tokens.color(if hovered {
            "control-hover"
        } else {
            "surface-field"
        }),
        Stroke::new(
            1.,
            tokens.color(if open {
                "theme-accent"
            } else if hovered {
                "border-strong"
            } else {
                "control-border"
            }),
        ),
        StrokeKind::Inside,
    );
    let chip_rect = egui::Rect::from_min_size(
        rect.min + vec2(tokens.number("s-4"), (rect.height() - CHIP.y) / 2.),
        vec2(82., CHIP.y),
    );
    if let Some(preset) = current {
        paint_text_chip(&painter, tokens, chip_rect, preset);
    }
    let text = painter.layout_no_wrap(
        title.to_owned(),
        FontId::proportional(tokens.number("text-sm")),
        tokens.color(if hovered { "text" } else { "text-muted" }),
    );
    painter.galley(
        pos2(
            chip_rect.right() + tokens.number("s-4"),
            rect.center().y - text.size().y / 2.,
        ),
        text,
        Color32::PLACEHOLDER,
    );
    super::chrome::icon(
        &painter,
        if open { "chevron-up" } else { "chevron-down" },
        pos2(rect.right() - tokens.number("s-4") - 7., rect.center().y),
        14.,
        1.9,
        tokens.color("text-muted"),
    );
    let name = format!("{label}: {title}");
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::ComboBox, true, open, &name));
    egui::Popup::menu(&response)
        .id(popup_id)
        .width(width)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.;
            if text_style_row(ui, tokens, None, "Plain", value.is_none()).clicked() {
                changed |= value.take().is_some();
                ui.close();
            }
            for preset in presets {
                let selected = Some(preset.id) == value.as_deref();
                if text_style_row(ui, tokens, Some(preset), preset.label, selected).clicked() {
                    changed |= !selected;
                    *value = Some(preset.id.into());
                    ui.close();
                }
            }
        });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conic_tile_follows_shipping_stops() {
        assert_eq!(conic(0.), Color32::from_rgb(0xff, 0x44, 0x44));
        assert_eq!(conic(1. / 6.), Color32::from_rgb(0xff, 0xff, 0x44));
        assert_eq!(conic(0.5), Color32::from_rgb(0x44, 0xff, 0xff));
        assert_eq!(conic(1.), conic(0.));
        assert_eq!(conic(-0.25), conic(0.75));
    }

    #[test]
    fn rounded_outline_closes_inside_its_rect() {
        let rect = egui::Rect::from_min_size(pos2(10., 20.), Vec2::splat(24.));
        let outline = rounded_outline(rect, 6.);
        assert_eq!(outline.first(), outline.last());
        assert!(
            outline
                .iter()
                .all(|point| rect.expand(0.01).contains(*point))
        );
    }

    #[test]
    fn custom_picker_drags_saturation_brightness_and_hue_within_its_column() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut hsva = seed("#2d9cff80");
        assert_eq!(hex(Color32::from(hsva)), "#2d9cff");
        let frame = |hsva: &mut Hsva, events| {
            let mut changed = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(240., 400.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.set_width(200.);
                        changed |= compact_picker(ui, &tokens, hsva);
                    });
                },
            );
            output.textures_delta.clear();
            changed
        };
        assert!(
            !frame(&mut hsva, vec![]),
            "showing the picker is not a change"
        );
        let press = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let release = |pos| {
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        let inside = pos2(100., 30.);
        let beyond = pos2(400., 0.);
        assert!(
            frame(&mut hsva, press(inside)),
            "pressing the square picks a color"
        );
        frame(&mut hsva, vec![egui::Event::PointerMoved(beyond)]);
        frame(&mut hsva, release(beyond));
        // Dragging past the corner clamps to full saturation and brightness.
        assert!(
            (hsva.s - 1.).abs() < 1e-6 && (hsva.v - 1.).abs() < 1e-6,
            "{hsva:?}"
        );
        assert_eq!(hex(Color32::from(hsva)).len(), 7);
    }

    #[test]
    fn hex_is_lowercase_rrggbb() {
        assert_eq!(hex(Color32::from_rgb(0xAB, 0x0C, 0xFF)), "#ab0cff");
    }
}
