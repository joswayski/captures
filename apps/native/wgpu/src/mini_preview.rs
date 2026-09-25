use eframe::egui::{self, Color32, Stroke};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    ExpandStack,
    MoveStack(egui::Pos2),
    DragFile,
    Copy,
    Save,
    Reveal,
    Edit,
    Trash,
    Dismiss,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackAction {
    ToggleCollapsed,
    ClearAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Busy {
    Drag,
    Copy,
    Save,
    Reveal,
    Trash,
}

pub fn stack_controls_visible(count: usize, collapsed: bool) -> bool {
    !collapsed && count >= 2
}

pub struct View<'a> {
    pub artifact_id: &'a str,
    pub texture: &'a egui::TextureHandle,
    pub width: u32,
    pub height: u32,
    pub busy: Option<Busy>,
    pub message: Option<&'a str>,
    pub can_save: bool,
    pub saved: bool,
    pub interactive: bool,
    pub collapsed: bool,
    pub stack_count: usize,
    pub depth: usize,
    pub desktop_pointer: Option<egui::Pos2>,
    pub reject_offset: f32,
    pub right_anchor: bool,
}

pub fn reject_offset(elapsed_seconds: f32, reduced_motion: bool) -> f32 {
    let duration = captures_app::preview::PREVIEW_DROP_REJECT_MS as f32 / 1000.;
    if reduced_motion || elapsed_seconds >= duration {
        return 0.;
    }
    let progress = (elapsed_seconds / duration).clamp(0., 1.);
    let stops = [0., -8., 7., -5., 3., 0.];
    let position = progress * 5.;
    let index = (position as usize).min(4);
    egui::lerp(stops[index]..=stops[index + 1], position - index as f32)
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    let size = egui::vec2(
        ui.available_width(),
        captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
    );
    let (card, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let card = card.translate(egui::vec2(view.reject_offset, 0.));
    let radius = tokens.number("thumbnail-card-radius");
    ui.painter()
        .rect_filled(card, radius, tokens.color("glass-raised"));
    egui::Image::new(view.texture)
        .uv(cover_uv(view.texture.size_vec2(), card.size()))
        .fit_to_exact_size(card.size())
        .corner_radius(radius as u8)
        .paint_at(ui, card);
    if view.collapsed && view.depth > 0 {
        ui.painter().rect_filled(
            card,
            radius,
            tokens
                .color("glass-strong-solid")
                .gamma_multiply(captures_app::preview::collapsed_dim_opacity(view.depth) as f32),
        );
    }
    ui.painter().rect_stroke(
        card,
        radius,
        Stroke::new(1., tokens.color("glass-border")),
        egui::StrokeKind::Inside,
    );

    if view.collapsed {
        if view.interactive {
            let response = ui.interact(
                card,
                ui.scope_id()
                    .with(("expand-preview-stack", view.artifact_id)),
                egui::Sense::click_and_drag(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    format!("Expand {} previews", view.stack_count),
                )
            });
            if response.clicked() {
                action = Some(Action::ExpandStack);
            }
            let drag_id = response.id.with("desktop-drag");
            // winit can retain old local coordinates after moving the window.
            // Keep the original grab offset and use an OS desktop pointer sample.
            if response.is_pointer_button_down_on() {
                if ui.input(|input| input.pointer.primary_pressed()) {
                    ui.data_mut(|data| data.remove::<(egui::Pos2, Option<egui::Pos2>)>(drag_id));
                }
                let press = ui.input(|input| input.pointer.press_origin());
                if let Some(press) = press {
                    ui.data_mut(|data| {
                        data.get_temp_mut_or_insert_with(drag_id, || (press, None::<egui::Pos2>));
                    });
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(pointer) = view.desktop_pointer
                && let Some((press, previous)) =
                    ui.data(|data| data.get_temp::<(egui::Pos2, Option<egui::Pos2>)>(drag_id))
            {
                let position = pointer - press.to_vec2();
                if previous != Some(position) {
                    ui.data_mut(|data| data.insert_temp(drag_id, (press, Some(position))));
                    action = Some(Action::MoveStack(position));
                }
            }
            if !ui.input(|input| input.pointer.primary_down()) {
                ui.data_mut(|data| data.remove::<(egui::Pos2, Option<egui::Pos2>)>(drag_id));
            }
            response
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("Click to expand; drag to move the preview pile");
        }
        return action;
    }

    let inset = 8.;
    let icon_size = egui::vec2(28., 28.);
    let gap = tokens.number("s-3");
    let outer_x = if view.right_anchor {
        card.right() - inset - icon_size.x
    } else {
        card.left() + inset
    };
    let inner_x = if view.right_anchor {
        card.left() + inset
    } else {
        card.right() - inset - icon_size.x
    };
    let top_y = card.top() + inset;
    let copy_rect = egui::Rect::from_center_size(
        card.center() - egui::vec2(0., 16. + gap / 2.),
        egui::vec2(140., 32.),
    );
    let save_rect = egui::Rect::from_center_size(
        card.center() + egui::vec2(0., 16. + gap / 2.),
        egui::vec2(140., 32.),
    );
    let edit_rect = egui::Rect::from_min_size(egui::pos2(inner_x, top_y), icon_size);
    let destructive_count: usize = if view.saved { 2 } else { 1 };
    let destructive_width =
        icon_size.x * destructive_count as f32 + gap * (destructive_count.saturating_sub(1)) as f32;
    let destructive_start = if view.right_anchor {
        outer_x + icon_size.x - destructive_width
    } else {
        outer_x
    };
    let close_rect = egui::Rect::from_min_size(egui::pos2(destructive_start, top_y), icon_size);
    let delete_rect = egui::Rect::from_min_size(
        egui::pos2(
            destructive_start + if view.saved { icon_size.x + gap } else { 0. },
            top_y,
        ),
        icon_size,
    );
    let pointer_over_control = ui
        .input(|input| {
            if input.pointer.primary_down() {
                input.pointer.press_origin()
            } else {
                input.pointer.hover_pos()
            }
        })
        .is_some_and(|pointer| {
            [copy_rect, save_rect, edit_rect, delete_rect]
                .iter()
                .any(|rect| rect.contains(pointer))
                || (view.saved && close_rect.contains(pointer))
        });
    if view.interactive && view.busy.is_none() && !pointer_over_control {
        let response = ui.interact(
            card,
            ui.scope_id().with(("file-drag", view.artifact_id)),
            egui::Sense::drag(),
        );
        if response.drag_started_by(egui::PointerButton::Primary) {
            action = Some(Action::DragFile);
        }
        response
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text("Drag the original file to another app");
    }
    let enabled = view.interactive && view.busy.is_none();
    let mut controls = Vec::new();
    if view.saved {
        controls.push((
            close_rect,
            "close",
            "Close",
            Action::Dismiss,
            Icon::Close,
            enabled,
        ));
    }
    controls.push((
        delete_rect,
        "delete",
        "Delete",
        if view.saved {
            Action::Trash
        } else {
            Action::Dismiss
        },
        Icon::Trash,
        enabled,
    ));
    controls.push((
        edit_rect,
        "edit",
        "Edit",
        Action::Edit,
        Icon::Edit,
        enabled && view.can_save,
    ));

    let card_hovered = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pointer| card.contains(pointer));
    let any_focused = ui.memory(|memory| {
        ["close", "delete", "edit", "copy", "save-reveal"]
            .iter()
            .any(|name| memory.has_focus(ui.scope_id().with((name, view.artifact_id))))
    });
    let reveal = view.interactive && (card_hovered || any_focused);
    if reveal {
        // wgpu has no inexpensive blur for an individual egui image. A dark
        // scrim provides shipping-equivalent contrast without CPU readback.
        ui.painter()
            .rect_filled(card, radius, Color32::from_black_alpha(128));
    }

    for (rect, id, label, result, icon, control_enabled) in controls {
        if control(
            ui,
            tokens,
            rect,
            (id, view.artifact_id),
            label,
            icon,
            reveal,
            control_enabled,
            false,
        ) {
            action = Some(result);
        }
    }
    if control(
        ui,
        tokens,
        copy_rect,
        ("copy", view.artifact_id),
        "Copy",
        Icon::Copy,
        reveal,
        enabled,
        false,
    ) {
        action = Some(Action::Copy);
    }
    let second_label = if view.saved {
        "Show in Folder"
    } else {
        "Save file"
    };
    let second_icon = if view.saved { Icon::Folder } else { Icon::Save };
    if control(
        ui,
        tokens,
        save_rect,
        ("save-reveal", view.artifact_id),
        second_label,
        second_icon,
        reveal,
        enabled && (view.saved || view.can_save),
        true,
    ) {
        action = Some(if view.saved {
            Action::Reveal
        } else {
            Action::Save
        });
    }

    let label = view
        .message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} × {}", view.width, view.height));
    let label_position = card.left_bottom() + egui::vec2(inset, -inset);
    let mut label_job = egui::text::LayoutJob::simple(
        label.clone(),
        egui::FontId::proportional(tokens.number("text-2xs")),
        tokens.color("glass-text"),
        card.width() - tokens.number("s-3") * 4.,
    );
    label_job.wrap.max_rows = 3;
    let galley = ui.painter().layout_job(label_job);
    let label_rect = egui::Rect::from_min_size(
        label_position - egui::vec2(0., galley.size().y + tokens.number("s-2") * 2.),
        galley.size() + egui::vec2(tokens.number("s-3") * 2., tokens.number("s-2") * 2.),
    );
    if view.message.is_some() || !reveal {
        ui.painter().rect_filled(
            label_rect,
            tokens.number("r-md"),
            tokens.color("glass-strong"),
        );
        ui.painter().galley(
            label_rect.min + egui::vec2(tokens.number("s-3"), tokens.number("s-2")),
            galley,
            tokens.color("glass-text"),
        );
    }
    ui.interact(
        label_rect,
        ui.scope_id().with("preview-status"),
        egui::Sense::hover(),
    )
    .on_hover_text(label);

    action
}

