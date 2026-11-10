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
//! `--dur-1` fade. Not ported yet: keyboard focus and UI Automation.

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

#[cfg(test)]
mod tests {
    use super::*;

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
