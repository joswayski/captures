//! Shipping's `.recording-countdown` scrim (ScreenshotCountdown and
//! RecordingCountdown) on one display: a heading, the seconds left and the
//! Esc hint, with the shared entry and exit motions. The owner creates the
//! popup with its own window class, forwards `WM_TIMER` here and decides
//! what happens when the deadline passes.

use std::time::Instant;

use captures_app::capture_flow;
use captures_app::motion::{Animation, Motion, Pose};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::gfx::{self, Gfx, rect};
use crate::tokens::Tokens;

pub struct Countdown {
    pub hwnd: HWND,
    g: Gfx,
    scale: f32,
    px: (u32, u32),
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
    deadline: capture_flow::Countdown,
    remaining: u8,
    /// When the current entry or exit motion started.
    started: Instant,
    exiting: bool,
    reduced: bool,
    scrim: Animation,
    content: Animation,
    /// "RECORDING IN" rather than "SCREENSHOT IN".
    recording: bool,
}

/// What a countdown frame decided.
pub enum Tick {
    Running,
    /// The deadline passed; drop the countdown before capturing.
    Elapsed,
    /// Cancelled and its exit motion finished.
    Closed,
}

impl Drop for Countdown {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

impl Countdown {
    /// Show the scrim in `hwnd`, a popup covering `display`, and start its
    /// 16 ms frame timer.
    pub fn open(
        hwnd: HWND,
        display: &captures_capture::DisplayDescriptor,
        g: Gfx,
        t: &Tokens,
        seconds: u8,
        recording: bool,
    ) -> Result<Self> {
        let resolve = |motion: Motion| motion.resolve(t).ok_or_else(Error::empty);
        let dcomp = g.dcomp.clone().ok_or_else(Error::empty)?;
        unsafe {
            let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.;
            let px = (display.width, display.height);
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            let surface = g.surface(px.0, px.1)?;
            visual.SetContent(&surface)?;
            target.SetRoot(&visual)?;
            let mut countdown = Countdown {
                hwnd,
                g,
                scale,
                px,
                _target: target,
                _visual: visual,
                surface,
                deadline: capture_flow::Countdown::new(Instant::now(), seconds),
                remaining: seconds,
                started: Instant::now(),
                exiting: false,
                reduced: crate::appearance::reduced_motion(),
                scrim: resolve(Motion::CountdownIn)?,
                content: resolve(Motion::CountdownContentIn)?,
                recording,
            };
            countdown.render(t)?;
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            SetTimer(Some(hwnd), 1, 16, None);
            Ok(countdown)
        }
    }

    /// A frame: play the exit motion once `current` is false, tick the
    /// number, or report the deadline.
    pub fn tick(&mut self, t: &Tokens, current: bool) -> Tick {
        if self.exiting {
            if self.animating() {
                let _ = self.render(t);
                return Tick::Running;
            }
            return Tick::Closed;
        }
        if !current {
            if self.reduced {
                return Tick::Closed;
            }
            self.exiting = true;
            self.started = Instant::now();
            if let (Some(scrim), Some(content)) = (
                Motion::CountdownOut.resolve(t),
                Motion::CountdownContentOut.resolve(t),
            ) {
                self.scrim = scrim;
                self.content = content;
            }
            let _ = self.render(t);
            return Tick::Running;
        }
        let remaining = self.deadline.remaining(Instant::now());
        if remaining == 0 {
            return Tick::Elapsed;
        }
        let changed = std::mem::replace(&mut self.remaining, remaining) != remaining;
        if changed || self.animating() {
            let _ = self.render(t);
        }
        Tick::Running
    }

