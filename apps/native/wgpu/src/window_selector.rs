use captures_app::selection::Point;
use captures_capture::{DisplayDescriptor, WindowDescriptor};
use eframe::egui::{self, Color32, FontId, Pos2, Sense, Stroke, StrokeKind, TextureHandle};

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

#[derive(Default)]
pub struct WindowSelector {
    hovered: Option<SelectionTarget>,
}

impl WindowSelector {
    pub fn hovered(&self) -> Option<SelectionTarget> {
        self.hovered
    }

    pub fn reset(&mut self) {
        self.hovered = None;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        frozen: Option<&TextureHandle>,
        display: &DisplayDescriptor,
        windows: &[WindowDescriptor],
        hit_test: impl Fn(Point) -> Option<usize>,
    ) -> Option<Action> {
        let surface = ui.max_rect();
        let coordinates = CoordinateMap::new(surface, display);
        let response = ui.allocate_rect(surface, Sense::click());
        if let Some(position) = response.hover_pos() {
            self.hovered = Some(match hit_test(coordinates.point(position)) {
                Some(index) if index < windows.len() => SelectionTarget::Window(index),
                _ => SelectionTarget::Display,
            });
        } else if ui.input(|input| input.pointer.latest_pos()).is_none() {
            self.hovered = None;
        }

        paint_surface(
            ui,
            tokens,
            frozen,
            coordinates,
            display,
            windows,
            self.hovered,
        );

        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            return Some(Action::Cancel);
        }
        if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(target) = self.hovered
        {
            return Some(Action::Confirm(target));
        }
        response
            .clicked()
            .then_some(self.hovered)
            .flatten()
            .map(Action::Confirm)
    }

    pub fn exercise(&mut self, cycle: usize, hit_test: impl Fn(Point) -> Option<usize>) {
        let point = match cycle {
            0 => Some(Point { x: 210., y: 130. }),
            1 => Some(Point { x: 540., y: 250. }),
            2 => Some(Point { x: 500., y: 20. }),
            3 => Some(Point { x: 920., y: 680. }),
            4 => Some(Point { x: 720., y: 500. }),
            5 => None,
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
    frozen: Option<&TextureHandle>,
    coordinates: CoordinateMap,
    display: &DisplayDescriptor,
    windows: &[WindowDescriptor],
    hovered: Option<SelectionTarget>,
) {
    let surface = coordinates.surface;
    let painter = ui.painter();
    if let Some(texture) = frozen {
        painter.image(
            texture.id(),
            surface,
            egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
    }
    let selected = match hovered {
        Some(SelectionTarget::Window(index)) => windows
            .get(index)
            .map(|window| (coordinates.rect(window, display), window)),
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
            if window.title.is_empty() {
                window.app_name.as_deref().unwrap_or("Window")
            } else {
                &window.title
            },
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
        "Click or press Enter to capture · Esc to cancel".into(),
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
        let action = selector.show(&mut ui, &tokens, None, &display, &windows, |point| {
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
        });
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn raw_input_hovers_frontmost_window_and_clicks_it() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![]);
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(520., 230.)),
                pointer(egui::pos2(520., 230.), true),
            ],
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Window(1)));
        assert_eq!(
            run_input(
                &ctx,
                &mut selector,
                vec![pointer(egui::pos2(520., 230.), false)]
            ),
            Some(Action::Confirm(SelectionTarget::Window(1)))
        );
    }

    #[test]
    fn raw_input_maps_shell_and_desktop_to_display_then_enter_confirms() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![]);
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(500., 20.))],
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Display));
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Enter)]),
            Some(Action::Confirm(SelectionTarget::Display))
        );
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(920., 680.))],
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Display));
    }

    #[test]
    fn raw_input_escape_cancels_and_reset_clears_hover() {
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        run_input(&ctx, &mut selector, vec![]);
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(200., 130.))],
        );
        assert!(selector.hovered().is_some());
        assert_eq!(
            run_input(&ctx, &mut selector, vec![key(egui::Key::Escape)]),
            Some(Action::Cancel)
        );
        selector.reset();
        assert_eq!(selector.hovered(), None);
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
}
