use captures_app::selection::Point;
use captures_capture::{DisplayDescriptor, WindowDescriptor};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, RichText, Sense, Stroke, StrokeKind, TextureHandle,
};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionTarget {
    Display,
    Window(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Confirm(SelectionTarget),
    Cancel,
}

pub struct View<'a> {
    pub frozen: Option<&'a TextureHandle>,
    pub display: &'a DisplayDescriptor,
    pub windows: &'a [WindowDescriptor],
    pub auto_start: bool,
}

#[derive(Default)]
pub struct WindowSelector {
    hovered: Option<SelectionTarget>,
    selected: Option<SelectionTarget>,
}

impl WindowSelector {
    pub fn hovered(&self) -> Option<SelectionTarget> {
        self.hovered
    }

    #[cfg(test)]
    fn selected(&self) -> Option<SelectionTarget> {
        self.selected
    }

    fn presentation_target(&self) -> Option<SelectionTarget> {
        self.hovered.or(self.selected)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: View<'_>,
        hit_test: impl Fn(Point) -> Option<usize>,
    ) -> Option<Action> {
        let surface = ui.max_rect();
        let coordinates = CoordinateMap::new(surface, view.display);
        let response = ui.allocate_rect(surface, Sense::click());
        if let Some(position) = response.hover_pos() {
            self.hovered = Some(match hit_test(coordinates.point(position)) {
                Some(index) if index < view.windows.len() => SelectionTarget::Window(index),
                _ => SelectionTarget::Display,
            });
        } else if ui.input(|input| input.pointer.latest_pos()).is_none() {
            self.hovered = None;
        }

        paint_surface(
            ui,
            tokens,
            &view,
            coordinates,
            self.presentation_target(),
            self.selected.is_some(),
        );

        let mut action = None;
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        } else if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(target) = self.selected
        {
            action = Some(Action::Confirm(target));
        } else if response.clicked()
            && let Some(target) = self.hovered
        {
            if view.auto_start {
                action = Some(Action::Confirm(target));
            } else {
                self.selected = Some(target);
            }
        }

        if !view.auto_start {
            egui::Area::new("window-selector-toolbar".into())
                .anchor(Align2::CENTER_BOTTOM, egui::vec2(0., -26.))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(tokens.color("glass-strong"))
                        .stroke(Stroke::new(1., tokens.color("glass-border")))
                        .corner_radius(tokens.number("r-2xl") as u8)
                        .inner_margin(tokens.number("s-4") as i8)
                        .show(ui, |ui| {
                            tokens.glass_controls(ui);
                            ui.horizontal(|ui| {
                                if ui.button("×").on_hover_text("Cancel (Esc)").clicked() {
                                    action = Some(Action::Cancel);
                                }
                                ui.separator();
                                ui.label(
                                    RichText::new(selection_label(self.selected, view.windows))
                                        .small()
                                        .color(tokens.color("glass-text-subtle")),
                                );
                                if ui
                                    .add_enabled(
                                        self.selected.is_some(),
                                        egui::Button::new(
                                            RichText::new("Capture")
                                                .color(tokens.color("theme-accent-ink")),
                                        )
                                        .fill(tokens.color("theme-accent"))
                                        .stroke(Stroke::NONE),
                                    )
                                    .on_hover_text("Capture selected target (Enter)")
                                    .clicked()
                                    && let Some(target) = self.selected
                                {
                                    action = Some(Action::Confirm(target));
                                }
                            });
                        });
                });
        }
        action
    }

    pub fn exercise(&mut self, cycle: usize, hit_test: impl Fn(Point) -> Option<usize>) {
        let point = match cycle {
            0 => Some(Point { x: 210., y: 130. }),
            1 => Some(Point { x: 540., y: 250. }),
            2 => Some(Point { x: 500., y: 20. }),
            3 => Some(Point { x: 920., y: 680. }),
            4 => Some(Point { x: 720., y: 500. }),
            5 => {
                self.reset();
                return;
            }
            _ => return,
        };
        self.hovered = point.map(|point| match hit_test(point) {
            Some(index) => SelectionTarget::Window(index),
            None => SelectionTarget::Display,
        });
    }
}