pub fn show_stack_controls(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    _count: usize,
    collapsed: bool,
    right_anchor: bool,
) -> Option<StackAction> {
    let mut action = None;
    let size = egui::vec2(28., 28.);
    let gap = tokens.number("s-1");
    let outer_x = if right_anchor {
        ui.max_rect().right() - 28.
    } else {
        ui.max_rect().left()
    };
    let inner_x = outer_x
        + if right_anchor {
            -(28. + gap)
        } else {
            28. + gap
        };
    let y = ui.max_rect().center().y - 14.;
    if control(
        ui,
        tokens,
        egui::Rect::from_min_size(egui::pos2(outer_x, y), size),
        "clear-all",
        "Clear all",
        Icon::Close,
        true,
        true,
        false,
    ) {
        action = Some(StackAction::ClearAll);
    }
    if control(
        ui,
        tokens,
        egui::Rect::from_min_size(egui::pos2(inner_x, y), size),
        "show-less",
        if collapsed { "Expand" } else { "Show less" },
        Icon::Stack,
        true,
        true,
        false,
    ) {
        action = Some(StackAction::ToggleCollapsed);
    }
    action
}

#[derive(Clone, Copy)]
enum Icon {
    Close,
    Trash,
    Edit,
    Copy,
    Save,
    Folder,
    Stack,
}

