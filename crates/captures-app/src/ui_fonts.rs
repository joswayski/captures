//! Bundled cross-platform UI typefaces (`Figtree` and `JetBrains Mono`) so
//! Captures renders with identical typography on macOS, Windows, and Linux
//! without drifting across OS system fonts.

/// Primary UI sans-serif family bundled with every native host.
pub const UI_SANS_FAMILY: &str = "Figtree";

/// Monospace family bundled for pixel dimensions, shortcuts, and timecodes.
pub const UI_MONO_FAMILY: &str = "JetBrains Mono";

/// Combined SIL OFL license notices for the bundled UI fonts.
pub const NOTICE: &str = concat!(
    include_str!("../fonts/figtree/OFL.txt"),
    "\n\n",
    include_str!("../fonts/jetbrains-mono/OFL.txt")
);

/// Static `(id, ttf_bytes)` entries for every bundled UI face.
#[must_use]
pub fn files() -> &'static [(&'static str, &'static [u8])] {
    &[
        (
            "figtree-regular",
            include_bytes!("../fonts/figtree/Figtree-Regular.ttf"),
        ),
        (
            "figtree-medium",
            include_bytes!("../fonts/figtree/Figtree-Medium.ttf"),
        ),
        (
            "figtree-semibold",
            include_bytes!("../fonts/figtree/Figtree-SemiBold.ttf"),
        ),
        (
            "figtree-bold",
            include_bytes!("../fonts/figtree/Figtree-Bold.ttf"),
        ),
        (
            "figtree-semibold-italic",
            include_bytes!("../fonts/figtree/Figtree-SemiBoldItalic.ttf"),
        ),
        (
            "jetbrains-mono-regular",
            include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
        ),
        (
            "jetbrains-mono-medium",
            include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Medium.ttf"),
        ),
        (
            "jetbrains-mono-semibold",
            include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-SemiBold.ttf"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_ui_fonts_have_valid_truetype_headers_and_notices() {
        assert_eq!(UI_SANS_FAMILY, "Figtree");
        assert_eq!(UI_MONO_FAMILY, "JetBrains Mono");
        assert!(NOTICE.contains("SIL OPEN FONT LICENSE"));
        assert!(NOTICE.contains("Figtree"));
        assert!(NOTICE.contains("JetBrains Mono"));
        assert_eq!(files().len(), 8);
        for (id, bytes) in files() {
            assert!(bytes.len() > 10_000, "{id} looks truncated");
            assert_eq!(
                &bytes[..4],
                b"\x00\x01\x00\x00",
                "{id} must be a valid TrueType font"
            );
        }
    }
}
