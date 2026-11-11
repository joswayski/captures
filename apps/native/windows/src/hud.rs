//! Recording controls (`RecordingHud`, `styles/capture.css` `.recording-hud`):
//! the privacy line, the status dot, elapsed time and state, and Stop,
//! Pause/Resume, Restart, Screenshot, the microphone level and Mute, Delete
//! and Hide. Control state, copy and icons come from
//! `captures_app::recording_hud::present`; this module only lays them out and
//! draws them, shared by the live window and the offscreen renderer.
//! Coordinates are DIPs from the 430×102 window's corner.
//!
//! Tooltips follow the pointer over any button, enabled or not, like
//! shipping's hovered `.recording-tooltip` wrapper; they appear without the
//! `--dur-1` fade. [`Hud::parts`] lists what UI Automation exposes
//! (`hud_uia`). The window never activates over the app being recorded, so
//! the controls have no Tab focus yet (shipping's webview focus is not ported).

use captures_app::capture_menu::{self, MenuMode};
use captures_app::recording_hud::{self as hud, Control};
use captures_recording::RecordingState;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use crate::gfx::{self, Gfx, Style, rect};
use crate::tokens::Tokens;

type Rect = D2D_RECT_F;

/// The shipping HUD window; the card sits inside its 6 px margin with the
/// error line below.
pub const SIZE: (f32, f32) = (430., 102.);
const CARD: Rect = D2D_RECT_F {
    left: 6.,
    top: 6.,
    right: 424.,
    bottom: 76.,
};

/// The HUD's accessible parts in shipping DOM order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// `.recording-hud-privacy`.
    Privacy,
    /// The elapsed `<strong>`.
    Time,
    /// The state `<small>`.
    Status,
    Button(Control),
    /// `.recording-microphone-level`, between Screenshot and Mute.
    Level,
    /// `.recording-hud-error` (`role="alert"`).
    Error,
}

pub struct Hud {
    pub input: hud::Input,
    /// Elapsed recording time, including the running segment.
    pub elapsed_ms: u64,
    /// Microphone peak, 0–1.
    pub level: f32,
    pub error: Option<String>,
    /// The controls are excluded from recordings (Preferences).
    pub excluded: bool,
    pub hover: Option<Control>,
    pub pressed: Option<Control>,
    /// The button under the pointer, enabled or not, whose tooltip shows.
    pub tooltip: Option<Control>,
}

impl Hud {
    pub fn new(excluded: bool) -> Self {
        Self {
            input: hud::Input {
                state: RecordingState::Countdown,
                busy: true,
                has_microphone: false,
                microphone_muted: false,
                hide_available: true,
            },
            elapsed_ms: 0,
            level: 0.,
            error: None,
            excluded,
            hover: None,
            pressed: None,
            tooltip: None,
        }
    }

    /// The action buttons and the level meter, laid out right to left inside
    /// the card's 8 px padding (`.recording-hud-actions`, gap `--s-2`).
    fn layout(&self) -> (Vec<(Control, Rect)>, Option<Rect>) {
        let meter = self.input.has_microphone;
        let (cy, mut x) = (CARD.top + 1. + 6. + 13. + 2. + 19., CARD.right - 1. - 8.);
        let mut buttons = Vec::new();
        let mut level = None;
        for control in Control::ALL.into_iter().rev() {
            let w = if control == Control::Stop { 36. } else { 34. };
            buttons.push((control, rect(x - w, cy - 17., x, cy + 17.)));
            x -= w + 4.;
            if control == Control::Hide {
                // `.recording-hide { margin-left: var(--s-2) }`
                x -= 4.;
            }
            if control == Control::Microphone && meter {
                // `.recording-microphone-level`: 34×4 with 4 px side margins,
                // the only flex item that shrinks. The row overflows the card
                // after the 96 px status column and its 12 px gap, so the
                // meter gives up the difference as in shipping.
                let rest = (36. + 34. * 3. + 4. * 3.) + 8. + 4.;
                let room = x - (CARD.left + 1. + 8. + 96. + 12.);
                let w = (room - rest).clamp(0., 34.);
                level = Some(rect(x - 4. - w, cy - 2., x - 4., cy + 2.));
                x -= w + 8. + 4.;
            }
        }
        buttons.reverse();
        (buttons, level)
    }

