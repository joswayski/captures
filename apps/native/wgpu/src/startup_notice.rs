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
    fallback_height: f32,
) -> Placement {
    let fallback = LogicalRect::new(0., 0., 1440., 900.);
    let Some(window) = window else {
        return resolve(fallback, fallback, None, fallback_height);
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
        return resolve(fallback, fallback, None, fallback_height);
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
        fallback_height,
    )
}

fn resolve(
    monitor: LogicalRect,
    work: LogicalRect,
    tray: Option<LogicalRect>,
    fallback_height: f32,
) -> Placement {
    let placement = tray_notice::startup_notice_placement(
        monitor,
        work,
        tray,
        false,
        FallbackEdge::for_current_platform(),
    );
    if placement.caret != Caret::None {
        return placement;
    }
    tray_notice::resolve_tray_notice_placement(
        monitor,
        work,
        None,
        false,
        FallbackEdge::for_current_platform(),
        tray_notice::STARTUP_NOTICE_WIDTH,
        f64::from(fallback_height),
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

struct Content {
    title: std::sync::Arc<egui::Galley>,
    hint: std::sync::Arc<egui::Galley>,
    chips: Vec<std::sync::Arc<egui::Galley>>,
    rows: Vec<egui::Rect>,
    width: f32,
    height: f32,
}

fn content(ctx: &egui::Context, tokens: &Tokens, keys: &[String], compact: bool) -> Content {
    let text = tokens.color("glass-text");
    let subtle = tokens.color(if compact {
        "glass-text-muted"
    } else {
        "glass-text-subtle"
    });
    let (title, hint, chips) = ctx.fonts_mut(|fonts| {
        let small = FontId::proportional(tokens.number("text-sm"));
        (
            fonts.layout_no_wrap(
                STARTUP_NOTICE_TITLE.into(),
                FontId::proportional(tokens.number("text-md")),
                text,
            ),
            fonts.layout_no_wrap(
                if keys.is_empty() {
                    "Open History from the tray menu"
                } else {
                    STARTUP_NOTICE_HINT
                }
                .into(),
                small.clone(),
                subtle,
            ),
            keys.iter()
                .map(|key| fonts.layout_no_wrap(key.clone(), small.clone(), text))
                .collect::<Vec<_>>(),
        )
    });
    let pad = egui::vec2(tokens.number("s-1") + 1., 1.);
    let sizes: Vec<_> = std::iter::once(hint.size())
        .chain(chips.iter().map(|chip| chip.size() + pad * 2.))
        .collect();
    let row_height = sizes.iter().map(|size| size.y).fold(0., f32::max);
    let available = tokens.number("startup-notice-width")
        - tokens.number("s-5")
        - tokens.number("h-sm")
        - tokens.number("s-5");
    let (mut x, mut y, mut width) = (0., 0., 0_f32);
    let mut rows = Vec::new();
    for (index, size) in sizes.into_iter().enumerate() {
        let gap = if index == 0 {
            0.
        } else if index == 1 {
            tokens.number("s-2")
        } else {
            tokens.number("s-1")
        };
        if compact && x > 0. && x + gap + size.x > available {
            x = 0.;
            y += row_height + tokens.number("s-1");
        } else {
            x += gap;
        }
        rows.push(egui::Rect::from_min_size(
            egui::pos2(x, y + (row_height - size.y) / 2.),
            size,
        ));
        x += size.x;
        width = width.max(x);
    }
    let height = title.size().y + tokens.number("s-1") + y + row_height;
    Content {
        title,
        hint,
        chips,
        rows,
        width,
        height,
    }
}

/// Measure before creating the native window, so a long shortcut never clips
/// its first frame. Anchored notices retain the shipping fixed-height pill.
pub fn fallback_height(ctx: &egui::Context, tokens: &Tokens, keys: &[String]) -> f32 {
    (content(ctx, tokens, keys, true).height + tokens.number("s-4") * 2.)
        .max(tokens.number("startup-notice-height"))
}

/// Paints the notice at the window origin. Returns true when Close is clicked.
pub fn show(ui: &mut egui::Ui, tokens: &Tokens, notice: &Notice) -> bool {
    let placement = notice.placement;
    let card = rect(placement.card_rect());
    let compact = placement.caret == Caret::None;
    let fill = tokens.color("glass-strong-solid");
    let painter = ui.painter().clone();
    let radius = if compact {
        tokens.number("r-lg")
    } else {
        tokens.number("r-pill").min(card.height() / 2.)
    };
    if compact {
        crate::effects::paint_box_shadows(
            &painter,
            card,
            radius,
            tokens.shadow("tooltip-shadow-compact"),
            1.,
        );
    } else {
        // Keep the existing anchored pill's silhouette and shadow.
        painter.add(
            egui::Shadow {
                offset: [0, 4],
                blur: 10,
                spread: 0,
                color: egui::Color32::from_black_alpha(82),
            }
            .as_shape(card, egui::CornerRadius::same((card.height() / 2.) as u8)),
        );
    }
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
    painter.rect(
        card,
        radius,
        fill,
        if compact {
            Stroke::new(1., tokens.color("glass-border"))
        } else {
            Stroke::NONE
        },
        egui::StrokeKind::Inside,
    );

    let text = tokens.color("glass-text");
    let subtle = tokens.color(if compact {
        "glass-text-muted"
    } else {
        "glass-text-subtle"
    });
    let content = content(ui.ctx(), tokens, &notice.keys, compact);
    let chip_pad = egui::vec2(tokens.number("s-1") + 1., 1.);
    let gap = tokens.number("s-1");
    let top = card.center().y - content.height / 2.;
    let left = card.left() + tokens.number("s-5");
    let title_pos = egui::pos2(
        if compact {
            left
        } else {
            card.center().x - content.title.size().x / 2.
        },
        top,
    );
    let row_origin = egui::vec2(
        if compact {
            left
        } else {
            card.center().x - content.width / 2.
        },
        top + content.title.size().y + gap,
    );
    painter.galley(title_pos, content.title, text);
    painter.galley(content.rows[0].min + row_origin, content.hint, subtle);
    for (chip, chip_rect) in content
        .chips
        .into_iter()
        .zip(content.rows.into_iter().skip(1))
    {
        let chip_rect = chip_rect.translate(row_origin);
        painter.rect(
            chip_rect,
            tokens.number("r-sm"),
            tokens.color("glass-hover"),
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        painter.galley(chip_rect.min + chip_pad, chip, text);
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
    fn absent_global_shortcut_points_to_history_instead() {
        let (_, shapes) = render(&notice(None, &[]), Vec::new());
        assert_eq!(
            texts(&shapes),
            [
                "Captures is ready to use",
                "Open History from the tray menu"
            ]
        );
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
        assert_eq!(card.corner_radius.nw, 10);
        assert_eq!(card.stroke, Stroke::new(1., tokens().color("glass-border")));
        assert_eq!(
            tokens().color("glass-strong-solid"),
            crate::tokens::load()["dark-cobalt"].color("glass-strong-solid")
        );
    }

    #[test]
    fn fallback_measures_wrapped_keys_before_placement_and_reserves_close_space() {
        let tokens = tokens();
        let ctx = egui::Context::default();
        let normal = ["Ctrl", "Shift", "Space"].map(String::from);
        let long = ["Ctrl", "Alt", "Shift", "Super", "F12"].map(String::from);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(fallback_height(ui.ctx(), &tokens, &normal), 54.);
            let measured = content(ui.ctx(), &tokens, &long, true);
            let height = fallback_height(ui.ctx(), &tokens, &long);
            assert!(height > 54., "a long chord grows rather than clipping");
            let monitor = LogicalRect::new(-1440., -100., 1440., 900.);
            let work = LogicalRect::new(-1440., -100., 1440., 840.);
            let placement = resolve(monitor, work, None, height);
            let card = rect(placement.card_rect());
            assert_eq!(card.width(), 296.);
            assert_eq!(card.height(), height);
            assert!(placement.y >= work.y);
            assert!(placement.y + placement.height <= work.y + work.height);
            let close = close_rect(&tokens, &placement);
            let left = card.left() + 12.;
            let top = card.center().y - measured.height / 2.;
            assert!(top >= card.top() + 8.);
            assert!(top + measured.height <= card.bottom() - 8.);
            assert!(measured.rows.last().unwrap().top() > measured.rows[0].top());
            for row in measured.rows {
                assert!(row.right() + left <= card.right() - 40.);
                assert!(row.right() + left < close.left());
            }
            let anchored = resolve(
                monitor,
                work,
                Some(LogicalRect::new(-200., 750., 24., 24.)),
                height,
            );
            assert_eq!(anchored.caret, Caret::Bottom);
            assert_eq!(
                anchored.card_rect().height,
                55.,
                "anchored geometry is unchanged"
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn fallback_left_aligns_but_real_tray_anchors_keep_centered_text() {
        for tray in [None, Some(LogicalRect::new(1860., 1048., 24., 24.))] {
            let notice = notice(tray, &["Ctrl", "Shift", "Space"]);
            let (_, shapes) = render(&notice, Vec::new());
            let title = shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == STARTUP_NOTICE_TITLE => {
                        Some(text)
                    }
                    _ => None,
                })
                .unwrap();
            let card = rect(notice.placement.card_rect());
            if tray.is_none() {
                assert_eq!(title.pos.x, card.left() + 12.);
            } else {
                assert_eq!(title.pos.x + title.galley.size().x / 2., card.center().x);
            }
        }
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
