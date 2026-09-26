use captures_app::capture_menu::{self, DisplayIdentity, GuidanceTarget};
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
    scripted: bool,
}

impl WindowSelector {
    pub fn fixture() -> Self {
        Self {
            scripted: true,
            ..Self::default()
        }
    }

    pub fn hovered(&self) -> Option<SelectionTarget> {
        self.hovered
    }

    pub fn selected(&self) -> Option<SelectionTarget> {
        self.selected
    }

    fn presentation_target(&self) -> Option<SelectionTarget> {
        self.hovered.or(self.selected)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn clear_hover(&mut self) {
        self.hovered = None;
        self.scripted = false;
    }

    pub fn clear_selection_and_hover(&mut self) {
        self.selected = None;
        self.clear_hover();
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: View<'_>,
        hit_test: impl Fn(Point) -> Option<usize>,
    ) -> Option<Action> {
        let auto_confirm = self.show_surface(ui, tokens, &view, hit_test);
        let mut action = auto_confirm.map(Action::Confirm);
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        } else if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(target) = self.selected
        {
            action = Some(Action::Confirm(target));
        }

        if !view.auto_start {
            egui::Area::new(egui::Id::unique("window-selector-toolbar"))
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

    pub fn show_surface(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: &View<'_>,
        hit_test: impl Fn(Point) -> Option<usize>,
    ) -> Option<SelectionTarget> {
        self.show_surface_with(ui, tokens, view, hit_test, false)
    }

    /// New Capture's window surface: the shipping guidance chip shows until a
    /// window is selected, switches to display copy over the desktop and ducks
    /// away from the pointer.
    pub fn show_menu_surface(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: &View<'_>,
        hit_test: impl Fn(Point) -> Option<usize>,
    ) -> Option<SelectionTarget> {
        self.show_surface_with(ui, tokens, view, hit_test, true)
    }

    fn show_surface_with(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: &View<'_>,
        hit_test: impl Fn(Point) -> Option<usize>,
        menu: bool,
    ) -> Option<SelectionTarget> {
        let surface = ui.max_rect();
        let ui = &mut crate::accessibility::group(
            ui,
            surface,
            "window-selector",
            capture_menu::WINDOW_SELECTOR_LABEL,
        );
        let coordinates = CoordinateMap::new(surface, view.display);
        let response = ui.allocate_rect(surface, Sense::click());
        if self.scripted
            && ui.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::PointerButton { .. } | egui::Event::Key { .. }
                    )
                })
            })
        {
            self.scripted = false;
        }
        if !self.scripted {
            if let Some(position) = response.hover_pos() {
                self.hovered = Some(match hit_test(coordinates.point(position)) {
                    Some(index) if index < view.windows.len() => SelectionTarget::Window(index),
                    _ => SelectionTarget::Display,
                });
            } else if ui.input(|input| input.pointer.latest_pos()).is_none() {
                self.hovered = None;
            }
        }

        // AppKit names the hovered target, or the display over the desktop.
        let target = selection_label(
            Some(
                self.presentation_target()
                    .unwrap_or(SelectionTarget::Display),
            ),
            view.windows,
        );
        crate::accessibility::set_value(ui, target);
        crate::accessibility::text(
            ui,
            "target",
            egui::Rect::from_min_size(surface.min, egui::Vec2::ZERO),
            &capture_menu::target_description(target),
            true,
        );
        paint_surface(
            ui,
            tokens,
            view,
            coordinates,
            self.presentation_target(),
            if menu {
                SurfaceGuidance::Menu {
                    hidden: matches!(self.selected, Some(SelectionTarget::Window(_))),
                    display: self.hovered == Some(SelectionTarget::Display),
                }
            } else {
                SurfaceGuidance::Direct {
                    has_selection: self.selected.is_some(),
                }
            },
        );

        if response.clicked()
            && let Some(target) = self.hovered
        {
            if view.auto_start {
                return Some(target);
            } else {
                self.selected = Some(target);
            }
        }
        None
    }

    pub fn exercise(&mut self, cycle: usize, hit_test: impl Fn(Point) -> Option<usize>) {
        let point = match cycle {
            0 => Some(Point { x: 210., y: 130. }),
            1 => Some(Point { x: 540., y: 250. }),
            2 => Some(Point { x: 500., y: 20. }),
            3 => Some(Point { x: 920., y: 680. }),
            4 => Some(Point { x: 800., y: 550. }),
            5 => {
                *self = Self::fixture();
                return;
            }
            _ => return,
        };
        self.hovered = point.map(|point| match hit_test(point) {
            Some(index) => SelectionTarget::Window(index),
            None => SelectionTarget::Display,
        });
        self.selected = self.hovered;
        self.scripted = true;
    }
}

