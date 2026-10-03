//! Shipping `CompressionPreview` (`.compression-preview-frame.is-cover`) over
//! an editor's media: the encoded After side clipped right of a draggable
//! divider, the Before/After size badges, a Hide pill, and the Processing veil
//! while a new encode runs. Both editors paint their own Before (the live
//! canvas or the source frame) first; `captures_app::compression_compare`
//! owns the copy, the badges and the split bounds.

use captures_app::compression_compare::{self as compare, Badges};
use eframe::egui::{self, Rect, Stroke, StrokeKind};

use crate::tokens::Tokens;

/// Height of the full-width split strip along the frame's bottom edge
/// (`.compression-preview-range`).
const RANGE_HEIGHT: f32 = 28.;
/// `AFTER_HINT_PAD` and the hint's pointer offsets.
const HINT_PAD: f32 = 8.;

pub struct Overlay<'a> {
    pub id: egui::Id,
    /// The whole media rect the split is measured against.
    pub frame: Rect,
    /// The visible part of the viewport.
    pub clip: Rect,
    pub after: Option<&'a egui::TextureHandle>,
    pub badges: Badges,
    pub processing: bool,
    pub error: Option<&'a str>,
    /// The bottom strip also moves the split (off while a drawing tool is
    /// selected, so strokes can start there).
    pub range_enabled: bool,
    /// Cursor-following hint on the After side (a drawing tool is selected).
    pub after_hint: Option<&'a str>,
}

#[derive(Default)]
pub struct Output {
    pub dismissed: bool,
    pub split_changed: bool,
    pub handle: Option<Rect>,
    pub dismiss: Option<Rect>,
    pub range: Option<Rect>,
}

fn visible(frame: Rect, clip: Rect) -> Option<Rect> {
    let visible = frame.intersect(clip);
    (visible.width() >= 1. && visible.height() >= 1.).then_some(visible)
}

fn split_x(frame: Rect, split: f64) -> f32 {
    frame.left() + frame.width() * compare::clamp_split(split) as f32
}

/// Whether comparison chrome owns this frame's press, drag or release.
/// Read the registered widget responses before handling raw canvas events,
/// which otherwise bypass egui's same-layer widget hit tests.
pub fn owns_pointer(ctx: &egui::Context, id: egui::Id) -> bool {
    ["handle", "range", "dismiss"].into_iter().any(|control| {
        ctx.read_response(id.with(control)).is_some_and(|response| {
            response.is_pointer_button_down_on() || response.clicked() || response.drag_stopped()
        })
    })
}

/// Paint the encoded After image right of the divider. Call this right after
/// the Before media so editor overlays (selection, crop) stay above it.
pub fn paint_after(
    ui: &egui::Ui,
    frame: Rect,
    clip: Rect,
    after: Option<&egui::TextureHandle>,
    split: f64,
) {
    let (Some(after), Some(visible)) = (after, visible(frame, clip)) else {
        return;
    };
    let x = split_x(frame, split);
    let side = Rect::from_min_max(egui::pos2(x, visible.top()), visible.max);
    if side.width() <= 0. {
        return;
    }
    // `object-fit: fill`: the file's pixels stretch over the Before box.
    ui.painter().with_clip_rect(side).image(
        after.id(),
        frame,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        egui::Color32::WHITE,
    );
}

fn pill_galley(
    ui: &egui::Ui,
    tokens: &Tokens,
    text: &str,
    accent: Option<&str>,
    size: &str,
) -> std::sync::Arc<egui::Galley> {
    let font = egui::FontId::proportional(tokens.number(size));
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.,
        egui::TextFormat::simple(font.clone(), tokens.color("glass-text")),
    );
    if let Some(accent) = accent {
        job.append(
            accent,
            0.,
            egui::TextFormat::simple(font, tokens.color("positive-text")),
        );
    }
    ui.painter().layout_job(job)
}

fn paint_pill(ui: &egui::Ui, tokens: &Tokens, rect: Rect, hovered: bool) {
    ui.painter().rect(
        rect,
        rect.height() / 2.,
        tokens.color(if hovered {
            "glass-raised"
        } else {
            "glass-strong"
        }),
        Stroke::new(
            1.,
            tokens.color(if hovered {
                "glass-border-strong"
            } else {
                "glass-border"
            }),
        ),
        StrokeKind::Inside,
    );
}