#[derive(Clone, Copy)]
struct CoordinateMap {
    surface: egui::Rect,
    width: f64,
    height: f64,
}

impl CoordinateMap {
    fn new(surface: egui::Rect, display: &DisplayDescriptor) -> Self {
        let (width, height) = display.overlay_size();
        Self {
            surface,
            width,
            height,
        }
    }

    fn point(self, point: Pos2) -> Point {
        Point {
            x: f64::from(point.x - self.surface.left()) * self.width
                / f64::from(self.surface.width()),
            y: f64::from(point.y - self.surface.top()) * self.height
                / f64::from(self.surface.height()),
        }
    }

    fn rect(self, window: &WindowDescriptor, display: &DisplayDescriptor) -> egui::Rect {
        let scale = if DisplayDescriptor::reports_physical_geometry() {
            display.scale_factor.max(1.)
        } else {
            1.
        };
        let left = (f64::from(window.x - display.x) / scale) / self.width;
        let top = (f64::from(window.y - display.y) / scale) / self.height;
        let width = (f64::from(window.width) / scale) / self.width;
        let height = (f64::from(window.height) / scale) / self.height;
        egui::Rect::from_min_size(
            self.surface.min
                + egui::vec2(
                    (left * f64::from(self.surface.width())) as f32,
                    (top * f64::from(self.surface.height())) as f32,
                ),
            egui::vec2(
                (width * f64::from(self.surface.width())) as f32,
                (height * f64::from(self.surface.height())) as f32,
            ),
        )
        .intersect(self.surface)
    }
}

fn paint_surface(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View<'_>,
    coordinates: CoordinateMap,
    hovered: Option<SelectionTarget>,
    has_selection: bool,
) {
    let surface = coordinates.surface;
    let painter = ui.painter();
    if let Some(texture) = view.frozen {
        painter.image(
            texture.id(),
            surface,
            egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
    }
    let selected = match hovered {
        Some(SelectionTarget::Window(index)) => view
            .windows
            .get(index)
            .map(|window| (coordinates.rect(window, view.display), window)),
        _ => None,
    };
    let veil = tokens.color("glass-veil-heavy");
    if let Some((rect, window)) = selected.filter(|(rect, _)| rect.is_positive()) {
        for outside in [
            egui::Rect::from_min_max(surface.min, Pos2::new(surface.right(), rect.top())),
            egui::Rect::from_min_max(Pos2::new(surface.left(), rect.bottom()), surface.max),
            egui::Rect::from_min_max(
                Pos2::new(surface.left(), rect.top()),
                Pos2::new(rect.left(), rect.bottom()),
            ),
            egui::Rect::from_min_max(
                Pos2::new(rect.right(), rect.top()),
                Pos2::new(surface.right(), rect.bottom()),
            ),
        ] {
            painter.rect_filled(outside, 0., veil);
        }
        let radius = window.corner_radius.unwrap_or(0.).clamp(0., 255.) as u8;
        painter.rect_filled(
            rect,
            radius,
            tokens.color("theme-accent").gamma_multiply(0.14),
        );
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(2., tokens.color("theme-accent")),
            StrokeKind::Outside,
        );
        paint_label(
            painter,
            tokens,
            rect.left_top() + egui::vec2(8., 8.),
            window_label(window),
        );
    } else {
        painter.rect_filled(surface, 0., veil);
        if hovered == Some(SelectionTarget::Display) {
            painter.rect_stroke(
                surface.shrink(2.),
                0.,
                Stroke::new(2., tokens.color("theme-accent")),
                StrokeKind::Inside,
            );
            if view.auto_start {
                let text = painter.layout_no_wrap(
                    "Entire display".into(),
                    FontId::proportional(tokens.number("text-sm")),
                    tokens.color("glass-text"),
                );
                let center = surface.center_bottom() - egui::vec2(0., 54.);
                let background =
                    egui::Rect::from_center_size(center, text.size() + egui::vec2(18., 10.));
                painter.rect_filled(
                    background,
                    tokens.number("r-md"),
                    tokens.color("glass-strong"),
                );
                painter.galley(center - text.size() / 2., text, Color32::WHITE);
            }
        }
    }

    let guidance = if hovered == Some(SelectionTarget::Display) {
        "Capture the entire display"
    } else {
        "Choose a window"
    };
    let title = painter.layout_no_wrap(
        guidance.into(),
        FontId::proportional(tokens.number("text-xl")),
        tokens.color("glass-text"),
    );
    let hint = painter.layout_no_wrap(
        if view.auto_start {
            "Click to capture · Esc to cancel"
        } else if has_selection {
            "Press Enter or Capture to confirm · Esc to cancel"
        } else {
            "Click a target to select it · Esc to cancel"
        }
        .into(),
        FontId::proportional(tokens.number("text-sm")),
        tokens.color("glass-text-muted"),
    );
    let center = surface.center_top() + egui::vec2(0., 56.);
    let size = egui::vec2(
        title.size().x.max(hint.size().x) + 28.,
        title.size().y + hint.size().y + 20.,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(center, size),
        tokens.number("r-xl"),
        tokens.color("glass-strong"),
    );
    painter.galley(
        center - egui::vec2(title.size().x / 2., title.size().y + 2.),
        title,
        Color32::WHITE,
    );
    painter.galley(
        center + egui::vec2(-hint.size().x / 2., 4.),
        hint,
        Color32::WHITE,
    );
}

