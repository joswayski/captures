use captures_app::motion::{Motion, Pose};
use eframe::egui::{self, RichText};

use crate::tokens::Tokens;

/// Shipping `RECORDING_COUNTDOWN_FADE_OUT_MS`: how long "Cancelling…" stays up.
pub const CANCEL_LINGER_MS: u64 = 180;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Screenshot,
    Recording,
}

impl Kind {
    /// Shipping sentence-case copy; the shipping CSS uppercases it on screen.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot in",
            Self::Recording => "Recording starts in",
        }
    }
}

pub fn hint(cancelling: bool) -> &'static str {
    if cancelling {
        "Cancelling…"
    } else {
        "Press Esc to cancel"
    }
}

/// Scrim and content poses: shipping `recording-countdown-fade-in` /
/// `-content-in` on arrival and `-fade-out` / `-content-out` for the
/// cancelling exit (`.recording-countdown.exiting`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Poses {
    pub scrim: Pose,
    pub content: Pose,
}

impl Poses {
    pub const REST: Self = Self {
        scrim: Pose::REST,
        content: Pose::REST,
    };

    fn sample(t: &Tokens, scrim: Motion, content: Motion, ms: f64, reduced: bool) -> (Self, bool) {
        let (scrim, content) = (t.motion(scrim), t.motion(content));
        (
            Self {
                scrim: scrim.pose_at(ms, reduced),
                content: content.pose_at(ms, reduced),
            },
            scrim.running(ms, reduced) || content.running(ms, reduced),
        )
    }

    /// `ms` after the countdown window appeared; true while still moving.
    pub fn entrance(t: &Tokens, ms: f64, reduced: bool) -> (Self, bool) {
        Self::sample(
            t,
            Motion::CountdownIn,
            Motion::CountdownContentIn,
            ms,
            reduced,
        )
    }

    /// `ms` after a cancel started the "Cancelling…" exit.
    pub fn exit(t: &Tokens, ms: f64, reduced: bool) -> (Self, bool) {
        Self::sample(
            t,
            Motion::CountdownOut,
            Motion::CountdownContentOut,
            ms,
            reduced,
        )
    }
}

/// Shared by the live secondary viewport and the screenshot-only CI probe.
/// The caller owns timing; a static countdown schedules no recurring redraw.
pub fn show(
    ui: &mut egui::Ui,
    t: &Tokens,
    remaining: u8,
    kind: Kind,
    cancelling: bool,
    poses: Poses,
) {
    let scrim = t
        .color("glass-countdown-scrim")
        .gamma_multiply(poses.scrim.opacity.clamp(0., 1.) as f32);
    // The content is a child of the scrim in CSS, so its opacity compounds.
    let content = Pose {
        opacity: poses.scrim.opacity * poses.content.opacity,
        ..poses.content
    };
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(scrim))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            crate::motion::with_pose(ui, content, rect, |ui| {
                content_ui(ui, t, rect, remaining, kind, cancelling)
            });
        });
}

fn content_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    rect: egui::Rect,
    remaining: u8,
    kind: Kind,
    cancelling: bool,
) {
    let label_size = (rect.width() * 0.016).clamp(
        t.number("countdown-label-min"),
        t.number("countdown-label-max"),
    );
    let number_size = (rect.width() * 0.26).clamp(
        t.number("countdown-number-min"),
        t.number("countdown-number-max"),
    );
    // Labels, not paint-only text: expose the content to AccessKit.
    for (text, offset, size, color) in [
        (
            kind.heading().to_uppercase(),
            -number_size * 0.65,
            label_size,
            "glass-text-muted",
        ),
        (remaining.to_string(), 0., number_size, "glass-text"),
        (
            hint(cancelling).into(),
            number_size * 0.65 + t.number("s-7"),
            t.number("text-md"),
            "glass-text-muted",
        ),
    ] {
        ui.put(
            egui::Rect::from_center_size(
                rect.center() + egui::vec2(0., offset),
                egui::vec2(rect.width(), size * 1.3),
            ),
            egui::Label::new(RichText::new(text).size(size).color(t.color(color))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_copy_matches_shipping_strings() {
        assert_eq!(Kind::Screenshot.heading(), "Screenshot in");
        assert_eq!(Kind::Recording.heading(), "Recording starts in");
        assert_eq!(hint(false), "Press Esc to cancel");
        assert_eq!(hint(true), "Cancelling…");
    }

    #[test]
    fn countdown_fades_in_and_out_like_shipping() {
        let t = &crate::tokens::load()["dark-mustard"];
        let (start, running) = Poses::entrance(t, 0., false);
        assert_eq!(start.scrim.opacity, 0.);
        assert_eq!(start.content.scale, 0.94);
        assert!(running);
        // The scrim settles at --dur-3, the content at --dur-4.
        let (settled, running) = Poses::entrance(t, 280., false);
        assert_eq!(settled, Poses::REST);
        assert!(!running);
        assert_eq!(Poses::entrance(t, 0., true), (Poses::REST, false));
        let (gone, running) = Poses::exit(t, 140., false);
        assert_eq!(gone.scrim.opacity, 0.);
        assert_eq!(gone.content.scale, 1.035);
        assert!(!running);
        // Shipping's 0.01 ms rule lands the exit on its final keyframe at once.
        assert_eq!(Poses::exit(t, 0., true).0.scrim.opacity, 0.);
    }
}