/// Shipping `compressionAfterHintPosition`: keep the hint inside the frame.
pub fn after_hint_position(pointer: egui::Pos2, frame: Rect, hint: egui::Vec2) -> egui::Pos2 {
    let width = hint.x.min((frame.width() - HINT_PAD * 2.).max(1.));
    let height = hint.y.min((frame.height() - HINT_PAD * 2.).max(1.));
    let local = pointer - frame.min;
    let mut x = local.x + 12.;
    let mut y = local.y + 14.;
    if x + width + HINT_PAD > frame.width() {
        x = local.x - width - HINT_PAD;
    }
    if y + height + HINT_PAD > frame.height() {
        y = local.y - height - HINT_PAD;
    }
    frame.min
        + egui::vec2(
            x.max(HINT_PAD).min(frame.width() - width - HINT_PAD),
            y.max(HINT_PAD).min(frame.height() - height - HINT_PAD),
        )
}

/// Shipping's focused range input (`step` 0.1 %, Page Up/Down a tenth of
/// the span, Home/End its ends); off while it is disabled for drawing. The
/// arrows stay on the split instead of moving egui's focus.
fn keyboard_split(ui: &egui::Ui, split: f64, ids: [egui::Id; 2]) -> Option<f64> {
    let focused = ids
        .into_iter()
        .find(|id| ui.memory(|memory| memory.has_focus(*id)))?;
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            focused,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        );
    });
    let keys = [
        (egui::Key::ArrowLeft, compare::SplitKey::Decrease),
        (egui::Key::ArrowDown, compare::SplitKey::Decrease),
        (egui::Key::ArrowRight, compare::SplitKey::Increase),
        (egui::Key::ArrowUp, compare::SplitKey::Increase),
        (egui::Key::PageDown, compare::SplitKey::PageDown),
        (egui::Key::PageUp, compare::SplitKey::PageUp),
        (egui::Key::Home, compare::SplitKey::Home),
        (egui::Key::End, compare::SplitKey::End),
    ];
    let mut next = split;
    for (key, step) in keys {
        let presses = ui.input_mut(|input| input.count_and_consume_key(egui::Modifiers::NONE, key));
        for _ in 0..presses {
            next = compare::keyboard_split(next, step);
        }
    }
    let actions = ui.input(|input| {
        use egui::accesskit::Action;
        (
            input.num_accesskit_action_requests(focused, Action::Increment),
            input.num_accesskit_action_requests(focused, Action::Decrement),
        )
    });
    for _ in 0..actions.0 {
        next = compare::keyboard_split(next, compare::SplitKey::Increase);
    }
    for _ in 0..actions.1 {
        next = compare::keyboard_split(next, compare::SplitKey::Decrease);
    }
    (next != split).then_some(next)
}

