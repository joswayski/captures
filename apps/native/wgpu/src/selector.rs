use captures_app::selection::{self, Bounds, DragMode, DragOptions, Point, Rect};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, RichText, Sense, Stroke, StrokeKind, TextureHandle,
};

use crate::tokens::Tokens;

const MINIMUM_SIZE: f64 = 16.;
const HANDLE_RADIUS: f32 = 7.;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Aspect {
    #[default]
    Free,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    NineSixteen,
}

impl Aspect {
    const ALL: [Self; 6] = [
        Self::Free,
        Self::Square,
        Self::FourThree,
        Self::ThreeTwo,
        Self::SixteenNine,
        Self::NineSixteen,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Square => "1 : 1",
            Self::FourThree => "4 : 3",
            Self::ThreeTwo => "3 : 2",
            Self::SixteenNine => "16 : 9",
            Self::NineSixteen => "9 : 16",
        }
    }

    fn ratio(self) -> Option<f64> {
        match self {
            Self::Free => None,
            Self::Square => Some(1.),
            Self::FourThree => Some(4. / 3.),
            Self::ThreeTwo => Some(3. / 2.),
            Self::SixteenNine => Some(16. / 9.),
            Self::NineSixteen => Some(9. / 16.),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    mode: DragMode,
    origin: Point,
    initial: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Confirm,
    Cancel,
}

#[derive(Default)]
pub struct Selector {
    rect: Option<Rect>,
    aspect: Aspect,
    drag: Option<Drag>,
}

impl Selector {
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    pub fn can_confirm(&self) -> bool {
        self.drag.is_none() && selection::capturable(self.rect)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn clear_selection(&mut self) {
        self.rect = None;
        self.drag = None;
    }

    pub fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.rect = selection::capturable(Some(drag.initial)).then_some(drag.initial);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        frozen: Option<&TextureHandle>,
        auto_start: bool,
        overlay_bounds: Option<Bounds>,
    ) -> Option<Action> {
        let bounds = overlay_bounds.unwrap_or(Bounds {
            width: ui.max_rect().width().into(),
            height: ui.max_rect().height().into(),
        });
        let mut action = self.show_surface(ui, tokens, frozen, auto_start, overlay_bounds);
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        } else if ui.input(|input| input.key_pressed(egui::Key::Enter)) && self.can_confirm() {
            action = Some(Action::Confirm);
        }

        let panel = egui::Area::new(egui::Id::unique("region-selector-toolbar"))
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
                            self.show_aspect_picker(ui, tokens, bounds);
                            if ui
                                .add_enabled(
                                    self.can_confirm(),
                                    egui::Button::new(
                                        RichText::new("Capture region")
                                            .color(tokens.color("theme-accent-ink")),
                                    )
                                    .fill(tokens.color("theme-accent"))
                                    .stroke(Stroke::NONE),
                                )
                                .on_hover_text("Capture region (Enter)")
                                .clicked()
                            {
                                action = Some(Action::Confirm);
                            }
                        });
                    });
            });
        if panel.response.contains_pointer() && ui.input(|input| input.pointer.any_pressed()) {
            self.drag = None;
        }
        action
    }

    pub fn show_surface(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        frozen: Option<&TextureHandle>,
        auto_start: bool,
        overlay_bounds: Option<Bounds>,
    ) -> Option<Action> {
        let surface = ui.max_rect();
        let bounds = overlay_bounds.unwrap_or(Bounds {
            width: surface.width().into(),
            height: surface.height().into(),
        });
        let coordinates = CoordinateMap { surface, bounds };
        let response = ui.allocate_rect(surface, Sense::click_and_drag());
        let pointer = response
            .interact_pointer_pos()
            .map(|position| coordinates.point(position));

        if response.drag_started()
            && let Some(point) = ui
                .input(|input| input.pointer.press_origin())
                .map(|position| coordinates.point(position))
        {
            self.begin(point);
        }
        if (response.dragged() || response.drag_stopped())
            && let Some(point) = pointer
        {
            let shift = ui.input(|input| input.modifiers.shift);
            self.update(point, bounds, shift);
        }
        let drag_stopped = response.drag_stopped();
        let created = self.drag_creates_selection();
        let capturable = drag_stopped && self.end();
        let auto_confirm = drag_stopped && created && capturable && auto_start;

        paint_surface(
            ui,
            coordinates,
            tokens,
            frozen,
            self.rect,
            self.drag.is_some(),
        );
        auto_confirm.then_some(Action::Confirm)
    }

    pub fn show_aspect_picker(&mut self, ui: &mut egui::Ui, tokens: &Tokens, bounds: Bounds) {
        ui.label(
            RichText::new("Aspect")
                .small()
                .color(tokens.color("glass-text-subtle")),
        );
        let previous = self.aspect;
        egui::ComboBox::from_id_salt("region-aspect")
            .selected_text(self.aspect.label())
            .show_ui(ui, |ui| {
                for aspect in Aspect::ALL {
                    ui.selectable_value(&mut self.aspect, aspect, aspect.label());
                }
            });
        if previous != self.aspect {
            self.apply_aspect(bounds);
        }
    }

    fn begin(&mut self, point: Point) {
        let (mode, initial) = self.rect.map_or(
            (
                DragMode::Create,
                Rect {
                    x: point.x,
                    y: point.y,
                    width: 0.,
                    height: 0.,
                },
            ),
            |rect| (hit_test(rect, point), rect),
        );
        if matches!(mode, DragMode::Create) {
            self.rect = Some(initial);
        }
        self.drag = Some(Drag {
            mode,
            origin: point,
            initial,
        });
    }

    fn update(&mut self, point: Point, bounds: Bounds, shift: bool) {
        let Some(drag) = self.drag else { return };
        self.rect = Some(selection::drag(
            drag.mode,
            drag.origin,
            point,
            drag.initial,
            bounds,
            DragOptions {
                minimum_size: MINIMUM_SIZE,
                aspect_ratio: self.aspect.ratio(),
                force_square: shift,
            },
        ));
    }

    fn end(&mut self) -> bool {
        self.drag = None;
        selection::capturable(self.rect)
    }

    fn drag_creates_selection(&self) -> bool {
        self.drag
            .is_some_and(|drag| matches!(drag.mode, DragMode::Create))
    }

    fn apply_aspect(&mut self, bounds: Bounds) {
        if let Some(rect) = self.rect {
            self.rect = Some(selection::constrain(
                rect,
                self.aspect.ratio(),
                Some(bounds),
                MINIMUM_SIZE,
            ));
        }
    }

    pub fn exercise(&mut self, cycle: usize, bounds: Bounds) {
        match cycle {
            0 => {
                self.begin(Point { x: 120., y: 90. });
                self.update(Point { x: 520., y: 330. }, bounds, false);
                self.end();
            }
            1 => {
                self.begin(Point { x: 300., y: 200. });
                self.update(Point { x: 360., y: 240. }, bounds, false);
                self.end();
            }
            2 => {
                let rect = self.rect.expect("drawn fixture selection");
                self.begin(Point {
                    x: rect.x + rect.width,
                    y: rect.y + rect.height,
                });
                self.update(
                    Point {
                        x: rect.x + rect.width + 110.,
                        y: rect.y + rect.height + 45.,
                    },
                    bounds,
                    false,
                );
                self.end();
            }
            3 => {
                self.aspect = Aspect::SixteenNine;
                self.apply_aspect(bounds);
            }
            4 => {
                let rect = self.rect.expect("aspect fixture selection");
                self.begin(Point {
                    x: rect.x + rect.width,
                    y: rect.y + rect.height,
                });
                self.update(
                    Point {
                        x: rect.x + rect.width - 80.,
                        y: rect.y + rect.height + 70.,
                    },
                    bounds,
                    true,
                );
                self.end();
            }
            5 => self.reset(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
struct CoordinateMap {
    surface: egui::Rect,
    bounds: Bounds,
}

impl CoordinateMap {
    fn point(self, point: Pos2) -> Point {
        Point {
            x: f64::from(point.x - self.surface.left()) * self.bounds.width
                / f64::from(self.surface.width()),
            y: f64::from(point.y - self.surface.top()) * self.bounds.height
                / f64::from(self.surface.height()),
        }
    }

    fn rect(self, rect: Rect) -> egui::Rect {
        egui::Rect::from_min_size(
            self.surface.min
                + egui::vec2(
                    (rect.x / self.bounds.width * f64::from(self.surface.width())) as f32,
                    (rect.y / self.bounds.height * f64::from(self.surface.height())) as f32,
                ),
            egui::vec2(
                (rect.width / self.bounds.width * f64::from(self.surface.width())) as f32,
                (rect.height / self.bounds.height * f64::from(self.surface.height())) as f32,
            ),
        )
        .intersect(self.surface)
    }
}

fn hit_test(rect: Rect, point: Point) -> DragMode {
    let near = |x: f64, y: f64| (point.x - x).hypot(point.y - y) <= HANDLE_RADIUS as f64 * 2.;
    if near(rect.x, rect.y) {
        DragMode::Nw
    } else if near(rect.x + rect.width, rect.y) {
        DragMode::Ne
    } else if near(rect.x, rect.y + rect.height) {
        DragMode::Sw
    } else if near(rect.x + rect.width, rect.y + rect.height) {
        DragMode::Se
    } else if point.x >= rect.x
        && point.x <= rect.x + rect.width
        && point.y >= rect.y
        && point.y <= rect.y + rect.height
    {
        DragMode::Move
    } else {
        DragMode::Create
    }
}

fn paint_surface(
    ui: &egui::Ui,
    coordinates: CoordinateMap,
    tokens: &Tokens,
    frozen: Option<&TextureHandle>,
    selection: Option<Rect>,
    dragging: bool,
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
    let selected = selection.map(|rect| coordinates.rect(rect));
    let veil = tokens.color("glass-veil-heavy");
    if let Some(rect) = selected.filter(|rect| rect.is_positive()) {
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
        painter.rect_stroke(
            rect,
            2.,
            Stroke::new(1.5, tokens.color("theme-accent")),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            rect.shrink(1.5),
            1.,
            Stroke::new(1., Color32::from_white_alpha(72)),
            StrokeKind::Inside,
        );
        for position in [
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
        ] {
            painter.circle_filled(position, 5., tokens.color("theme-accent"));
            painter.circle_stroke(
                position,
                5.,
                Stroke::new(2., tokens.color("theme-accent-ink")),
            );
        }
        let dimensions = selection
            .map(|rect| format!("{} × {}", rect.width.round(), rect.height.round()))
            .unwrap_or_default();
        let label_center = if rect.top() < 30. {
            rect.center_top() + egui::vec2(0., 17.)
        } else {
            rect.center_top() - egui::vec2(0., 14.)
        };
        let galley = painter.layout_no_wrap(
            dimensions,
            FontId::proportional(tokens.number("text-sm")),
            tokens.color("glass-text"),
        );
        painter.rect_filled(
            egui::Rect::from_center_size(label_center, galley.size() + egui::vec2(12., 6.)),
            tokens.number("r-sm"),
            tokens.color("glass-strong"),
        );
        painter.galley(label_center - galley.size() / 2., galley, Color32::WHITE);
    } else if !dragging {
        painter.rect_filled(surface, 0., veil);
        painter.text(
            surface.center() - egui::vec2(0., 28.),
            Align2::CENTER_CENTER,
            "Drag to select a region",
            FontId::proportional(tokens.number("text-xl")),
            tokens.color("glass-text"),
        );
        painter.text(
            surface.center(),
            Align2::CENTER_CENTER,
            "Hold Shift for a square · Esc to cancel",
            FontId::proportional(tokens.number("text-sm")),
            tokens.color("glass-text-muted"),
        );
    } else {
        painter.rect_filled(surface, 0., veil);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Bounds = Bounds {
        width: 800.,
        height: 600.,
    };

    fn assert_rect(actual: Rect, expected: [f64; 4]) {
        for (actual, expected) in [actual.x, actual.y, actual.width, actual.height]
            .into_iter()
            .zip(expected)
        {
            assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
        }
    }

    fn raw(screen: egui::Rect, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        }
    }

    fn pointer(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn run_input(
        ctx: &egui::Context,
        selector: &mut Selector,
        events: Vec<egui::Event>,
        auto_start: bool,
    ) -> Option<Action> {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 600.));
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, events));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("selector-input-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = selector.show(&mut ui, &tokens, None, auto_start, None);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn starts_blank_then_uses_shared_geometry_for_create_move_and_resize() {
        let mut selector = Selector::default();
        assert_eq!(selector.rect(), None);
        selector.begin(Point { x: 100., y: 80. });
        selector.update(Point { x: 300., y: 180. }, BOUNDS, false);
        assert!(selector.end());
        assert_rect(selector.rect().unwrap(), [100., 80., 200., 100.]);

        selector.begin(Point { x: 150., y: 120. });
        selector.update(Point { x: 220., y: 150. }, BOUNDS, false);
        selector.end();
        assert_rect(selector.rect().unwrap(), [170., 110., 200., 100.]);

        selector.begin(Point { x: 370., y: 210. });
        selector.update(Point { x: 430., y: 250. }, BOUNDS, false);
        selector.end();
        assert_rect(selector.rect().unwrap(), [170., 110., 260., 140.]);
    }

    #[test]
    fn all_shipping_aspects_constrain_and_shift_release_restores_the_preset() {
        assert_eq!(Aspect::ALL.len(), 6);
        let mut selector = Selector {
            rect: Some(Rect {
                x: 100.,
                y: 100.,
                width: 320.,
                height: 240.,
            }),
            aspect: Aspect::SixteenNine,
            drag: None,
        };
        selector.apply_aspect(BOUNDS);
        let preset = selector.rect().unwrap();
        assert!((preset.width / preset.height - 16. / 9.).abs() < 1e-9);
        selector.begin(Point {
            x: preset.x + preset.width,
            y: preset.y + preset.height,
        });
        let pointer = Point { x: 500., y: 400. };
        selector.update(pointer, BOUNDS, true);
        let square = selector.rect().unwrap();
        assert!((square.width - square.height).abs() < 1e-9);
        selector.update(pointer, BOUNDS, false);
        let restored = selector.rect().unwrap();
        assert!((restored.width / restored.height - 16. / 9.).abs() < 1e-9);
    }

    #[test]
    fn reset_cancels_selection_and_active_drag() {
        let mut selector = Selector::default();
        selector.begin(Point { x: 10., y: 20. });
        selector.update(Point { x: 80., y: 90. }, BOUNDS, false);
        selector.reset();
        assert_eq!(selector.rect(), None);
        assert!(selector.drag.is_none());
        assert_eq!(selector.aspect, Aspect::Free);
    }

    #[test]
    fn display_change_clears_selection_but_preserves_aspect() {
        let mut selector = Selector {
            rect: Some(Rect {
                x: 10.,
                y: 20.,
                width: 100.,
                height: 80.,
            }),
            aspect: Aspect::SixteenNine,
            drag: None,
        };
        selector.begin(Point { x: 30., y: 40. });
        selector.clear_selection();
        assert_eq!(selector.rect, None);
        assert!(selector.drag.is_none());
        assert_eq!(selector.aspect, Aspect::SixteenNine);
    }

    #[test]
    fn auto_start_only_follows_a_completed_create_not_move_or_resize() {
        let mut selector = Selector::default();
        selector.begin(Point { x: 100., y: 80. });
        assert!(selector.drag_creates_selection());
        selector.update(Point { x: 300., y: 180. }, BOUNDS, false);
        selector.end();

        selector.begin(Point { x: 150., y: 120. });
        assert!(!selector.drag_creates_selection(), "move is adjust-only");
        selector.end();

        selector.begin(Point { x: 300., y: 180. });
        assert!(!selector.drag_creates_selection(), "resize is adjust-only");
    }

    #[test]
    fn raw_input_uses_press_origin_and_only_create_auto_starts() {
        let ctx = egui::Context::default();
        let mut selector = Selector::default();
        run_input(&ctx, &mut selector, vec![], true);
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(50., 60.)),
                pointer(egui::pos2(50., 60.), true, egui::Modifiers::NONE),
            ],
            true,
        );
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(250., 160.))],
            true,
        );
        let action = run_input(
            &ctx,
            &mut selector,
            vec![pointer(
                egui::pos2(250., 160.),
                false,
                egui::Modifiers::NONE,
            )],
            true,
        );
        assert_eq!(action, Some(Action::Confirm));
        assert_rect(selector.rect().unwrap(), [50., 60., 200., 100.]);

        // Crossing egui's drag threshold away from the handle must still use
        // the original corner press for hit-testing, and adjustment never auto-starts.
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(250., 160.)),
                pointer(egui::pos2(250., 160.), true, egui::Modifiers::NONE),
            ],
            true,
        );
        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::PointerMoved(egui::pos2(280., 190.))],
            true,
        );
        let action = run_input(
            &ctx,
            &mut selector,
            vec![pointer(
                egui::pos2(280., 190.),
                false,
                egui::Modifiers::NONE,
            )],
            true,
        );
        assert_eq!(action, None);
        assert_rect(selector.rect().unwrap(), [50., 60., 230., 130.]);
    }

    #[test]
    fn raw_input_blocks_enter_while_dragging_and_updates_stationary_shift() {
        let ctx = egui::Context::default();
        let mut selector = Selector {
            aspect: Aspect::SixteenNine,
            ..Default::default()
        };
        run_input(&ctx, &mut selector, vec![], false);
        run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(100., 100.)),
                pointer(egui::pos2(100., 100.), true, egui::Modifiers::NONE),
            ],
            false,
        );
        let action = run_input(
            &ctx,
            &mut selector,
            vec![
                egui::Event::PointerMoved(egui::pos2(300., 200.)),
                key(egui::Key::Enter, true, egui::Modifiers::NONE),
            ],
            false,
        );
        assert_eq!(action, None);
        let preset = selector.rect().unwrap();
        assert!((preset.width / preset.height - 16. / 9.).abs() < 1e-9);

        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)],
            false,
        );
        let square = selector.rect().unwrap();
        assert!((square.width - square.height).abs() < 1e-9);

        run_input(
            &ctx,
            &mut selector,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
            false,
        );
        let restored = selector.rect().unwrap();
        assert!((restored.width / restored.height - 16. / 9.).abs() < 1e-9);
        run_input(
            &ctx,
            &mut selector,
            vec![pointer(
                egui::pos2(300., 200.),
                false,
                egui::Modifiers::NONE,
            )],
            false,
        );
    }

    #[test]
    fn asymmetric_viewport_ratios_map_to_overlay_coordinates_and_back() {
        let map = CoordinateMap {
            surface: egui::Rect::from_min_size(egui::pos2(20., 30.), egui::vec2(1000., 500.)),
            bounds: Bounds {
                width: 2000.,
                height: 1500.,
            },
        };
        let point = map.point(egui::pos2(120., 130.));
        assert_eq!((point.x, point.y), (200., 300.));
        let painted = map.rect(Rect {
            x: 200.,
            y: 300.,
            width: 400.,
            height: 600.,
        });
        assert_eq!(painted.min, egui::pos2(120., 130.));
        assert_eq!(painted.size(), egui::vec2(200., 200.));
    }
}