    pub fn view(&self) -> hud::View {
        hud::present(&self.input)
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Control> {
        let view = self.view();
        self.layout()
            .0
            .into_iter()
            .find(|(control, r)| {
                view.control(*control).enabled
                    && x >= r.left
                    && x < r.right
                    && y >= r.top
                    && y < r.bottom
            })
            .map(|(control, _)| control)
    }

    /// The button under the pointer, including disabled ones (the tooltip
    /// target).
    pub fn under(&self, x: f32, y: f32) -> Option<Control> {
        self.layout()
            .0
            .into_iter()
            .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|(control, _)| control)
    }

    /// The accessible parts with their rects, in reading order.
    pub fn parts(&self) -> Vec<(Part, Rect)> {
        let cy = CARD.top + 1. + 6. + 13. + 2. + 19.;
        let left = CARD.left + 1. + 8. + 11. + 8.;
        let mut parts = vec![
            (
                Part::Privacy,
                rect(
                    CARD.left + 1.,
                    CARD.top + 7.,
                    CARD.right - 1.,
                    CARD.top + 20.,
                ),
            ),
            (
                Part::Time,
                rect(left, cy - 17., CARD.left + 1. + 8. + 96., cy + 3.),
            ),
            (
                Part::Status,
                rect(left, cy + 3., CARD.left + 1. + 8. + 96., cy + 17.),
            ),
        ];
        let (buttons, level) = self.layout();
        for (control, r) in buttons {
            if control == Control::Microphone
                && let Some(level) = level
            {
                parts.push((Part::Level, level));
            }
            parts.push((Part::Button(control), r));
        }
        if self.error.is_some() {
            parts.push((
                Part::Error,
                rect(10., CARD.bottom + 5., SIZE.0 - 10., SIZE.1),
            ));
        }
        parts
    }

    /// A part's accessible name: shipping's `aria-label`s and text.
    pub fn part_name(&self, part: Part) -> String {
        match part {
            Part::Privacy => {
                let note = capture_menu::visibility_note(MenuMode::Recording, true, self.excluded);
                let hint = if self.excluded {
                    ""
                } else {
                    " · Use Hide controls to keep them out"
                };
                format!("{}{}{}{hint}", note.lead, note.emphasis, note.trail)
            }
            Part::Time => clock(self.elapsed_ms),
            Part::Status => self.view().status_label.to_string(),
            Part::Button(control) => self.view().control(control).label.to_string(),
            Part::Level => format!(
                "Microphone level {}%",
                (self.level.clamp(0., 1.) * 100.).round()
            ),
            Part::Error => self
                .error
                .as_deref()
                .map(hud::error_message)
                .unwrap_or_default(),
        }
    }

    /// Over the card, where a drag moves the window (shipping `cursor: grab`).
    pub fn over_card(x: f32, y: f32) -> bool {
        (CARD.left..CARD.right).contains(&x) && (CARD.top..CARD.bottom).contains(&y)
    }