/// The divider, handle, badges, Hide pill, Processing veil and error. Call
/// after the editor's own canvas interactions so the handle, the bottom strip
/// and Hide take the pointer before them.
pub fn show_chrome(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    overlay: Overlay<'_>,
    split: &mut f64,
) -> Output {
    let mut output = Output::default();
    let Some(visible) = visible(overlay.frame, overlay.clip) else {
        return output;
    };
    let painter = ui.painter().with_clip_rect(visible);
    let inset = tokens.number("s-5");
    let show_split = overlay.after.is_some();
    let reduced = crate::motion::reduced(ui.ctx());

    if show_split {
        let mut focused = false;
        // `.compression-preview-range`: a full-width strip on the bottom edge.
        let range = Rect::from_min_max(
            egui::pos2(visible.left(), visible.bottom() - RANGE_HEIGHT),
            visible.max,
        );
        if overlay.range_enabled && !overlay.processing {
            let response = ui
                .interact(
                    range,
                    overlay.id.with("range"),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, compare::RANGE_LABEL)
            });
            if (response.clicked() || response.dragged())
                && let Some(pointer) = response.interact_pointer_pos()
            {
                response.request_focus();
                *split = compare::split_at(
                    f64::from(pointer.x - overlay.frame.left()),
                    f64::from(overlay.frame.width()),
                );
                output.split_changed = true;
            }
            focused |= response.has_focus();
            output.range = Some(range);
        }
        let x = split_x(overlay.frame, *split);
        // `.compression-preview-divider`: 2 px glass text with a dark edge.
        painter.line_segment(
            [
                egui::pos2(x, visible.top()),
                egui::pos2(x, visible.bottom()),
            ],
            Stroke::new(4., egui::Color32::from_black_alpha(89)),
        );
        painter.line_segment(
            [
                egui::pos2(x, visible.top()),
                egui::pos2(x, visible.bottom()),
            ],
            Stroke::new(2., tokens.color("glass-text")),
        );
        let size = compare::HANDLE_SIZE as f32;
        let handle =
            Rect::from_center_size(egui::pos2(x, visible.center().y), egui::Vec2::splat(size));
        let response = ui
            .interact(
                handle,
                overlay.id.with("handle"),
                if overlay.processing {
                    egui::Sense::hover()
                } else {
                    egui::Sense::click_and_drag()
                },
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Slider,
                !overlay.processing,
                compare::HANDLE_LABEL,
            )
        });
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            response.request_focus();
            *split = compare::split_at(
                f64::from(pointer.x - overlay.frame.left()),
                f64::from(overlay.frame.width()),
            );
            output.split_changed = true;
        } else if response.clicked() {
            // A press without a drag still focuses the split for the keys.
            response.request_focus();
        }
        focused |= response.has_focus();
        let handle_id = response.id;
        if focused
            && !overlay.processing
            && overlay.range_enabled
            && let Some(next) = keyboard_split(ui, *split, [handle_id, overlay.id.with("range")])
        {
            *split = next;
            output.split_changed = true;
        }
        let x = split_x(overlay.frame, *split);
        let handle =
            Rect::from_center_size(egui::pos2(x, visible.center().y), egui::Vec2::splat(size));
        painter.add(crate::preferences_widgets::shadow_sm(true).as_shape(handle, size / 2.));
        painter.circle(
            handle.center(),
            size / 2. - 0.5,
            tokens.color("glass-strong"),
            Stroke::new(1., tokens.color("glass-border-strong")),
        );
        painter.text(
            handle.center(),
            egui::Align2::CENTER_CENTER,
            compare::HANDLE_GLYPH,
            egui::FontId::proportional(tokens.number("text-sm")),
            tokens.color("glass-text"),
        );
        if focused {
            crate::primitives::focus_ring(ui, tokens, handle, size / 2.);
        }
        output.handle = Some(handle);
    }

    if overlay.processing {
        // `.compression-preview-veil` and the centred status pill.
        painter.rect_filled(visible, 0., tokens.color("glass-veil"));
        let galley = pill_galley(ui, tokens, compare::PROCESSING, None, "text-sm");
        let spinner = 16.;
        let gap = tokens.number("s-4");
        let pad = egui::vec2(tokens.number("s-6"), tokens.number("s-4"));
        let pill = Rect::from_center_size(
            visible.center(),
            egui::vec2(
                spinner + gap + galley.size().x,
                spinner.max(galley.size().y),
            ) + pad * 2.,
        );
        paint_pill(ui, tokens, pill, false);
        let centre = egui::pos2(pill.left() + pad.x + spinner / 2., pill.center().y);
        let color = tokens.color("glass-text");
        if reduced {
            painter.circle_stroke(centre, spinner / 2. - 1., Stroke::new(2., color));
        } else {
            // `preferences-status-spin 0.72s linear infinite` on a 3/4 ring.
            let time = ui.input(|input| input.time);
            let start = (time / 0.72).fract() as f32 * std::f32::consts::TAU;
            let points: Vec<_> = (0..=24)
                .map(|step| {
                    let angle = start + step as f32 / 24. * std::f32::consts::TAU * 0.75;
                    centre + egui::vec2(angle.cos(), angle.sin()) * (spinner / 2. - 1.)
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(2., color)));
            ui.ctx().request_repaint();
        }
        painter.galley(
            egui::pos2(
                centre.x + spinner / 2. + gap,
                pill.center().y - galley.size().y / 2.,
            ),
            galley,
            color,
        );
    }

    // `.compression-preview-badge`: bottom corners, tabular sizes.
    let pad = egui::vec2(tokens.number("s-4"), tokens.number("s-2"));
    for (text, savings, align) in [
        (
            overlay.badges.before.as_str(),
            None,
            egui::Align2::LEFT_BOTTOM,
        ),
        (
            overlay.badges.after.as_str(),
            overlay.badges.savings.as_deref(),
            egui::Align2::RIGHT_BOTTOM,
        ),
    ] {
        let galley = pill_galley(ui, tokens, text, savings, "text-xs");
        let anchor = if align == egui::Align2::LEFT_BOTTOM {
            visible.left_bottom() + egui::vec2(inset, -inset)
        } else {
            visible.right_bottom() + egui::vec2(-inset, -inset)
        };
        let badge = align.anchor_size(anchor, galley.size() + pad * 2.);
        paint_pill(ui, tokens, badge, false);
        painter.galley(badge.min + pad, galley, egui::Color32::PLACEHOLDER);
    }

    // `.compression-preview-dismiss`: Hide, top right.
    let galley = pill_galley(ui, tokens, compare::DISMISS, None, "text-xs");
    let dismiss = egui::Align2::RIGHT_TOP.anchor_size(
        visible.right_top() + egui::vec2(-inset, inset),
        galley.size() + pad * 2.,
    );
    let response = ui
        .interact(dismiss, overlay.id.with("dismiss"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, compare::DISMISS_LABEL)
    });
    paint_pill(ui, tokens, dismiss, response.hovered());
    painter.galley(dismiss.min + pad, galley, egui::Color32::PLACEHOLDER);
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, dismiss, dismiss.height() / 2.);
    }
    output.dismissed = response.clicked();
    output.dismiss = Some(dismiss);

    if let Some(error) = overlay.error {
        // `.compression-preview-error`: a strip across the top.
        let margin = tokens.number("s-4");
        let width = visible.width() - margin * 2.;
        let galley = ui.painter().layout(
            error.to_owned(),
            egui::FontId::proportional(tokens.number("text-sm")),
            tokens.color("danger-text"),
            (width - tokens.number("s-4") * 2.).max(1.),
        );
        let strip = Rect::from_min_size(
            visible.min + egui::vec2(margin, margin),
            egui::vec2(width, galley.size().y + tokens.number("s-3") * 2.),
        );
        painter.rect_filled(strip, tokens.number("r-md"), tokens.color("danger-surface"));
        painter.galley(
            strip.min + egui::vec2(tokens.number("s-4"), tokens.number("s-3")),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }

    if let (Some(hint), true, false) = (overlay.after_hint, show_split, overlay.processing)
        && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
        && visible.contains(pointer)
        && pointer.x > split_x(overlay.frame, *split)
    {
        let galley = ui.painter().layout(
            hint.to_owned(),
            egui::FontId::proportional(tokens.number("text-xs")),
            tokens.color("glass-text"),
            220. - 16.,
        );
        let size = galley.size() + egui::vec2(16., 10.);
        let min = after_hint_position(pointer, visible, size);
        let rect = Rect::from_min_size(min, size);
        let layer = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            overlay.id.with("hint"),
        ));
        layer.rect(
            rect,
            tokens.number("r-sm"),
            tokens.color("glass-strong"),
            Stroke::new(1., tokens.color("glass-border")),
            StrokeKind::Inside,
        );
        layer.galley(
            rect.min + egui::vec2(8., 5.),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        processing: bool,
        split: &mut f64,
        events: Vec<egui::Event>,
    ) -> (Output, egui::FullOutput) {
        run_with(ctx, texture, processing, true, split, events)
    }

    fn run_with(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        processing: bool,
        range_enabled: bool,
        split: &mut f64,
        events: Vec<egui::Event>,
    ) -> (Output, egui::FullOutput) {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let mut shown = Output::default();
        let mut full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 400.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = Rect::from_min_size(egui::pos2(50., 50.), egui::vec2(400., 300.));
                shown = show_chrome(
                    ui,
                    &tokens,
                    Overlay {
                        id: egui::Id::unique("compare-test"),
                        frame,
                        clip: frame,
                        after: Some(texture),
                        badges: compare::badges(Some(2_000), Some(500), processing),
                        processing,
                        error: None,
                        range_enabled,
                        after_hint: None,
                    },
                    split,
                );
            },
        );
        full.textures_delta.clear();
        (shown, full)
    }

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn handle_strip_and_hide_follow_the_pointer_and_processing_locks_the_split() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "after",
            egui::ColorImage::new([2, 2], vec![egui::Color32::RED; 4]),
            egui::TextureOptions::default(),
        );
        let mut split = compare::DEFAULT_SPLIT;
        let (shown, full) = run(&ctx, &texture, false, &mut split, vec![]);
        let handle = shown.handle.unwrap();
        assert_eq!(handle.center(), egui::pos2(250., 200.));
        let labels = texts(&full);
        assert!(labels.contains(&"Before · 2.0 KB".to_owned()));
        assert!(labels.contains(&"After · 500 B · 75% smaller".to_owned()));
        assert!(labels.contains(&compare::DISMISS.to_owned()));
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Drag the handle far left: the split stops at the shipping 6 %.
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![egui::Event::PointerMoved(handle.center())],
        );
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![press(handle.center(), true)],
        );
        for x in [200., 100., 10.] {
            run(
                &ctx,
                &texture,
                false,
                &mut split,
                vec![egui::Event::PointerMoved(egui::pos2(x, 200.))],
            );
        }
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![press(egui::pos2(10., 200.), false)],
        );
        assert_eq!(split, compare::MIN_SPLIT);
        // The bottom strip also moves it.
        let strip = shown.range.unwrap().center();
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![egui::Event::PointerMoved(strip)],
        );
        run(&ctx, &texture, false, &mut split, vec![press(strip, true)]);
        run(&ctx, &texture, false, &mut split, vec![press(strip, false)]);
        assert!((split - 0.5).abs() < 0.01, "{split}");
        // Processing: badges wait and the split cannot move.
        let (locked, full) = run(&ctx, &texture, true, &mut split, vec![]);
        assert!(locked.range.is_none());
        assert!(texts(&full).contains(&"After · Processing…".to_owned()));
        assert!(texts(&full).contains(&compare::PROCESSING.to_owned()));
        let hide = shown.dismiss.unwrap().center();
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![egui::Event::PointerMoved(hide)],
        );
        run(&ctx, &texture, false, &mut split, vec![press(hide, true)]);
        let (clicked, _) = run(&ctx, &texture, false, &mut split, vec![press(hide, false)]);
        assert!(clicked.dismissed);
    }

    #[test]
    fn focused_split_takes_the_shipping_range_keys_only_while_enabled() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "after",
            egui::ColorImage::new([2, 2], vec![egui::Color32::RED; 4]),
            egui::TextureOptions::default(),
        );
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut split = compare::DEFAULT_SPLIT;
        let (shown, _) = run(&ctx, &texture, false, &mut split, vec![]);
        let handle = shown.handle.unwrap().center();
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![egui::Event::PointerMoved(handle)],
        );
        run(&ctx, &texture, false, &mut split, vec![press(handle, true)]);
        run(
            &ctx,
            &texture,
            false,
            &mut split,
            vec![press(handle, false)],
        );
        assert_eq!(
            split,
            compare::DEFAULT_SPLIT,
            "a click on the handle keeps the split"
        );
        // The next frame takes focus and locks the arrows to the split.
        run(&ctx, &texture, false, &mut split, vec![]);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let step = |events, expected: f64, split: &mut f64| {
            let (output, _) = run(&ctx, &texture, false, split, events);
            assert!(close(*split, expected), "{split} != {expected}");
            output
        };
        let output = step(vec![key(egui::Key::ArrowRight)], 0.501, &mut split);
        assert!(output.split_changed);
        step(vec![key(egui::Key::ArrowDown)], 0.5, &mut split);
        step(
            vec![key(egui::Key::ArrowUp), key(egui::Key::ArrowUp)],
            0.502,
            &mut split,
        );
        step(vec![key(egui::Key::PageUp)], 0.59, &mut split);
        step(vec![key(egui::Key::PageDown)], 0.502, &mut split);
        step(vec![key(egui::Key::End)], compare::MAX_SPLIT, &mut split);
        step(vec![key(egui::Key::Home)], compare::MIN_SPLIT, &mut split);
        step(
            vec![key(egui::Key::ArrowLeft)],
            compare::MIN_SPLIT,
            &mut split,
        );
        // The arrows kept focus on the split rather than moving it on.
        step(vec![key(egui::Key::End)], compare::MAX_SPLIT, &mut split);
        // A drawing tool disables the range; its keys no longer move the split.
        let (output, _) = run_with(
            &ctx,
            &texture,
            false,
            false,
            &mut split,
            vec![key(egui::Key::Home)],
        );
        assert!(!output.split_changed);
        assert_eq!(split, compare::MAX_SPLIT);
        // So does a running encode.
        run(&ctx, &texture, true, &mut split, vec![key(egui::Key::Home)]);
        assert_eq!(split, compare::MAX_SPLIT);
    }

    #[test]
    fn after_hint_stays_inside_the_frame() {
        let frame = Rect::from_min_size(egui::pos2(100., 50.), egui::vec2(400., 300.));
        let hint = egui::vec2(220., 40.);
        assert_eq!(
            after_hint_position(egui::pos2(150., 80.), frame, hint),
            egui::pos2(162., 94.)
        );
        // Near the right and bottom edges the hint flips to the other side.
        let flipped = after_hint_position(egui::pos2(480., 330.), frame, hint);
        assert_eq!(flipped, egui::pos2(480. - 220. - 8., 330. - 40. - 8.));
        let rect = Rect::from_min_size(flipped, hint);
        assert!(frame.shrink(HINT_PAD - 0.01).contains_rect(rect));
    }
}