#[allow(clippy::too_many_arguments)]
fn control(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    icon: Icon,
    reveal: bool,
    enabled: bool,
    primary: bool,
) -> bool {
    let id = ui.scope_id().with(id_source);
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let visible = reveal || response.has_focus();
    if visible {
        let fill = if primary {
            tokens.color("theme-accent")
        } else if response.hovered() {
            tokens.color("glass-raised")
        } else {
            tokens.color("glass-strong")
        };
        ui.painter().rect(
            rect,
            tokens.number("r-md"),
            fill,
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        let color = if primary {
            tokens.color("theme-accent-ink")
        } else if enabled {
            if matches!(icon, Icon::Trash) {
                tokens.color("theme-signal-text")
            } else {
                tokens.color("glass-text")
            }
        } else {
            tokens.color("glass-text-muted")
        };
        let label = (rect.width() > 40.).then(|| {
            ui.painter().layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(tokens.number("text-xs")),
                color,
            )
        });
        let gap = tokens.number("s-3");
        let icon_center = label.as_ref().map_or(rect.center(), |text| {
            rect.center() - egui::vec2((text.size().x + gap) / 2., 0.)
        });
        paint_icon(
            ui.painter(),
            icon,
            egui::Rect::from_center_size(icon_center, egui::vec2(16., 16.)),
            color,
        );
        if let Some(text) = label {
            ui.painter().galley(
                egui::pos2(
                    icon_center.x + 8. + gap,
                    rect.center().y - text.size().y / 2.,
                ),
                text,
                color,
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.),
                tokens.number("r-md"),
                Stroke::new(2., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
    }
    response.on_hover_text(label).clicked() && enabled
}

fn paint_icon(p: &egui::Painter, icon: Icon, rect: egui::Rect, color: Color32) {
    let q = |x: f32, y: f32| {
        egui::pos2(
            rect.left() + x / 24. * rect.width(),
            rect.top() + y / 24. * rect.height(),
        )
    };
    let stroke = Stroke::new(1.8, color);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| q(x, y)).collect(),
            stroke,
        ))
    };
    match icon {
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(18., 6.), (6., 18.)]);
        }
        Icon::Edit => {
            line(&[
                (4., 16.),
                (3., 21.),
                (8., 20.),
                (19., 9.),
                (15., 5.),
                (4., 16.),
                (8., 20.),
            ]);
            line(&[(13.5, 6.5), (17.5, 10.5)]);
        }
        Icon::Trash => {
            line(&[(4., 7.), (20., 7.)]);
            line(&[(9., 7.), (9., 4.), (15., 4.), (15., 7.)]);
            line(&[(18., 7.), (17., 20.), (7., 20.), (6., 7.)]);
            line(&[(10., 11.), (10., 16.)]);
            line(&[(14., 11.), (14., 16.)]);
        }
        Icon::Copy => {
            p.rect_stroke(
                egui::Rect::from_min_max(q(8., 8.), q(19., 19.)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[
                (16., 8.),
                (16., 6.),
                (14., 4.),
                (6., 4.),
                (4., 6.),
                (4., 14.),
                (6., 16.),
                (8., 16.),
            ]);
        }
        Icon::Save => {
            line(&[
                (5., 4.),
                (17., 4.),
                (19., 6.),
                (19., 20.),
                (5., 20.),
                (5., 4.),
            ]);
            line(&[(8., 4.), (8., 10.), (16., 10.), (16., 4.)]);
            line(&[(8., 20.), (8., 14.), (16., 14.), (16., 20.)]);
        }
        Icon::Folder => {
            line(&[
                (3., 7.),
                (3., 17.),
                (5., 19.),
                (19., 19.),
                (21., 17.),
                (21., 9.),
                (19., 7.),
                (12., 7.),
                (10., 5.),
                (5., 5.),
                (3., 7.),
            ]);
            p.circle_stroke(q(16.5, 13.5), rect.width() * 2.5 / 24., stroke);
            line(&[(18.3, 15.3), (20.5, 17.5)]);
        }
        Icon::Stack => {
            line(&[(4., 9.), (12., 4.), (20., 9.), (12., 14.), (4., 9.)]);
            line(&[(4., 13.), (12., 18.), (20., 13.)]);
            line(&[(4., 17.), (12., 22.), (20., 17.)]);
        }
    }
}