/// New Capture's Full screen target: display outline and the shipping
/// `recording-display-identity` (name, size and Record FPS), no guidance chip.
pub fn show_display_surface(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &View<'_>,
    identity: &DisplayIdentity,
) -> bool {
    let surface = ui.max_rect();
    let coordinates = CoordinateMap::new(surface, view.display);
    let response = ui.allocate_rect(surface, Sense::click());
    paint_surface(
        ui,
        tokens,
        view,
        coordinates,
        Some(SelectionTarget::Display),
        SurfaceGuidance::None,
    );
    paint_display_identity(ui.painter(), tokens, surface, identity);
    response.clicked()
}

fn paint_display_identity(
    painter: &egui::Painter,
    tokens: &Tokens,
    surface: egui::Rect,
    identity: &DisplayIdentity,
) {
    let name = painter.layout_no_wrap(
        identity.name.clone(),
        FontId::proportional(tokens.number("text-2xl")),
        tokens.color("glass-text"),
    );
    let detail = painter.layout_no_wrap(
        identity.detail.clone(),
        FontId::proportional(tokens.number("text-md")),
        tokens.color("glass-text-muted"),
    );
    let gap = tokens.number("s-4");
    let height = name.size().y + gap + detail.size().y;
    // translate(-50%, -60%) around the display center.
    let top = surface.center().y - height * 0.6;
    let detail_top = top + height - detail.size().y;
    for (galley, y) in [(name, top), (detail, detail_top)] {
        let origin = Pos2::new(surface.center().x - galley.size().x / 2., y);
        // The shipping text-shadow keeps the label legible on bright desktops.
        painter.galley_with_override_text_color(
            origin + egui::vec2(0., 2.),
            galley.clone(),
            Color32::from_black_alpha(128),
        );
        painter.galley(origin, galley, Color32::WHITE);
    }
}

#[derive(Clone, Copy)]
enum SurfaceGuidance {
    /// Direct window overlay: persistent chip with its Enter hint.
    Direct {
        has_selection: bool,
    },
    /// New Capture: shipping chip, hidden once a window is selected.
    Menu {
        hidden: bool,
        display: bool,
    },
    None,
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
    guidance: SurfaceGuidance,
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
    let direct = matches!(guidance, SurfaceGuidance::Direct { .. });
    let display_target = matches!(guidance, SurfaceGuidance::None);
    // Shipping `CaptureDim`: window mode uses the stronger window shade and
    // the Full screen target the region shade.
    let window_shade = tokens.color("capture-shade-window");
    if let Some((rect, window)) = selected.filter(|(rect, _)| rect.is_positive()) {
        let radius = window
            .corner_radius
            .unwrap_or(0.)
            .clamp(0., f64::from(rect.width().min(rect.height())) / 2.) as f32;
        paint_shade_around(painter, surface, rect, radius, window_shade);
        let corner = radius.round().clamp(0., 255.) as u8;
        painter.rect_filled(
            rect,
            corner,
            tokens.color("theme-accent").gamma_multiply(0.14),
        );
        painter.rect_stroke(
            rect,
            corner,
            Stroke::new(2., tokens.color("theme-accent")),
            StrokeKind::Outside,
        );
        // `.window-target` clips its title chip to the rounded window bounds.
        let space = tokens.number("s-2");
        paint_chip(
            &painter.with_clip_rect(rect.intersect(painter.clip_rect())),
            tokens,
            rect.left_top() + egui::vec2(space, space),
            window_label(window),
            rect.width() - 8.,
        );
    } else if hovered == Some(SelectionTarget::Display) {
        if display_target {
            painter.rect_filled(surface, 0., tokens.color("capture-shade"));
        } else if !direct {
            painter.rect_filled(surface, 0., window_shade);
        }
        // `.capture-display-outline`: an inset 2 px accent ring at the display edge.
        painter.rect_stroke(
            surface,
            0.,
            Stroke::new(2., tokens.color("theme-accent")),
            StrokeKind::Inside,
        );
        if direct {
            // Shipping `.capture-display-fallback`; the direct overlay stays clear.
            let inset = tokens.number("s-4");
            paint_chip(
                painter,
                tokens,
                surface.min + egui::vec2(inset, inset),
                "Entire display",
                360_f32.min(surface.width() - 8.),
            );
        }
    } else if !direct {
        // New Capture's window target dims while it waits for a choice; the
        // direct screenshot overlay stays clear until something is hovered.
        painter.rect_filled(surface, 0., window_shade);
    }

