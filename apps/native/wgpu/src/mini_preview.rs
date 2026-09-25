use eframe::egui::{self, Align, Layout, RichText, Stroke};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    ExpandStack,
    MoveStack(egui::Pos2),
    Copy,
    Save,
    OpenHistory,
    Dismiss,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackAction {
    ToggleCollapsed,
    ClearAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Busy {
    Copy,
    Save,
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
    pub interactive: bool,
    pub collapsed: bool,
    pub stack_count: usize,
    pub desktop_pointer: Option<egui::Pos2>,
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    let size = egui::vec2(
        ui.available_width(),
        captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
    );
    let (card, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(card, tokens.number("r-xl"), tokens.color("glass-raised"));
    egui::Image::new(view.texture)
        .uv(cover_uv(view.texture.size_vec2(), card.size()))
        .fit_to_exact_size(card.size())
        .corner_radius(tokens.number("r-xl") as u8)
        .paint_at(ui, card);
    ui.painter().rect_stroke(
        card,
        tokens.number("r-xl"),
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

    let footer = egui::Rect::from_min_max(
        egui::pos2(card.left(), card.bottom() - 44.),
        card.right_bottom(),
    );
    ui.painter().rect_filled(
        footer,
        egui::CornerRadius {
            sw: tokens.number("r-xl") as u8,
            se: tokens.number("r-xl") as u8,
            ..Default::default()
        },
        tokens.color("glass-strong"),
    );
    let label = view
        .message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} × {}", view.width, view.height));
    let label_position = card.left_top() + egui::vec2(tokens.number("s-3"), tokens.number("s-3"));
    let galley = ui.painter().layout_no_wrap(
        label,
        egui::FontId::proportional(tokens.number("text-xs")),
        tokens.color("glass-text"),
    );
    let label_rect = egui::Rect::from_min_size(
        label_position,
        galley.size() + egui::vec2(tokens.number("s-3") * 2., tokens.number("s-2") * 2.),
    );
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

    ui.scope_builder(egui::UiBuilder::new().max_rect(footer.shrink(8.)), |ui| {
        tokens.glass_controls(ui);
        ui.horizontal(|ui| {
            let enabled = view.interactive && view.busy.is_none();
            let copy = ui
                .add_enabled(enabled, egui::Button::new("Copy"))
                .on_hover_text("Copy full-resolution pixels");
            if copy.clicked() {
                action = Some(Action::Copy);
            }
            if ui
                .add_enabled(enabled && view.can_save, egui::Button::new("Save"))
                .on_hover_text("Save with current screenshot preferences")
                .clicked()
            {
                action = Some(Action::Save);
            }
            if ui
                .add_enabled(enabled, egui::Button::new("History"))
                .on_hover_text("Select this capture in history")
                .clicked()
            {
                action = Some(Action::OpenHistory);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        view.interactive,
                        egui::Button::new(RichText::new("×").color(tokens.color("glass-text"))),
                    )
                    .on_hover_text("Dismiss preview")
                    .clicked()
                {
                    action = Some(Action::Dismiss);
                }
            });
        });
    });
    action
}

pub fn show_stack_controls(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    count: usize,
    collapsed: bool,
) -> Option<StackAction> {
    let mut action = None;
    tokens.glass_controls(ui);
    ui.horizontal(|ui| {
        let toggle = ui.button(if collapsed { "Expand" } else { "Show less" });
        if toggle.clicked() {
            action = Some(StackAction::ToggleCollapsed);
        }
        ui.label(
            RichText::new(format!("{count} captures"))
                .small()
                .color(tokens.color("glass-text-muted")),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Clear all").clicked() {
                action = Some(StackAction::ClearAll);
            }
        });
    });
    action
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
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
            ),
        );
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
                busy: None,
                message: None,
                can_save: true,
                interactive,
                collapsed,
                stack_count: 3,
                desktop_pointer,
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
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
        let action = show_stack_controls(&mut ui, &tokens, 3, false);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
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
            run_card(&ctx, &texture, moved(egui::pos2(30., 138.)), false, true),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(30., 138.), true),
                false,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(30., 138.), false),
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
        assert_eq!(run_controls(&ctx, moved(egui::pos2(32., 10.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(32., 10.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(32., 10.), false)),
            Some(StackAction::ToggleCollapsed)
        );
        assert_eq!(run_controls(&ctx, moved(egui::pos2(305., 10.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(305., 10.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(305., 10.), false)),
            Some(StackAction::ClearAll)
        );
    }
}