fn cover_uv(image: egui::Vec2, target: egui::Vec2) -> egui::Rect {
    let image_aspect = image.x / image.y.max(1.);
    let target_aspect = target.x / target.y.max(1.);
    if image_aspect > target_aspect {
        let visible = target_aspect / image_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(inset, 0.), egui::pos2(1. - inset, 1.))
    } else {
        let visible = image_aspect / target_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(0., inset), egui::pos2(1., 1. - inset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_drop_shake_settles_at_420ms_and_obeys_reduced_motion() {
        assert_eq!(reject_offset(0., false), 0.);
        assert!((reject_offset(0.084, false) + 8.).abs() < 0.001);
        assert!((reject_offset(0.168, false) - 7.).abs() < 0.001);
        assert_ne!(reject_offset(0.419, false), 0.);
        assert_eq!(reject_offset(0.420, false), 0.);
        assert_eq!(reject_offset(1., false), 0.);
        assert_eq!(reject_offset(0.084, true), 0.);
    }

    fn raw(screen: egui::Rect, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        }
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn moved(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(pos)]
    }

    fn run_card(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
    ) -> Option<Action> {
        let pointer = events.iter().rev().find_map(|event| match event {
            egui::Event::PointerMoved(pos) => Some(*pos),
            _ => None,
        });
        run_card_on_desktop(
            ctx,
            texture,
            events,
            collapsed,
            interactive,
            egui::Pos2::ZERO,
            pointer,
        )
    }

    fn run_card_on_desktop(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
        origin: egui::Pos2,
        desktop_pointer: Option<egui::Pos2>,
    ) -> Option<Action> {
        run_card_state(
            ctx,
            texture,
            events,
            collapsed,
            interactive,
            origin,
            desktop_pointer,
            None,
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_card_state(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
        origin: egui::Pos2,
        desktop_pointer: Option<egui::Pos2>,
        busy: Option<Busy>,
        can_save: bool,
        saved: bool,
    ) -> Option<Action> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let mut input = raw(screen, events);
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .outer_rect = Some(egui::Rect::from_min_size(origin, screen.size()));
        ctx.begin_pass(input);
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique(("preview-card-input-test", interactive)),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = show(
            &mut ui,
            &tokens,
            View {
                artifact_id: "artifact",
                texture,
                width: 320,
                height: 180,
                busy,
                message: None,
                can_save,
                saved,
                interactive,
                collapsed,
                stack_count: 3,
                depth: usize::from(!interactive),
                desktop_pointer,
                reject_offset: 0.,
                right_anchor: false,
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn shipping_action_labels_fit_asymmetric_284_by_160_card() {
        for (saved, right_anchor) in [(false, false), (true, false), (false, true), (true, true)] {
            let ctx = egui::Context::default();
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            tokens.apply(&ctx, false);
            let texture = ctx.load_texture(
                "actions",
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                Default::default(),
            );
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
            ctx.begin_pass(raw(rect, moved(rect.center())));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("action-layout"),
                egui::UiBuilder::new().max_rect(rect),
            );
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "saved",
                    texture: &texture,
                    width: 310,
                    height: 170,
                    busy: None,
                    message: None,
                    can_save: true,
                    saved,
                    interactive: true,
                    collapsed: false,
                    stack_count: 1,
                    depth: 0,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor,
                },
            );
            let mut output = ctx.end_pass();
            let labels = ["Copy", if saved { "Show in Folder" } else { "Save file" }];
            let bounds: Vec<_> = labels
                .iter()
                .map(|label| {
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == *label => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .expect("every action must be painted")
                })
                .collect();
            assert!(
                bounds[0].max.y < bounds[1].min.y,
                "actions must be vertical: {bounds:?}"
            );
            assert!(bounds.iter().all(|bounds| rect.contains_rect(*bounds)));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn chrome_hides_at_rest_reveals_on_focus_and_keeps_busy_primary_contrast() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "chrome",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        for (hovered, focused, busy) in [
            (false, false, None),
            (true, false, Some(Busy::Save)),
            (false, true, None),
        ] {
            ctx.begin_pass(raw(
                screen,
                moved(if hovered {
                    screen.center()
                } else {
                    egui::pos2(500., 500.)
                }),
            ));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("chrome-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            if focused {
                ui.memory_mut(|memory| {
                    memory.request_focus(ui.scope_id().with(("copy", "fixture")))
                });
            }
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "fixture",
                    texture: &texture,
                    width: 391,
                    height: 207,
                    busy,
                    message: None,
                    can_save: true,
                    saved: false,
                    interactive: true,
                    collapsed: false,
                    stack_count: 1,
                    depth: 0,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor: true,
                },
            );
            let mut output = ctx.end_pass();
            let text = |label: &str| {
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                    _ => None,
                })
            };
            assert_eq!(text("391 × 207").is_some(), !hovered && !focused);
            assert_eq!(text("Copy").is_some(), hovered || focused);
            if hovered || focused {
                let save = text("Save file").unwrap();
                assert_eq!(
                    save.fallback_color,
                    Color32::from_rgb(23, 24, 27),
                    "busy yellow actions must not turn white"
                );
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn image_drag_starts_on_image_press_but_never_on_action_press() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag-crossing",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let origin = egui::pos2(40., 60.);
        run_card(&ctx, &texture, moved(origin), false, true);
        assert_eq!(
            run_card(&ctx, &texture, pointer(origin, true), false, true),
            Some(Action::DragFile)
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            None
        );
        run_card(&ctx, &texture, moved(egui::pos2(142., 60.)), false, true);
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), true),
                false,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            Some(Action::Copy)
        );
    }

    #[test]
    fn second_button_saves_unsaved_reveals_saved_and_is_guarded_while_busy() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "save-reveal",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let click = egui::pos2(142., 100.);
        for (saved, busy, can_save, expected) in [
            (false, None, true, Some(Action::Save)),
            (true, None, false, Some(Action::Reveal)),
            (false, None, false, None),
            (true, Some(Busy::Reveal), false, None),
        ] {
            run_card_state(
                &ctx,
                &texture,
                moved(click),
                false,
                true,
                egui::Pos2::ZERO,
                Some(click),
                busy,
                can_save,
                saved,
            );
            run_card_state(
                &ctx,
                &texture,
                pointer(click, true),
                false,
                true,
                egui::Pos2::ZERO,
                Some(click),
                busy,
                can_save,
                saved,
            );
            assert_eq!(
                run_card_state(
                    &ctx,
                    &texture,
                    pointer(click, false),
                    false,
                    true,
                    egui::Pos2::ZERO,
                    Some(click),
                    busy,
                    can_save,
                    saved,
                ),
                expected
            );
        }
    }

    fn run_controls(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<StackAction> {
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                captures_app::preview::THUMBNAIL_CONTROL_GUTTER as f32,
            ),
        );
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, events));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("preview-controls-input-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = show_stack_controls(&mut ui, &tokens, 3, false, false);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn only_compact_rear_images_receive_the_glass_depth_overlay() {
        for (collapsed, depth, expected) in [(true, 0, false), (true, 1, true), (false, 1, false)] {
            let ctx = egui::Context::default();
            let texture = ctx.load_texture(
                "shade",
                egui::ColorImage::filled([2, 2], egui::Color32::RED),
                egui::TextureOptions::LINEAR,
            );
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            ctx.begin_pass(raw(screen, vec![]));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("shade-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "red",
                    texture: &texture,
                    width: 2,
                    height: 2,
                    busy: None,
                    message: None,
                    can_save: true,
                    saved: false,
                    interactive: depth == 0,
                    collapsed,
                    stack_count: 2,
                    depth,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor: false,
                },
            );
            let mut output = ctx.end_pass();
            // CSS depth1=.13748, glass-strong-solid=(15,15,18). Premultiplied
            // 8-bit overlay rounds to (2,2,2,35), not black or full opacity.
            let shade = egui::Color32::from_rgba_premultiplied(2, 2, 2, 35);
            assert_eq!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Rect(rect) if rect.fill == shade)),
                expected
            );
            output.textures_delta.clear();
        }
    }

    #[test]
    fn cover_crop_is_centered_and_preserves_aspect() {
        let wide = cover_uv(egui::vec2(400., 100.), egui::vec2(200., 100.));
        assert_eq!(wide.min, egui::pos2(0.25, 0.));
        assert_eq!(wide.max, egui::pos2(0.75, 1.));

        let tall = cover_uv(egui::vec2(100., 400.), egui::vec2(100., 200.));
        assert_eq!(tall.min, egui::pos2(0., 0.25));
        assert_eq!(tall.max, egui::pos2(1., 0.75));
    }

    #[test]
    fn stack_toolbar_only_appears_for_expanded_multi_card_stacks() {
        assert!(!stack_controls_visible(0, false));
        assert!(!stack_controls_visible(1, false));
        assert!(stack_controls_visible(2, false));
        assert!(!stack_controls_visible(2, true));
    }

    #[test]
    fn compact_drag_moves_without_expanding_and_next_click_still_expands() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let press = egui::pos2(100., 80.);
        run_card(&ctx, &texture, moved(press), true, true);
        run_card(&ctx, &texture, pointer(press, true), true, true);
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(142., 53.)), true, true),
            Some(Action::MoveStack(egui::pos2(42., -27.)))
        );
        // The window moved but winit still reports the old local pointer. A
        // fresh desktop sample stays put; do not drift or repaint in a loop.
        assert_eq!(
            run_card_on_desktop(
                &ctx,
                &texture,
                vec![],
                true,
                true,
                egui::pos2(42., -27.),
                Some(egui::pos2(142., 53.))
            ),
            None
        );
        assert_eq!(
            run_card_on_desktop(
                &ctx,
                &texture,
                moved(egui::pos2(110., 90.)),
                true,
                true,
                egui::pos2(42., -27.),
                Some(egui::pos2(152., 63.))
            ),
            Some(Action::MoveStack(egui::pos2(52., -17.)))
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 53.), false),
                true,
                true
            ),
            None
        );
        run_card(&ctx, &texture, pointer(press, true), true, true);
        assert_eq!(
            run_card(&ctx, &texture, pointer(press, false), true, true),
            Some(Action::ExpandStack)
        );
    }

    #[test]
    fn raw_input_routes_expanded_card_action_and_collapsed_front_expansion() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "preview-input-test",
            egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]),
            egui::TextureOptions::LINEAR,
        );

        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(142., 60.)), false, true),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), true),
                false,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            Some(Action::Copy)
        );
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(100., 80.)), true, false),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), true),
                true,
                false
            ),
            None,
            "rear collapsed cards must be inert"
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), false),
                true,
                false
            ),
            None
        );
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(100., 80.)), true, true),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), true),
                true,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), false),
                true,
                true
            ),
            Some(Action::ExpandStack)
        );
    }

    #[test]
    fn raw_input_routes_show_less_and_clear_all_controls() {
        let ctx = egui::Context::default();
        assert_eq!(run_controls(&ctx, moved(egui::pos2(40., 24.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(40., 24.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(40., 24.), false)),
            Some(StackAction::ToggleCollapsed)
        );
        assert_eq!(run_controls(&ctx, moved(egui::pos2(10., 24.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(10., 24.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(10., 24.), false)),
            Some(StackAction::ClearAll)
        );
    }
}