    let has_selection = match guidance {
        SurfaceGuidance::Direct { has_selection } => has_selection,
        SurfaceGuidance::Menu { hidden, display } => {
            crate::capture_controls::paint_guidance(
                ui,
                tokens,
                surface,
                capture_menu::guidance(
                    if display {
                        GuidanceTarget::Display
                    } else {
                        GuidanceTarget::Window
                    },
                    false,
                ),
                hidden,
            );
            return;
        }
        SurfaceGuidance::None => return,
    };
    // Shipping CaptureGuidance: shell/desktop hover switches to display copy.
    let guidance = if hovered == Some(SelectionTarget::Display) {
        "Click to capture this display"
    } else {
        "Select a window to continue"
    };
    let title = painter.layout_no_wrap(
        guidance.into(),
        FontId::proportional(tokens.number("text-xl")),
        tokens.color("glass-text"),
    );
    let hint = painter.layout_no_wrap(
        if has_selection && !view.auto_start {
            "Esc to cancel · Press Enter to confirm"
        } else {
            "Esc to cancel"
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

/// Shipping glass title chip (`.window-target span`,
/// `.capture-display-fallback span`): one ellipsized line of `--text-xs`.
fn paint_chip(painter: &egui::Painter, tokens: &Tokens, origin: Pos2, title: &str, max_width: f32) {
    let padding = egui::vec2(tokens.number("s-3"), tokens.number("s-2"));
    let mut job = egui::text::LayoutJob::simple_singleline(
        title.to_owned(),
        FontId::proportional(tokens.number("text-xs")),
        tokens.color("glass-text"),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - 2. * padding.x).max(0.));
    let galley = painter.layout_job(job);
    let background = egui::Rect::from_min_size(origin, galley.size() + 2. * padding);
    painter.rect_filled(
        background,
        tokens.number("r-sm"),
        tokens.color("glass-strong"),
    );
    painter.galley(origin + padding, galley, Color32::WHITE);
}

/// The window shade with a hole that follows the window's rounded corners:
/// four strips around the bounds plus a fan in each corner outside the arc.
fn paint_shade_around(
    painter: &egui::Painter,
    surface: egui::Rect,
    hole: egui::Rect,
    radius: f32,
    color: Color32,
) {
    for outside in [
        egui::Rect::from_min_max(surface.min, Pos2::new(surface.right(), hole.top())),
        egui::Rect::from_min_max(Pos2::new(surface.left(), hole.bottom()), surface.max),
        egui::Rect::from_min_max(
            Pos2::new(surface.left(), hole.top()),
            Pos2::new(hole.left(), hole.bottom()),
        ),
        egui::Rect::from_min_max(
            Pos2::new(hole.right(), hole.top()),
            Pos2::new(surface.right(), hole.bottom()),
        ),
    ] {
        if outside.is_positive() {
            painter.rect_filled(outside, 0., color);
        }
    }
    if radius <= 0. {
        return;
    }
    painter.add(egui::Shape::mesh(rounded_corner_mesh(hole, radius, color)));
}

/// Triangles between each corner of `hole` and its quarter arc (y down).
fn rounded_corner_mesh(hole: egui::Rect, radius: f32, color: Color32) -> egui::Mesh {
    use std::f32::consts::{FRAC_PI_2, PI};
    const STEPS: u32 = 12;
    let mut mesh = egui::Mesh::default();
    for (corner, center, start) in [
        (
            hole.left_top(),
            hole.left_top() + egui::vec2(radius, radius),
            PI,
        ),
        (
            hole.right_top(),
            hole.right_top() + egui::vec2(-radius, radius),
            PI + FRAC_PI_2,
        ),
        (
            hole.right_bottom(),
            hole.right_bottom() - egui::vec2(radius, radius),
            0.,
        ),
        (
            hole.left_bottom(),
            hole.left_bottom() + egui::vec2(radius, -radius),
            FRAC_PI_2,
        ),
    ] {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(corner, color);
        for step in 0..=STEPS {
            let angle = start + FRAC_PI_2 * step as f32 / STEPS as f32;
            mesh.colored_vertex(
                center + radius * egui::vec2(angle.cos(), angle.sin()),
                color,
            );
        }
        for step in 0..STEPS {
            mesh.add_triangle(base, base + 1 + step, base + 2 + step);
        }
    }
    mesh
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
        Some(SelectionTarget::Display) => capture_menu::DISPLAY_TARGET,
        Some(SelectionTarget::Window(index)) => windows
            .get(index)
            .map(window_label)
            .unwrap_or("Selected window"),
        None => "Select a window to continue",
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
            egui::Id::unique("window-selector-input-test"),
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

    fn painted(hovered: Option<SelectionTarget>, guidance: SurfaceGuidance) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000., 720.));
        let display = display();
        let (windows, _) = targets();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, Vec::new()));
        let ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("window-selector-paint-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let view = View {
            frozen: None,
            display: &display,
            windows: &windows,
            auto_start: true,
        };
        paint_surface(
            &ui,
            &tokens,
            &view,
            CoordinateMap::new(screen, &display),
            hovered,
            guidance,
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect()
    }

    fn fills(shapes: &[egui::Shape], color: Color32) -> Vec<egui::Rect> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == color => Some(rect.rect),
                _ => None,
            })
            .collect()
    }

    fn texts(shapes: &[egui::Shape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn direct_overlay_stays_clear_until_hover_and_labels_the_display() {
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let window_shade = tokens.color("capture-shade-window");
        let region_shade = tokens.color("capture-shade");
        let direct = SurfaceGuidance::Direct {
            has_selection: false,
        };

        let idle = painted(None, direct);
        assert!(
            fills(&idle, window_shade).is_empty(),
            "direct overlay dims before hover"
        );
        assert!(fills(&idle, region_shade).is_empty());

        let display = painted(Some(SelectionTarget::Display), direct);
        assert!(
            fills(&display, window_shade).is_empty(),
            "desktop hover must stay clear"
        );
        assert!(texts(&display).iter().any(|text| text == "Entire display"));
        let chip = fills(&display, tokens.color("glass-strong"))
            .into_iter()
            .find(|rect| rect.top() < 20.)
            .expect("Entire display chip");
        let inset = tokens.number("s-4");
        assert_eq!(
            chip.min,
            Pos2::new(inset, inset),
            "chip sits at the --s-4 inset"
        );

        let window = painted(Some(SelectionTarget::Window(1)), direct);
        let strips = fills(&window, window_shade);
        assert_eq!(strips.len(), 4, "shade surrounds the hovered window");
        assert!(texts(&window).iter().any(|text| text == "front"));
        assert!(!texts(&window).iter().any(|text| text == "Entire display"));

        // New Capture keeps shipping `dimWithoutHole`; Full screen uses the region shade.
        let menu = SurfaceGuidance::Menu {
            hidden: false,
            display: false,
        };
        assert_eq!(fills(&painted(None, menu), window_shade).len(), 1);
        let full = painted(Some(SelectionTarget::Display), SurfaceGuidance::None);
        assert_eq!(fills(&full, region_shade).len(), 1);
        assert!(!texts(&full).iter().any(|text| text == "Entire display"));
    }

    fn covered(mesh: &egui::Mesh, point: Pos2) -> bool {
        mesh.indices.chunks(3).any(|triangle| {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize].pos);
            let side =
                |p: Pos2, q: Pos2| (q.x - p.x) * (point.y - p.y) - (q.y - p.y) * (point.x - p.x);
            let (ab, bc, ca) = (side(a, b), side(b, c), side(c, a));
            (ab >= 0. && bc >= 0. && ca >= 0.) || (ab <= 0. && bc <= 0. && ca <= 0.)
        })
    }

    #[test]
    fn rounded_window_hole_dims_each_corner_outside_the_arc_only() {
        let hole = egui::Rect::from_min_max(Pos2::new(100., 100.), Pos2::new(300., 200.));
        let mesh = rounded_corner_mesh(hole, 20., Color32::BLACK);
        for corner in [
            Pos2::new(102., 102.),
            Pos2::new(298., 102.),
            Pos2::new(298., 198.),
            Pos2::new(102., 198.),
        ] {
            assert!(covered(&mesh, corner), "corner {corner:?} left undimmed");
        }
        for inside in [
            Pos2::new(112., 112.),
            Pos2::new(288., 112.),
            Pos2::new(288., 188.),
            Pos2::new(112., 188.),
            Pos2::new(200., 150.),
            Pos2::new(130., 101.),
        ] {
            assert!(
                !covered(&mesh, inside),
                "{inside:?} inside the window was dimmed"
            );
        }
        for outside in [Pos2::new(99., 99.), Pos2::new(301., 150.)] {
            assert!(!covered(&mesh, outside), "{outside:?} is the strips' job");
        }
    }

    #[test]
    fn scripted_fixture_ignores_passive_pointer_until_real_input() {
        let ctx = egui::Context::default();
        let display = display();
        let (windows, shell) = targets();
        let mut selector = WindowSelector::fixture();
        selector.exercise(1, |point| {
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
        assert_eq!(selector.hovered(), Some(SelectionTarget::Window(1)));

        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(200., 130.))],
            false,
        );
        assert_eq!(
            selector.hovered(),
            Some(SelectionTarget::Window(1)),
            "the X11 startup pointer must not overwrite a scripted screenshot target"
        );

        run_input(
            &ctx,
            &mut selector,
            vec![pointer(egui::pos2(200., 130.), true)],
            false,
        );
        run_input(
            &ctx,
            &mut selector,
            vec![pointer(egui::pos2(200., 130.), false)],
            false,
        );
        assert_eq!(selector.hovered(), Some(SelectionTarget::Window(0)));
        assert_eq!(selector.selected(), Some(SelectionTarget::Window(0)));
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
    fn overlay_exposes_appkit_group_name_and_live_target() {
        use crate::accessibility::tests::{contains, find, find_value, tree};
        use egui::accesskit::{Live, Role};
        let ctx = egui::Context::default();
        let mut selector = WindowSelector::default();
        let display = display();
        let (windows, shell) = targets();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let mut frame = |events| {
            tree(&ctx, egui::vec2(1000., 720.), events, |ui| {
                selector.show(
                    ui,
                    &tokens,
                    View {
                        frozen: None,
                        display: &display,
                        windows: &windows,
                        auto_start: true,
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
            })
        };
        let idle = frame(vec![]);
        let (group, node) = find(&idle, "Capture window selector").expect("selector group");
        assert_eq!(node.role(), Role::Group);
        assert_eq!(node.value(), Some("Entire display"));
        let (target, text) = find_value(&idle, "Target: Entire display").expect("target");
        assert_eq!(text.role(), Role::Label);
        assert_eq!(text.live(), Some(Live::Polite));
        assert!(contains(&idle, group, target));

        let hovered = frame(vec![egui::Event::PointerMoved(egui::pos2(600., 300.))]);
        let (_, node) = find(&hovered, "Capture window selector").expect("selector group");
        assert_eq!(node.value(), Some("front"));
        assert!(find_value(&hovered, "Target: front").is_some());
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