fn paint_label(painter: &egui::Painter, tokens: &Tokens, origin: Pos2, title: &str) {
    let galley = painter.layout(
        title.into(),
        FontId::proportional(tokens.number("text-sm")),
        tokens.color("glass-text"),
        320.,
    );
    let background = egui::Rect::from_min_size(origin, galley.size() + egui::vec2(14., 8.));
    painter.rect_filled(
        background,
        tokens.number("r-sm"),
        tokens.color("glass-strong"),
    );
    painter.galley(origin + egui::vec2(7., 4.), galley, Color32::WHITE);
}

fn window_label(window: &WindowDescriptor) -> &str {
    let title = window.title.trim();
    if !title.is_empty() {
        return title;
    }
    window
        .app_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Window")
}

fn selection_label(target: Option<SelectionTarget>, windows: &[WindowDescriptor]) -> &str {
    match target {
        Some(SelectionTarget::Display) => "Entire display",
        Some(SelectionTarget::Window(index)) => windows
            .get(index)
            .map(window_label)
            .unwrap_or("Selected window"),
        None => "Click a target to select it",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::window::target_index_at_point;

    fn display() -> DisplayDescriptor {
        DisplayDescriptor {
            id: "fixture".into(),
            name: "Fixture".into(),
            x: -100,
            y: 50,
            width: 1000,
            height: 720,
            scale_factor: 1.,
            is_primary: true,
        }
    }

    fn window(id: &str, z_order: i32, x: i32, y: i32, width: u32, height: u32) -> WindowDescriptor {
        WindowDescriptor {
            id: id.into(),
            title: id.into(),
            app_name: None,
            z_order,
            x,
            y,
            width,
            height,
            display_id: "fixture".into(),
            corner_radius: Some(8.),
        }
    }

    fn targets() -> (Vec<WindowDescriptor>, Vec<WindowDescriptor>) {
        (
            vec![
                window("back", 10, 20, 140, 620, 420),
                window("front", 20, 380, 230, 330, 240),
            ],
            vec![window("shell", 30, -100, 50, 1000, 42)],
        )
    }

    fn raw(screen: egui::Rect, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        }
    }

    fn pointer(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
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

    fn run_input(
        ctx: &egui::Context,
        selector: &mut WindowSelector,
        events: Vec<egui::Event>,
        auto_start: bool,
    ) -> Option<Action> {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000., 720.));
        let display = display();
        let (windows, shell) = targets();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, events));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("window-selector-input-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = selector.show(
            &mut ui,
            &tokens,
            View {
                frozen: None,
                display: &display,
                windows: &windows,
                auto_start,
            },
            |point| {
                target_index_at_point(
                    &windows,
                    &shell,
                    point,
                    Point {
                        x: f64::from(display.x),
                        y: f64::from(display.y),
                    },
                    1.,
                )
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn raw_input_false_latches_click_until_enter_while_true_click_starts() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![], false);
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(520., 230.)),
                pointer(egui::pos2(520., 230.), true),
            ],
            false,
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Window(1)));
        assert_eq!(
            run_input(
                &ctx,
                &mut selector,
                vec![pointer(egui::pos2(520., 230.), false)],
                false,
            ),
            None
        );
        assert_eq!(selector.selected(), Some(SelectionTarget::Window(1)));
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(200., 130.))],
            false,
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Window(0)));
        assert_eq!(
            selector.presentation_target(),
            Some(SelectionTarget::Window(0)),
            "hover drives presentation while confirmation stays latched"
        );
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Enter)], false,),
            Some(Action::Confirm(SelectionTarget::Window(1))),
            "Enter must confirm the clicked target, not the later hover"
        );

        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![], true);
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(520., 230.)),
                pointer(egui::pos2(520., 230.), true),
            ],
            true,
        );
        assert_eq!(
            run_input(
                &ctx,
                &mut selector,
                vec![pointer(egui::pos2(520., 230.), false)],
                true,
            ),
            Some(Action::Confirm(SelectionTarget::Window(1)))
        );
        assert_eq!(selector.selected(), None);
    }

    #[test]
    fn raw_input_maps_shell_and_desktop_to_display_then_enter_confirms() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![], false);
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(500., 20.))],
            false,
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Display));
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Enter)], false),
            None,
            "Enter cannot confirm a mere hover"
        );
        run_input(
            &ctx,
            &mut selector,
            vec![pointer(egui::pos2(500., 20.), true)],
            false,
        );
        assert_eq!(
            run_input(
                &ctx,
                &mut selector,
                vec![pointer(egui::pos2(500., 20.), false)],
                false,
            ),
            None
        );
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Enter)], false),
            Some(Action::Confirm(SelectionTarget::Display))
        );
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(920., 680.))],
            false,
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Display));
    }

    #[test]
    fn raw_input_escape_cancels_and_reset_clears_hover() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![], false);
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(200., 130.))],
            false,
        );
        assert!(selector.hovered().is_some());
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Escape)], false,),
            Some(Action::Cancel)
        );
        selector.reset();
        assert_eq!(selector.hovered(), None);
        assert_eq!(selector.selected(), None);
    }

    #[test]
    fn coordinate_map_handles_asymmetric_viewport_scaling() {
        let display = DisplayDescriptor {
            width: 2000,
            height: 1500,
            ..display()
        };
        let map = CoordinateMap::new(
            egui::Rect::from_min_size(egui::pos2(20., 30.), egui::vec2(1000., 500.)),
            &display,
        );
        let point = map.point(egui::pos2(120., 130.));
        assert_eq!((point.x, point.y), (200., 300.));
        let rect = map.rect(&window("mapped", 1, 100, 350, 400, 600), &display);
        assert_eq!(rect.min, egui::pos2(120., 130.));
        assert_eq!(rect.size(), egui::vec2(200., 200.));
    }

    #[test]
    fn label_uses_nonempty_trimmed_title_then_app_then_fallback() {
        let mut target = window("id", 1, 0, 0, 100, 100);
        target.title = "  Document  ".into();
        target.app_name = Some(" Editor ".into());
        assert_eq!(window_label(&target), "Document");
        target.title = "  ".into();
        assert_eq!(window_label(&target), "Editor");
        target.app_name = Some(" ".into());
        assert_eq!(window_label(&target), "Window");
        target.app_name = None;
        assert_eq!(window_label(&target), "Window");
    }
}