    /// `pulse` is the `recording-pulse` phase, 0–1 (0 at rest).
    pub fn paint(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens, pulse: f32) -> Result<()> {
        let view = self.view();
        gfx::fill_round(dc, CARD, 18., t.color("glass-strong"))?;
        gfx::border(dc, CARD, 18., 1., t.color("glass-border"))?;
        // `.recording-hud-privacy`: "These controls **won’t** show in recordings".
        let note = capture_menu::visibility_note(MenuMode::Recording, true, self.excluded);
        let lead = g.text(t, Style::Tiny, note.lead.trim_end(), 10_000.)?;
        let space = g.text(t, Style::Tiny, "a a", 10_000.)?.width
            - 2. * g.text(t, Style::Tiny, "a", 10_000.)?.width;
        let strong = g.text_sized(t, note.emphasis, 10., "weight-bold", None)?;
        // Shipping's `showHideHint`: included controls point at Hide.
        let trail = if self.excluded {
            note.trail.clone()
        } else {
            format!("{} · Use Hide controls to keep them out", note.trail)
        };
        let trail = g.text(t, Style::Tiny, &trail, 10_000.)?;
        let total = lead.width + space + strong.width + trail.width;
        let mut x = (CARD.left + CARD.right - total) / 2.;
        let row = CARD.top + 1. + 6. + 6.5;
        let subtle = t.color("glass-text-subtle");
        gfx::text(dc, &lead, x, row - lead.height / 2., subtle)?;
        x += lead.width + space;
        gfx::text(
            dc,
            &strong,
            x,
            row - strong.height / 2.,
            t.color("glass-text"),
        )?;
        x += strong.width;
        gfx::text(dc, &trail, x, row - trail.height / 2., subtle)?;
        // `.recording-hud-status`: dot, time and the uppercased state.
        let cy = CARD.top + 1. + 6. + 13. + 2. + 19.;
        let left = CARD.left + 1. + 8.;
        let dot = t.color(view.dot_token);
        let (scale, opacity) = if view.pulsing {
            let wave = (1. - (pulse * std::f32::consts::TAU).cos()) / 2.;
            (1. - 0.16 * wave, 1. - 0.4 * wave)
        } else {
            (1., 1.)
        };
        let (dx, r) = (left + 5.5, 5. * scale);
        if view.dot_halo {
            let halo = rect(
                dx - r - 4. * scale,
                cy - r - 4. * scale,
                dx + r + 4. * scale,
                cy + r + 4. * scale,
            );
            gfx::fill_round(dc, halo, 9. * scale, alpha(dot, 0.16 * opacity))?;
        }
        gfx::fill_round(
            dc,
            rect(dx - r, cy - r, dx + r, cy + r),
            r,
            alpha(dot, opacity),
        )?;
        let time = g.text_sized(
            t,
            &clock(self.elapsed_ms),
            t.n("text-lg"),
            "weight-semibold",
            Some("tracking-tight"),
        )?;
        let status = g.text_sized(
            t,
            &view.status_label.to_uppercase(),
            t.n("text-2xs"),
            "weight-semibold",
            Some("tracking-wide"),
        )?;
        let tx = left + 11. + 8.;
        let top = cy - (time.height + status.height) / 2.;
        gfx::text(dc, &time, tx, top, t.color("glass-text"))?;
        gfx::text(dc, &status, tx, top + time.height, subtle)?;
        let (buttons, level) = self.layout();
        if let Some(m) = level {
            gfx::fill_round(
                dc,
                m,
                2.,
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.4,
                },
            )?;
            let w = (m.right - m.left) * self.level.clamp(0., 1.);
            if w > 0. {
                gfx::fill_round(
                    dc,
                    rect(m.left, m.top, m.left + w, m.bottom),
                    2.,
                    t.color("positive"),
                )?;
            }
        }
        let signal = t.color("theme-signal");
        let white = D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 1.,
        };
        for (control, r) in buttons {
            let state = view.control(control);
            let hovered = state.enabled
                && self.hover == Some(control)
                && self.pressed.is_none_or(|p| p == control);
            let fade = if state.enabled { 1. } else { 0.32 };
            if control == Control::Stop {
                // `.recording-stop`: a signal-tinted square button.
                if hovered {
                    gfx::fill_round(dc, r, 8., alpha(signal, fade))?;
                } else {
                    gfx::fill_round(dc, r, 8., alpha(signal, 0.14 * fade))?;
                    gfx::border(dc, r, 8., 1., alpha(signal, 0.4 * fade))?;
                }
                let (sx, sy) = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
                let square = rect(sx - 5.5, sy - 5.5, sx + 5.5, sy + 5.5);
                let ink = if hovered { white } else { signal };
                gfx::fill_round(dc, square, 2., alpha(ink, fade))?;
                continue;
            }
            let destructive = control == Control::Delete && hovered;
            if destructive {
                gfx::fill_round(dc, r, 8., signal)?;
            } else if hovered {
                gfx::fill_round(dc, r, 8., t.color("glass-hover"))?;
            }
            if state.selected {
                gfx::border(dc, r, 8., 1., t.color_alpha("theme-accent", 0.3 * fade))?;
            }
            let color = if destructive {
                white
            } else if state.selected {
                t.color("theme-accent")
            } else if hovered {
                t.color("glass-text")
            } else {
                t.color("glass-text-muted")
            };
            if let Some(icon) = state.icon {
                let (ix, iy) = ((r.left + r.right) / 2. - 8., (r.top + r.bottom) / 2. - 8.);
                gfx::icon(
                    dc,
                    &g.factory,
                    icon,
                    &[],
                    (ix, iy, 16.),
                    1.8,
                    alpha(color, fade),
                )?;
            }
        }
        // `.recording-hud-error` below the card.
        if let Some(error) = &self.error {
            let text = g.text(t, Style::Tiny, &hud::error_message(error), 10_000.)?;
            g.ellipsis(&text, SIZE.0 - 20.)?;
            gfx::text(
                dc,
                &text,
                10.,
                CARD.bottom + 5.,
                t.color("theme-signal-text"),
            )?;
        }
        self.paint_tooltip(dc, g, t, &view)
    }

    /// Shipping `.recording-tooltip > [role="tooltip"]` under the hovered
    /// button, placed by the shared `tooltip_frame` inside the window.
    fn paint_tooltip(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        view: &hud::View,
    ) -> Result<()> {
        let Some(control) = self.tooltip else {
            return Ok(());
        };
        let Some(anchor) = self
            .layout()
            .0
            .into_iter()
            .find(|(c, _)| *c == control)
            .map(|(_, r)| r)
        else {
            return Ok(());
        };
        let copy = view.control(control);
        let pad = (hud::TOOLTIP_PADDING_X as f32, hud::TOOLTIP_PADDING_Y as f32);
        let text = g.text_sized(t, copy.tooltip, t.n("text-xs"), "weight-medium", None)?;
        let inner = hud::TOOLTIP_MAX_WIDTH as f32 - pad.0 * 2. - 2.;
        if text.width > inner {
            g.ellipsis(&text, inner)?;
        }
        let frame = hud::tooltip_frame(
            captures_app::tray_notice::LogicalRect::new(
                anchor.left.into(),
                anchor.top.into(),
                (anchor.right - anchor.left).into(),
                (anchor.bottom - anchor.top).into(),
            ),
            text.width.min(inner).into(),
            text.height.into(),
            copy.tooltip_right_aligned,
            1.,
            captures_app::tray_notice::LogicalRect::new(0., 0., SIZE.0.into(), SIZE.1.into()),
        );
        let r = rect(
            frame.x as f32,
            frame.y as f32,
            (frame.x + frame.width) as f32,
            (frame.y + frame.height) as f32,
        );
        let radius = t.n("r-sm");
        gfx::fill_round(dc, r, radius, t.color("glass-strong"))?;
        gfx::border(dc, r, radius, 1., t.color("glass-border"))?;
        gfx::text(
            dc,
            &text,
            r.left + 1. + pad.0,
            r.top + 1. + pad.1,
            t.color("glass-text"),
        )
    }
}

