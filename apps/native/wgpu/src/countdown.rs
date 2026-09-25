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

/// Shared by the live secondary viewport and the screenshot-only CI probe.
/// The caller owns timing; a static countdown schedules no recurring redraw.
pub fn show(ui: &mut egui::Ui, t: &Tokens, remaining: u8, kind: Kind, cancelling: bool) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.color("glass-countdown-scrim")))
        .show(ui, |ui| {
            let rect = ui.max_rect();
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
        });
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
}
