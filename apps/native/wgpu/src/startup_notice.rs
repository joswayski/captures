//! The shipping "Captures is ready to use" launch notice: a dark glass pill
//! whose triangle caret points at the tray icon. Placement is shared with the
//! AppKit host through `captures_app::tray_notice`.
use std::time::{Duration, Instant};

use captures_app::tray_notice::{
    self, Caret, FallbackEdge, LogicalRect, Placement, STARTUP_NOTICE_CARET_SPAN,
    STARTUP_NOTICE_HINT, STARTUP_NOTICE_TITLE,
};
use eframe::egui::{self, FontId, Stroke};

use crate::tokens::Tokens;

#[derive(Clone, Debug)]
pub struct Notice {
    pub placement: Placement,
    pub keys: Vec<String>,
    pub generation: u64,
    shown_at: Instant,
    expires_at: Instant,
    retry_until: Instant,
}

impl Notice {
    pub fn new(
        placement: Placement,
        keys: Vec<String>,
        generation: u64,
        visible_for: Duration,
        now: Instant,
    ) -> Self {
        let retry = if tray_notice::should_retry_tray_rect() {
            tray_notice::STARTUP_NOTICE_TRAY_RETRY_DELAY
                * tray_notice::STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS
        } else {
            Duration::ZERO
        };
        Self {
            placement,
            keys,
            generation,
            shown_at: now,
            expires_at: now + visible_for,
            retry_until: now + retry,
        }
    }

    /// Time since the notice appeared, for its entrance animation.
    pub fn elapsed_ms(&self, now: Instant) -> f64 {
        crate::motion::elapsed_ms(self.shown_at, now)
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.expires_at.saturating_duration_since(now)
    }

    /// The tray icon may not have a screen rect yet right after launch.
    pub fn wants_tray_retry(&self, now: Instant) -> bool {
        self.placement.caret == Caret::None && now < self.retry_until
    }
}

/// Physical tray rect `(x, y, width, height)` to a placement on the monitor
/// that contains it (else the primary or first monitor).
pub fn placement(
    window: Option<&winit::window::Window>,
    tray: Option<(f64, f64, f64, f64)>,
) -> Placement {
    let fallback = LogicalRect::new(0., 0., 1440., 900.);
    let Some(window) = window else {
        return resolve(fallback, fallback, None);
    };
    let monitors: Vec<_> = window.available_monitors().collect();
    let contains = |monitor: &winit::monitor::MonitorHandle, (x, y, w, h): (f64, f64, f64, f64)| {
        let (cx, cy) = (x + w / 2., y + h / 2.);
        let position = monitor.position();
        let size = monitor.size();
        cx >= f64::from(position.x)
            && cx <= f64::from(position.x) + f64::from(size.width)
            && cy >= f64::from(position.y)
            && cy <= f64::from(position.y) + f64::from(size.height)
    };
    let monitor = tray
        .and_then(|tray| monitors.iter().find(|m| contains(m, tray)).cloned())
        .or_else(|| window.primary_monitor())
        .or_else(|| monitors.first().cloned());
    let Some(monitor) = monitor else {
        return resolve(fallback, fallback, None);
    };
    let scale = monitor.scale_factor().max(1.);
    let position = monitor.position();
    let size = monitor.size();
    let full = crate::work_area::PhysicalRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let work = crate::work_area::for_monitor(full).unwrap_or(full);
    let logical = |x: f64, y: f64, w: f64, h: f64| {
        LogicalRect::new(x / scale, y / scale, w / scale, h / scale)
    };
    resolve(
        logical(
            f64::from(full.x),
            f64::from(full.y),
            f64::from(full.width),
            f64::from(full.height),
        ),
        logical(
            f64::from(work.x),
            f64::from(work.y),
            f64::from(work.width),
            f64::from(work.height),
        ),
        tray.map(|(x, y, w, h)| logical(x, y, w, h)),
    )
}

fn resolve(monitor: LogicalRect, work: LogicalRect, tray: Option<LogicalRect>) -> Placement {
    tray_notice::startup_notice_placement(
        monitor,
        work,
        tray,
        false,
        FallbackEdge::for_current_platform(),
    )
}

fn rect(rect: LogicalRect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(rect.x as f32, rect.y as f32),
        egui::vec2(rect.width as f32, rect.height as f32),
    )
}

pub fn size(placement: &Placement) -> egui::Vec2 {
    egui::vec2(placement.width as f32, placement.height as f32)
}

/// Close button bounds inside the notice window.
pub fn close_rect(tokens: &Tokens, placement: &Placement) -> egui::Rect {
    let card = rect(placement.card_rect());
    let side = tokens.number("h-sm");
    egui::Rect::from_center_size(
        egui::pos2(
            card.right() - tokens.number("s-3") - side / 2.,
            card.center().y,
        ),
        egui::vec2(side, side),
    )
}