fn alpha(mut color: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    color.a *= a;
    color
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Shipping `RecordingControlsHiddenNotice`: the 418×74 card inside a 28 px
/// transparent frame that keeps its glass shadow unclipped.
pub const NOTICE_PAD: f32 = 28.;
pub const NOTICE_SIZE: (f32, f32) = (
    hud::HIDDEN_NOTICE_WIDTH as f32 + NOTICE_PAD * 2.,
    hud::HIDDEN_NOTICE_HEIGHT as f32 + NOTICE_PAD * 2.,
);

/// `.recording-controls-hidden-notice`: the accent capture tile, the title
/// and the sentence with one `<kbd>` chip per shortcut key, wrapped like the
/// shipping flex row. Drawn at rest (the 6 s fade lifecycle is not ported).
pub fn paint_hidden_notice(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    copy: &hud::HiddenNoticeCopy,
) -> Result<()> {
    let card = rect(
        NOTICE_PAD,
        NOTICE_PAD,
        NOTICE_SIZE.0 - NOTICE_PAD,
        NOTICE_SIZE.1 - NOTICE_PAD,
    );
    let radius = t.n("r-xl");
    gfx::shadow(dc, card, radius, t.shadow("glass-shadow"))?;
    gfx::fill_round(dc, card, radius, t.color("glass-strong"))?;
    gfx::border(dc, card, radius, 1., t.color("glass-border"))?;
    let (pad, gap, chip_gap) = (t.n("s-5"), t.n("s-5"), t.n("s-2"));
    let cy = (card.top + card.bottom) / 2.;
    // `.recording-controls-hidden-icon`: 34×34 accent tile, 20 px icon.
    let tile_left = card.left + 1. + pad;
    let tile = rect(tile_left, cy - 17., tile_left + 34., cy + 17.);
    gfx::fill_round(dc, tile, t.n("r-lg"), t.color("theme-accent"))?;
    gfx::icon(
        dc,
        &g.factory,
        "capture",
        &[],
        (tile.left + 7., tile.top + 7., 20.),
        1.8,
        t.color("theme-accent-ink"),
    )?;
    let left = tile.right + gap;
    let width = card.right - 1. - pad - left;
    let title = g.text_sized(t, copy.title, t.n("text-sm"), "weight-semibold", None)?;
    // The sentence's flex items: the two text runs and the key chips.
    let size = t.n("text-xs");
    let mut items = vec![(
        g.text_sized(t, &copy.before_keys, size, "weight-regular", None)?,
        false,
    )];
    for key in &copy.keys {
        items.push((
            g.text_sized(t, key, t.n("text-2xs"), "weight-semibold", None)?,
            true,
        ));
    }
    items.push((
        g.text_sized(t, copy.after_keys, size, "weight-regular", None)?,
        false,
    ));
    // `kbd`: 2px 5px padding, 1 px border with a 2 px bottom, min-width 20.
    let item_size = |text: &gfx::Text, chip: bool| {
        if chip {
            ((text.width + 12.).max(20.), text.height + 2. + 3. + 2.)
        } else {
            (text.width.min(width), text.height)
        }
    };
    let mut rows: Vec<(f32, Vec<(usize, f32)>)> = Vec::new();
    let mut x = 0.;
    for (index, (text, chip)) in items.iter().enumerate() {
        let (w, h) = item_size(text, *chip);
        if rows.is_empty() || (x > 0. && x + w > width) {
            rows.push((0., Vec::new()));
            x = 0.;
        }
        let row = rows.last_mut().expect("a row");
        row.0 = row.0.max(h);
        row.1.push((index, x));
        x += w + chip_gap;
    }
    let sentence: f32 = rows.iter().map(|r| r.0).sum::<f32>() + chip_gap * (rows.len() as f32 - 1.);
    let mut y = cy - (title.height + 3. + sentence) / 2.;
    gfx::text(dc, &title, left, y, t.color("glass-text"))?;
    y += title.height + 3.;
    for (height, row) in rows {
        for (index, x) in row {
            let (text, chip) = &items[index];
            let (w, h) = item_size(text, *chip);
            let top = y + (height - h) / 2.;
            if *chip {
                let r = rect(left + x, top, left + x + w, top + h);
                let fill = D2D1_COLOR_F {
                    r: 1.,
                    g: 1.,
                    b: 1.,
                    a: 0.08,
                };
                gfx::fill_round(dc, r, t.n("r-xs"), fill)?;
                gfx::border(dc, r, t.n("r-xs"), 1., t.color("glass-border-strong"))?;
                let bottom = rect(r.left + 2., r.bottom - 2., r.right - 2., r.bottom - 1.);
                gfx::fill(dc, bottom, t.color("glass-border-strong"))?;
                let tx = r.left + (w - text.width) / 2.;
                gfx::text(dc, text, tx, top + 3., t.color("glass-text"))?;
            } else {
                if text.width > width {
                    g.ellipsis(text, width)?;
                }
                gfx::text(dc, text, left + x, top, t.color("glass-text-muted"))?;
            }
        }
        y += height + chip_gap;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_parts_follow_the_shipping_dom() {
        let mut h = Hud::new(true);
        h.input.state = RecordingState::Recording;
        h.input.busy = false;
        let kinds = |h: &Hud| h.parts().into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        // No microphone: no level meter, and no error line.
        assert!(!kinds(&h).contains(&Part::Level));
        assert!(!kinds(&h).contains(&Part::Error));
        h.input.has_microphone = true;
        h.level = 0.426;
        h.error = Some("ffmpeg exited".into());
        let parts = kinds(&h);
        let at = |p| parts.iter().position(|q| *q == p).unwrap();
        assert_eq!(
            &parts[..4],
            &[
                Part::Privacy,
                Part::Time,
                Part::Status,
                Part::Button(Control::Stop)
            ]
        );
        // The meter sits between Screenshot and Mute; the error comes last.
        assert_eq!(at(Part::Level) + 1, at(Part::Button(Control::Microphone)));
        assert_eq!(at(Part::Button(Control::Screenshot)) + 1, at(Part::Level));
        assert_eq!(parts.last(), Some(&Part::Error));
        assert_eq!(h.part_name(Part::Level), "Microphone level 43%");
        assert_eq!(h.part_name(Part::Button(Control::Stop)), "Stop recording");
        assert_eq!(
            h.part_name(Part::Button(Control::PauseResume)),
            "Pause recording"
        );
        h.input.state = RecordingState::Paused;
        assert_eq!(
            h.part_name(Part::Button(Control::PauseResume)),
            "Resume recording"
        );
        assert_eq!(
            h.part_name(Part::Button(Control::Hide)),
            "Hide recording controls"
        );
        assert!(h.part_name(Part::Privacy).contains("won’t"));
        // Every part is inside the window.
        for (_, r) in h.parts() {
            assert!(r.left >= 0. && r.top >= 0. && r.right <= SIZE.0 && r.bottom <= SIZE.1);
        }
    }

    #[test]
    fn controls_match_the_shipping_row() {
        let mut h = Hud::new(true);
        h.input.has_microphone = true;
        let (buttons, level) = h.layout();
        let order: Vec<Control> = buttons.iter().map(|(c, _)| *c).collect();
        assert_eq!(order, Control::ALL);
        // The row ends at the card's 8 px padding; Stop is 36 wide, the rest 34.
        let (_, hide) = buttons[6];
        assert_eq!(hide.right, 424. - 1. - 8.);
        assert_eq!(buttons[0].1.right - buttons[0].1.left, 36.);
        // The meter sits between Screenshot and Mute with its 4 px margins.
        let meter = level.expect("meter with a microphone");
        // Shipping's measured meter: 34 px shrunk to 12 by the overflow, so
        // Stop starts right after the status column.
        assert_eq!(meter.right - meter.left, 12.);
        assert_eq!(buttons[0].1.left, 6. + 1. + 8. + 96. + 12.);
        assert_eq!(buttons[4].1.left - meter.right, 4. + 4.);
        assert_eq!(meter.left - buttons[3].1.right, 4. + 4.);
        // Hide keeps its extra 4 px margin.
        assert_eq!(hide.left - buttons[5].1.right, 8.);
        // Without a microphone the meter is gone and the row closes up.
        h.input.has_microphone = false;
        let (without, level) = h.layout();
        assert!(level.is_none());
        assert_eq!(without[4].1.left - without[3].1.right, 4.);
    }

    #[test]
    fn clock_switches_to_hours() {
        assert_eq!(clock(94_000), "1:34");
        assert_eq!(clock(3_725_000), "1:02:05");
    }
}