    fn render(&mut self, t: &Tokens) -> Result<()> {
        let ms = self.started.elapsed().as_secs_f64() * 1000.;
        let (scrim, content) = (
            self.scrim.pose_at(ms, self.reduced),
            self.content.pose_at(ms, self.reduced),
        );
        let size = (self.px.0 as f32 / self.scale, self.px.1 as f32 / self.scale);
        let heading = if self.recording {
            "RECORDING IN"
        } else {
            "SCREENSHOT IN"
        };
        let hint = if self.exiting {
            "Cancelling…"
        } else {
            "Press Esc to cancel"
        };
        let (g, remaining) = (&self.g, self.remaining);
        gfx::draw_surface(&self.surface, self.px, (self.scale, self.scale), 0., |dc| {
            paint(dc, g, t, size, remaining, (heading, hint), scrim, content)
        })?;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    fn animating(&self) -> bool {
        let ms = self.started.elapsed().as_secs_f64() * 1000.;
        self.scrim.running(ms, self.reduced) || self.content.running(ms, self.reduced)
    }
}

/// Shipping's Screenshot or Recording countdown at `remaining` seconds: the
/// `--glass-countdown-scrim`, then the heading, number and hint, centred
/// with `.recording-countdown-content`'s grid, sizes and text shadow.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    (w, h): (f32, f32),
    remaining: u8,
    (heading, hint): (&str, &str),
    scrim: Pose,
    content: Pose,
) -> Result<()> {
    let scrim_color = t.color("glass-countdown-scrim");
    gfx::fill(
        dc,
        rect(0., 0., w, h),
        D2D1_COLOR_F {
            a: scrim_color.a * scrim.opacity as f32,
            ..scrim_color
        },
    )?;
    let heading_size = (w * 0.016).clamp(t.n("countdown-label-min"), t.n("countdown-label-max"));
    let number_size = (w * 0.26).clamp(t.n("countdown-number-min"), t.n("countdown-number-max"));
    let heading = g.text_sized(
        t,
        heading,
        heading_size,
        "weight-medium",
        Some("tracking-wide"),
    )?;
    let number = g.text_sized(
        t,
        &remaining.to_string(),
        number_size,
        "weight-bold",
        Some("tracking-tighter"),
    )?;
    let hint = g.text_sized(t, hint, t.n("text-md"), "weight-regular", None)?;
    // `margin: 0.04em 0 0.06em` around a `line-height: 0.95` number, then the
    // hint's `--s-7` top margin.
    let number_box = number_size * 0.95;
    let before = number_size * 0.04;
    let after = number_size * 0.06 + t.n("s-7");
    let total = heading.height + before + number_box + after + hint.height;
    let opacity = content.opacity as f32;
    let mut base = windows_numerics::Matrix3x2::identity();
    unsafe { dc.GetTransform(&mut base) };
    // `scale()` about the centre, after any `translateY`.
    let (cx, cy) = (w / 2., h / 2.);
    let k = content.scale as f32;
    let local = windows_numerics::Matrix3x2 {
        M11: k,
        M12: 0.,
        M21: 0.,
        M22: k,
        M31: cx - k * cx,
        M32: cy - k * cy + content.translate_y as f32,
    };
    unsafe { dc.SetTransform(&(local * base)) };
    let mut y = (h - total) / 2.;
    let lines = [
        (&heading, y, "glass-text-muted"),
        (
            &number,
            {
                y += heading.height + before;
                y + (number_box - number.height) / 2.
            },
            "glass-text",
        ),
        (
            &hint,
            {
                y += number_box + after;
                y
            },
            "glass-text-muted",
        ),
    ];
    let mut result = Ok(());
    for (text, top, color) in lines {
        let x = (w - text.width) / 2.;
        // `text-shadow: 0 3px 24px rgba(0, 0, 0, 0.6)`
        result = gfx::text_blur_shadow(dc, text, (x, top + 3.), 24., 0.6 * opacity)
            .and_then(|()| gfx::text(dc, text, x, top, t.color_alpha(color, opacity)));
        if result.is_err() {
            break;
        }
    }
    unsafe { dc.SetTransform(&base) };
    result
}