/// Paints the notice at the window origin. Returns true when Close is clicked.
pub fn show(ui: &mut egui::Ui, tokens: &Tokens, notice: &Notice) -> bool {
    let placement = notice.placement;
    let card = rect(placement.card_rect());
    let fill = tokens.color("glass-strong-solid");
    let painter = ui.painter().clone();
    // `--tooltip-shadow` (drop-shadow 0 4px 10px, 32% black); the token
    // export only carries colors and plain numbers, not filter values.
    painter.add(
        egui::Shadow {
            offset: [0, 4],
            blur: 10,
            spread: 0,
            color: egui::Color32::from_black_alpha(82),
        }
        .as_shape(card, egui::CornerRadius::same((card.height() / 2.) as u8)),
    );
    if let Some(points) = placement.caret_triangle(STARTUP_NOTICE_CARET_SPAN) {
        painter.add(egui::Shape::convex_polygon(
            points
                .iter()
                .map(|(x, y)| egui::pos2(*x as f32, *y as f32))
                .collect(),
            fill,
            Stroke::NONE,
        ));
    }
    // `--r-pill` clamps to a full half-height end cap.
    let radius = tokens.number("r-pill").min(card.height() / 2.);
    painter.rect_filled(card, radius, fill);

    let text = tokens.color("glass-text");
    let subtle = tokens.color("glass-text-subtle");
    let title = painter.layout_no_wrap(
        STARTUP_NOTICE_TITLE.to_owned(),
        FontId::proportional(tokens.number("text-md")),
        text,
    );
    let small = FontId::proportional(tokens.number("text-sm"));
    let hint = painter.layout_no_wrap(STARTUP_NOTICE_HINT.to_owned(), small.clone(), subtle);
    let chip_pad = egui::vec2(tokens.number("s-1") + 1., 1.);
    let chips: Vec<_> = notice
        .keys
        .iter()
        .map(|key| painter.layout_no_wrap(key.clone(), small.clone(), text))
        .collect();
    let chip_gap = tokens.number("s-1");
    let chips_width: f32 = chips
        .iter()
        .map(|chip| chip.size().x + chip_pad.x * 2. + chip_gap)
        .sum();
    // A space follows the hint before the first chip's own margin.
    let space = tokens.number("s-2") - chip_gap;
    let row_height = chips
        .iter()
        .map(|chip| chip.size().y + chip_pad.y * 2.)
        .fold(hint.size().y, f32::max);
    let row_width = hint.size().x + space + chips_width;
    let gap = tokens.number("s-1");
    let content_height = title.size().y + gap + row_height;
    let top = card.center().y - content_height / 2.;
    let title_pos = egui::pos2(card.center().x - title.size().x / 2., top);
    let title_rect = egui::Rect::from_min_size(title_pos, title.size());
    painter.galley(title_pos, title, text);
    let row_top = top + title_rect.height() + gap;
    let mut x = card.center().x - row_width / 2.;
    painter.galley(
        egui::pos2(x, row_top + (row_height - hint.size().y) / 2.),
        hint.clone(),
        subtle,
    );
    x += hint.size().x + space;
    for chip in chips {
        x += chip_gap;
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(x, row_top + (row_height - chip.size().y) / 2. - chip_pad.y),
            chip.size() + chip_pad * 2.,
        );
        painter.rect(
            chip_rect,
            tokens.number("r-sm"),
            tokens.color("glass-hover"),
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        painter.galley(chip_rect.min + chip_pad, chip, text);
        x = chip_rect.right();
    }

    let close = close_rect(tokens, &placement);
    let response = ui.interact(
        close,
        ui.scope_id()
            .with(("startup-notice-close", notice.generation)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close"));
    let color = if response.hovered() || response.has_focus() {
        painter.circle_filled(
            close.center(),
            close.width() / 2.,
            tokens.color("glass-hover"),
        );
        text
    } else {
        subtle
    };
    // 14px `CloseIcon` (a 24-unit × with a 2-unit stroke), not a text glyph.
    let arm = 3.5;
    let stroke = Stroke::new(1.5, color);
    let c = close.center();
    painter.line_segment(
        [c + egui::vec2(-arm, -arm), c + egui::vec2(arm, arm)],
        stroke,
    );
    painter.line_segment(
        [c + egui::vec2(-arm, arm), c + egui::vec2(arm, -arm)],
        stroke,
    );
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        crate::tokens::load().remove("light-cobalt").unwrap()
    }

    fn notice(tray: Option<LogicalRect>, keys: &[&str]) -> Notice {
        let monitor = LogicalRect::new(0., 0., 1920., 1080.);
        let work = LogicalRect::new(0., 0., 1920., 1040.);
        Notice::new(
            tray_notice::startup_notice_placement(monitor, work, tray, false, FallbackEdge::Bottom),
            keys.iter().map(|key| (*key).to_owned()).collect(),
            1,
            tray_notice::STARTUP_NOTICE_AFTER_SETUP_VISIBLE,
            Instant::now(),
        )
    }

    fn render(
        notice: &Notice,
        frames: Vec<Vec<egui::Event>>,
    ) -> (bool, Vec<egui::epaint::ClippedShape>) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size(&notice.placement));
        let mut clicked = false;
        let mut shapes = Vec::new();
        for events in std::iter::once(Vec::new()).chain(frames) {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| clicked |= show(ui, &tokens(), notice),
            );
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        (clicked, shapes)
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn carets(shapes: &[egui::epaint::ClippedShape]) -> Vec<Vec<egui::Pos2>> {
        shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) if path.points.len() == 3 => Some(path.points.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn renders_the_shipping_copy_and_one_chip_per_shortcut_key() {
        let notice = notice(None, &["Ctrl", "Shift", "Space"]);
        let (_, shapes) = render(&notice, Vec::new());
        assert_eq!(
            texts(&shapes),
            [
                "Captures is ready to use",
                "Open New Capture with",
                "Ctrl",
                "Shift",
                "Space"
            ]
        );
        let chips = shapes
            .iter()
            .filter(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == tokens().color("glass-hover")
                        && rect.stroke.color == tokens().color("glass-border"))
            })
            .count();
        assert_eq!(chips, 3);
    }

    #[test]
    fn glass_palette_is_fixed_regardless_of_appearance() {
        let notice = notice(None, &["Ctrl"]);
        let (_, shapes) = render(&notice, Vec::new());
        let card = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect.width() == 296. && rect.blur_width == 0. => {
                    Some(rect.clone())
                }
                _ => None,
            })
            .expect("card");
        // A light theme still paints the dark media glass.
        assert_eq!(card.fill, tokens().color("glass-strong-solid"));
        assert_ne!(card.fill, tokens().color("surface-raised"));
        assert_eq!(
            card.corner_radius.nw as f32,
            (card.rect.height() / 2.).floor()
        );
    }

    #[test]
    fn caret_is_a_triangle_pointing_at_the_tray_only_when_anchored() {
        let fallback = notice(None, &["Ctrl"]);
        assert!(carets(&render(&fallback, Vec::new()).1).is_empty());

        let anchored = notice(Some(LogicalRect::new(1860., 1048., 24., 24.)), &["Ctrl"]);
        assert_eq!(anchored.placement.caret, Caret::Bottom);
        let shapes = render(&anchored, Vec::new()).1;
        let caret = carets(&shapes);
        assert_eq!(caret.len(), 1);
        let tip = caret[0][0];
        assert_eq!(tip.y, anchored.placement.height as f32);
        assert_eq!(tip.x, anchored.placement.caret_x as f32);
        // The base is flat and 12px wide: a real triangle, not a rotated square.
        assert_eq!(caret[0][1].y, caret[0][2].y);
        assert_eq!(caret[0][2].x - caret[0][1].x, 12.);
    }

    #[test]
    fn close_button_dismisses() {
        let notice = notice(None, &["Ctrl"]);
        let center = close_rect(&tokens(), &notice.placement).center();
        assert_eq!(
            center,
            egui::pos2(28. + 296. - 6. - 14., 28. + 27.),
            "right-aligned in the card"
        );
        let press = |pressed| egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let (clicked, _) = render(
            &notice,
            vec![
                vec![egui::Event::PointerMoved(center)],
                vec![press(true)],
                vec![press(false)],
            ],
        );
        assert!(clicked);
        let (missed, _) = render(
            &notice,
            [true, false]
                .into_iter()
                .map(|pressed| {
                    vec![
                        egui::Event::PointerMoved(egui::pos2(60., 55.)),
                        egui::Event::PointerButton {
                            pos: egui::pos2(60., 55.),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                })
                .collect(),
        );
        assert!(!missed);
    }

    #[test]
    fn lifetime_and_tray_retry_follow_the_shipping_timings() {
        let now = Instant::now();
        let quiet = Notice::new(
            notice(None, &[]).placement,
            Vec::new(),
            2,
            tray_notice::STARTUP_NOTICE_AUTOSTART_VISIBLE,
            now,
        );
        assert!(!quiet.expired(now + Duration::from_millis(4_999)));
        assert!(quiet.expired(now + Duration::from_secs(5)));
        assert_eq!(
            quiet.wants_tray_retry(now),
            tray_notice::should_retry_tray_rect()
        );
        assert!(!quiet.wants_tray_retry(now + Duration::from_secs(1)));
        let anchored = notice(Some(LogicalRect::new(1860., 1048., 24., 24.)), &[]);
        assert!(!anchored.wants_tray_retry(now));
    }
}
