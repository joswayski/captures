//! Shipping Preferences copy, option lists and small presentation policies,
//! shared by the AppKit and wgpu hosts so both match `Preferences` in the
//! Tauri UI (`App.tsx`, `lib/preferencesFind.ts`, `lib/shortcut.ts`).
//!
//! Native-only differences are explicit here rather than in each host: the
//! system screenshot-key takeover, signed updates and the development login
//! item are not connected in native builds, so their copy says so.

use crate::shortcuts::ShortcutPlatform;

/// One Preferences card and its sidebar entry, in shipping order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

pub const SECTIONS: [Section; 7] = [
    Section {
        id: "appearance",
        title: "Appearance",
        description: "One look across every Captures window. Capture overlays stay dark so they read on any desktop.",
    },
    Section {
        id: "capture",
        title: "Capture",
        description: "Where captures go and what happens right after you take one.",
    },
    Section {
        id: "shortcuts",
        title: "Shortcuts",
        description: "Select a shortcut, then press the key combination you want. Press Esc to cancel recording.",
    },
    Section {
        id: "recording",
        title: "Recording",
        description: "Defaults for new screen recordings. You can still change them in the capture menu.",
    },
    Section {
        id: "gif",
        title: "GIF export",
        description: "Starting point when a recording is exported as an animated GIF.",
    },
    Section {
        id: "updates",
        title: "Updates",
        description: "Preview builds check for a new version automatically. Update now installs it in place.",
    },
    Section {
        id: "about",
        title: "About",
        description: "Captures is in active development. Telling us what breaks is the fastest way to fix it.",
    },
];

pub const TITLE: &str = "Preferences";
pub const SUBTITLE: &str = "Changes save automatically.";
pub const HISTORY_ACTION: &str = "Capture History…";
pub const LOADING: &str = "Loading preferences…";
pub const SAVING: &str = "Saving changes…";
pub const SAVED: &str = "Changes saved";

pub fn save_error(error: &str) -> String {
    format!("Couldn’t save changes: {error}")
}

pub fn load_error(error: &str) -> String {
    format!("Couldn’t load preferences: {error}")
}

/// The sidebar entry for the section in view (shipping `useVisibleSection`):
/// the last card whose top has passed `probe` (a point just inside the top of
/// the viewport), or the final card once the page reaches its end.
pub fn visible_section(card_tops: &[f32], probe: f32, at_end: bool) -> usize {
    if at_end {
        return card_tops.len().saturating_sub(1);
    }
    card_tops
        .iter()
        .rposition(|top| *top <= probe)
        .unwrap_or_default()
}

/// A title/description pair for one setting row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

/// Static setting rows by persisted key (`recording.` prefixes nested keys).
/// Rows whose description depends on state have their own functions below.
pub const ROWS: [Row; 27] = [
    Row {
        key: "appearance",
        title: "Interface theme",
        description: "Follow the system setting, or lock Captures to light or dark.",
    },
    Row {
        key: "theme",
        title: "Accent color",
        description: "Used for the capture action, selection, and focus. Status colors keep their meaning.",
    },
    Row {
        key: "output_directory",
        title: "Save captures to",
        description: "",
    },
    Row {
        key: "auto_copy_to_clipboard",
        title: "Automatically copy captures to the clipboard",
        description: "Turn this off to preserve existing text or other clipboard contents.",
    },
    Row {
        key: "auto_start_on_selection",
        title: "Start capture as soon as a target is selected",
        description: "Drawing a region, choosing a window, or clicking Full screen immediately starts the capture. When this is off, press Enter in the capture menu to confirm.",
    },
    Row {
        key: "show_mini_previews",
        title: "Show mini previews after screenshots",
        description: "Turn this off to keep the quick-access preview stack hidden.",
    },
    Row {
        key: "mini_preview_placement",
        title: "Mini preview position",
        description: "Choose a screen corner. Show less stays on that edge, and the stack opens away from it. You can still drag the collapsed pile during a session.",
    },
    Row {
        key: "include_mini_previews_in_captures",
        title: "Show mini previews in screenshots and recordings",
        description: "",
    },
    Row {
        key: "include_recording_controls_in_captures",
        title: "Show recording controls in screenshots and recordings",
        description: "",
    },
    Row {
        key: "freeze_screen",
        title: "Freeze screen when capturing",
        description: "Holds hover states, tooltips, menus, and motion still while you choose a region or window. Turn this off to select from the live desktop.",
    },
    Row {
        key: "show_cursor_in_screenshots",
        title: "Show cursor in screenshots",
        description: "Includes the pointer in still captures. Freeze screen only holds the desktop still; it does not add the cursor by itself.",
    },
    Row {
        key: "screenshot_format",
        title: "Screenshot format",
        description: "Used when you save or export. Capture History keeps a lossless PNG until then.",
    },
    Row {
        key: "screenshot_countdown_seconds",
        title: "Screenshot countdown",
        description: "Wait before capturing so you can open menus or hover states. Press Esc to cancel.",
    },
    Row {
        key: "recording.video_format",
        title: "Recording format",
        description: "Recordings are captured as H.264 MP4. GIF and WebM are converted when you save or export.",
    },
    Row {
        key: "recording.video_fps",
        title: "Frames per second",
        description: "",
    },
    Row {
        key: "recording.video_max_resolution",
        title: "Maximum resolution",
        description: "",
    },
    Row {
        key: "recording.countdown_seconds",
        title: "Countdown",
        description: "Delay before a recording starts.",
    },
    Row {
        key: "recording.microphone_device_id",
        title: "Default microphone",
        description: "Used when a recording starts with microphone audio.",
    },
    Row {
        key: "recording.capture_system_audio",
        title: "Record desktop audio",
        description: "Records sound playing through the system output.",
    },
    Row {
        key: "recording.mono_audio",
        title: "Export recording audio in mono",
        description: "",
    },
    Row {
        key: "recording.show_cursor",
        title: "Show cursor in recordings",
        description: "",
    },
    Row {
        key: "recording.highlight_clicks",
        title: "Show clicks in recordings",
        description: "",
    },
    Row {
        key: "recording.open_editor_after_recording",
        title: "Open the editor after recording",
        description: "The recording is kept in Capture History for 30 days, so closing the editor never loses it.",
    },
    Row {
        key: "recording.gif_fps",
        title: "Frames per second",
        description: "",
    },
    Row {
        key: "recording.gif_max_width",
        title: "Maximum width",
        description: "",
    },
    Row {
        key: "recording.gif_max_colors",
        title: "Palette colors",
        description: "",
    },
    Row {
        key: "show_update_changelog",
        title: "Show what’s new on update notices",
        description: "Lists every Preview since the version you have. Turn this off for a compact Update now prompt.",
    },
];

/// The copy for a static row. Panics on an unknown key: rows are fixed.
pub fn row(key: &str) -> Row {
    ROWS.iter()
        .copied()
        .find(|row| row.key == key)
        .unwrap_or_else(|| panic!("unknown Preferences row {key}"))
}

/// Order of the Recording card's toggles after the microphone select.
pub const RECORDING_TOGGLES: [&str; 5] = [
    "capture_system_audio",
    "mono_audio",
    "show_cursor",
    "highlight_clicks",
    "open_editor_after_recording",
];

/// Shipping `APPEARANCE_MODES` (`shared/appearance.ts`), in segment order.
pub const APPEARANCE_MODES: [(&str, &str); 3] =
    [("system", "System"), ("light", "Light"), ("dark", "Dark")];

/// One accent choice (`shared/themes.ts`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub const THEMES: [Theme; 10] = [
    Theme {
        id: "mustard",
        name: "Mustard",
        description: "Captures mustard and signal red",
    },
    Theme {
        id: "ember",
        name: "Ember",
        description: "Warm orange and electric pink",
    },
    Theme {
        id: "rose",
        name: "Rose",
        description: "Bright rose and coral",
    },
    Theme {
        id: "violet",
        name: "Violet",
        description: "Orchid violet and raspberry",
    },
    Theme {
        id: "cobalt",
        name: "Cobalt",
        description: "True blue and coral",
    },
    Theme {
        id: "aqua",
        name: "Aqua",
        description: "Clear cyan and watermelon",
    },
    Theme {
        id: "mint",
        name: "Mint",
        description: "Fresh mint and vermilion",
    },
    Theme {
        id: "lime",
        name: "Lime",
        description: "Crisp lime and vermilion",
    },
    Theme {
        id: "mono",
        name: "Mono",
        description: "Vercel-like black and white",
    },
    Theme {
        id: "custom",
        name: "Custom",
        description: "Build your own RGB palette",
    },
];

/// Accent chips per row in the shipping `.theme-options` grid.
pub const THEME_COLUMNS: usize = 5;

/// Shipping accessible name for a theme chip, `"{name}: {description}"`.
pub fn theme_accessibility_label(theme: &Theme) -> String {
    format!("{}: {}", theme.name, theme.description)
}

/// The shipping `.custom-theme-editor` copy.
pub struct CustomThemeCopy {
    pub title: &'static str,
    pub description: &'static str,
    pub reset: &'static str,
    /// `(key, label, description)` for Accent then Recording signal.
    pub fields: [(&'static str, &'static str, &'static str); 2],
}

pub const CUSTOM_THEME: CustomThemeCopy = CustomThemeCopy {
    title: "Custom colors",
    description: "Open either RGB picker or enter a hex value. Supporting shades stay readable.",
    reset: "Reset colors",
    fields: [
        (
            "accent",
            "Accent",
            "Capture actions, selections, focus, and editing.",
        ),
        (
            "signal",
            "Recording signal",
            "Recording indicators, errors, and destructive actions.",
        ),
    ],
};

/// Shipping `MINI_PREVIEW_PLACEMENTS`: persisted value and name. Corners are
/// laid out top-left, top-right, bottom-left, bottom-right.
pub const MINI_PREVIEW_PLACEMENTS: [(&str, &str); 4] = [
    ("top_left", "Top left"),
    ("top_right", "Top right"),
    ("bottom_left", "Bottom left"),
    ("bottom_right", "Bottom right"),
];
pub const MINI_PREVIEW_POSITION_LABEL: &str = "Mini preview position";

/// The placement name, falling back to the shipping default (Bottom left).
pub fn mini_preview_placement_name(value: &str) -> &'static str {
    MINI_PREVIEW_PLACEMENTS
        .iter()
        .find(|(id, _)| *id == value)
        .map_or(MINI_PREVIEW_PLACEMENTS[2].1, |(_, name)| name)
}

pub fn mini_previews_in_captures_description(show_previews: bool, include: bool) -> &'static str {
    if !show_previews {
        "Mini previews are off, so they won’t show in screenshots or recordings."
    } else if include {
        "Mini previews will show in screenshots and recordings. Turn this off to keep them out."
    } else {
        "Mini previews won’t show in screenshots or recordings."
    }
}

/// Description with the shipping `<strong>` word split out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Emphasized {
    pub lead: &'static str,
    pub emphasis: &'static str,
    pub trail: &'static str,
}

impl Emphasized {
    pub fn text(&self) -> String {
        format!("{}{}{}", self.lead, self.emphasis, self.trail)
    }
}

/// The recording-controls row explains when the platform cannot exclude them.
pub fn recording_controls_description(can_exclude: bool, include: bool) -> Emphasized {
    if !can_exclude {
        Emphasized {
            lead: "This desktop session cannot keep recording controls out of screenshots and recordings. Use Hide controls on the recording bar to keep them off-screen.",
            emphasis: "",
            trail: "",
        }
    } else if include {
        Emphasized {
            lead: "Recording controls ",
            emphasis: "will",
            trail: " show in screenshots and recordings. Turn this off to keep them out.",
        }
    } else {
        Emphasized {
            lead: "Recording controls ",
            emphasis: "won’t",
            trail: " show in screenshots or recordings.",
        }
    }
}

pub const SCREENSHOT_FORMATS: [(&str, &str); 3] =
    [("png", "PNG"), ("jpeg", "JPEG"), ("webp", "WebP")];
pub const RECORDING_FORMATS: [(&str, &str); 3] = [("mp4", "MP4"), ("gif", "GIF"), ("webm", "WebM")];
pub const RECORDING_FPS: [u16; 3] = crate::capture_menu::FPS_OPTIONS;
pub const RESOLUTIONS: [(&str, &str); 3] = crate::capture_menu::RESOLUTION_OPTIONS;
pub const COUNTDOWN_SECONDS: std::ops::RangeInclusive<u8> = 0..=10;
pub const GIF_FPS: [u16; 7] = [8, 10, 12, 15, 20, 24, 30];
pub const GIF_MAX_WIDTHS: [u16; 5] = [320, 480, 640, 800, 1200];
pub const GIF_PALETTE_COLORS: [u16; 4] = [64, 96, 128, 256];
pub const MICROPHONE_OFF: &str = "Off";
pub const MICROPHONES_LOADING: &str = "Finding microphones…";

pub fn countdown_label(seconds: u8) -> String {
    match seconds {
        0 => "Off".into(),
        1 => "1 second".into(),
        _ => format!("{seconds} seconds"),
    }
}

pub fn fps_label(fps: u16) -> String {
    format!("{fps} FPS")
}

pub fn width_label(width: u16) -> String {
    format!("{width} px")
}

/// Accessible names for selects whose visible titles repeat across cards.
pub fn select_accessibility_label(key: &str) -> &'static str {
    match key {
        "recording.video_fps" => "Recording frames per second",
        "recording.video_max_resolution" => "Recording maximum resolution",
        "recording.countdown_seconds" => "Recording countdown",
        "recording.gif_fps" => "GIF frames per second",
        "recording.gif_max_width" => "GIF maximum width",
        "recording.gif_max_colors" => "GIF palette colors",
        other => row(other).title,
    }
}

/// Shipping recorder rows: label and persisted path.
pub const SHORTCUT_ROWS: [(&str, &[&str]); 7] = [
    ("New Capture", &["new_capture_shortcut"]),
    ("Region", &["region_shortcut"]),
    ("Window", &["window_shortcut"]),
    ("Full Screen", &["display_shortcut"]),
    ("Record Region", &["recording", "video_shortcut"]),
    ("Record Window", &["recording", "window_shortcut"]),
    ("Record Full Screen", &["recording", "display_shortcut"]),
];
pub const SHORTCUT_PROMPT: &str = "Press shortcut…";
pub const FIXTURE_SHORTCUTS_NOTE: &str =
    "Fixture scenes edit disposable settings and never register global keys.";

/// Shortcuts card intro and the system screenshot-key utility row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutHelp {
    pub intro: &'static str,
    pub system_title: &'static str,
    pub system_body: &'static str,
    pub system_action: &'static str,
}

/// Shipping `platformShortcutHelp`. The shipping app unbinds overlapping
/// system keys; native builds do not yet, so the body says how to free them.
pub fn shortcut_help(platform: ShortcutPlatform) -> ShortcutHelp {
    match platform {
        ShortcutPlatform::Macos => ShortcutHelp {
            intro: "Defaults match macOS Screenshot for full screen, region, and region recording. Captures-only actions keep their own shortcuts.",
            system_title: "macOS Screenshot shortcuts",
            system_body: "This native build doesn’t unbind overlapping Screenshot app keys (⌘⇧3, ⌘⇧4, ⌘⇧5) yet. Turn them off in Keyboard Shortcuts so they reach Captures instead of the system overlay.",
            system_action: "Open",
        },
        ShortcutPlatform::Windows => ShortcutHelp {
            intro: "Defaults match Windows screenshot keys: Win+Shift+S region, PrtScn full screen, Alt+PrtScn window, and Win+Alt+R region recording.",
            system_title: "Windows screenshot shortcuts",
            system_body: "This native build doesn’t unbind overlapping Snipping Tool keys yet. Turn off Print Screen for Snipping Tool in keyboard settings so it reaches Captures instead of the system overlay.",
            system_action: "Open",
        },
        ShortcutPlatform::Linux => ShortcutHelp {
            intro: "Defaults match GNOME/Ubuntu screenshot keys: PrtScn opens New Capture, Super+Shift+S region, Shift+PrtScn full screen, Alt+PrtScn window, and Ctrl+Shift+Alt+R region recording.",
            system_title: "GNOME screenshot shortcuts",
            system_body: "This native build doesn’t turn off overlapping GNOME screenshot keys (or KDE Spectacle region capture) yet. Turn them off in Keyboard settings so they reach Captures.",
            system_action: "Open",
        },
    }
}

/// Shipping `open_system_screenshot_shortcut_settings` targets, tried in order.
/// macOS entries are URLs for the host's workspace opener.
pub fn keyboard_settings_targets(platform: ShortcutPlatform) -> &'static [&'static [&'static str]] {
    match platform {
        ShortcutPlatform::Macos => &[
            &["x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Screenshots"],
            &["x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Shortcuts"],
            &["x-apple.systempreferences:com.apple.preference.keyboard?Shortcuts"],
        ],
        ShortcutPlatform::Windows => &[&["ms-settings:easeofaccess-keyboard"]],
        ShortcutPlatform::Linux => &[
            &["gnome-control-center", "keyboard"],
            &["systemsettings", "kcm_keys"],
        ],
    }
}

/// Opens the platform keyboard-shortcut settings (Windows/Linux). macOS hosts
/// open [`keyboard_settings_targets`] URLs with their workspace API.
pub fn open_keyboard_settings(platform: ShortcutPlatform) -> Result<(), String> {
    for target in keyboard_settings_targets(platform) {
        let spawned = match platform {
            ShortcutPlatform::Windows => std::process::Command::new("explorer.exe")
                .arg(target[0])
                .spawn(),
            ShortcutPlatform::Linux => std::process::Command::new(target[0])
                .args(&target[1..])
                .spawn(),
            ShortcutPlatform::Macos => std::process::Command::new("open").arg(target[0]).spawn(),
        };
        if let Ok(mut child) = spawned {
            // Reap the launcher without blocking the UI thread.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
    }
    Err("Couldn’t open Keyboard settings.".into())
}

/// Updates card utility row. Signed updates are not connected natively.
pub const UPDATES_TITLE: &str = "Updates aren’t connected in this build";
pub const UPDATES_DETAIL: &str =
    "Signed Preview updates are not available to native development builds yet.";
pub const UPDATES_ACTION: &str = "Check Now";

pub const FEEDBACK_TITLE: &str = "Send feedback";
pub const FEEDBACK_DETAIL: &str = "Report a bug or share an idea.";
pub const FEEDBACK_ACTION: &str = "Open";

pub const LOGIN_ITEM_TITLE: &str = "Launch native Captures at login";
pub const LOGIN_ITEM_DETAIL: &str =
    "Start this native development profile hidden when you sign in.";
pub const LOGIN_ITEM_CHECKING: &str = "Checking…";
pub const LOGIN_ITEM_RETRY: &str = "Retry";

pub fn login_item_error(error: &str) -> String {
    format!("Couldn’t update the login item: {error} Select Retry to try again.")
}

/// Copy for a host that cannot register a development login item.
pub fn login_item_unavailable(platform: ShortcutPlatform) -> &'static str {
    match platform {
        ShortcutPlatform::Macos => "Available in a live development profile.",
        _ => {
            "Available in a live Windows or X11 development profile. Wayland hidden startup is not supported."
        }
    }
}

pub const FIND_PLACEHOLDER: &str = "Find settings";
pub const FIND_PREVIOUS: &str = "Previous match";
pub const FIND_NEXT: &str = "Next match";
pub const FIND_CLOSE: &str = "Close find";

/// Shipping `preferenceTextMatches`: case-insensitive, whitespace collapsed.
pub fn find_matches(text: &str, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .contains(&needle)
}

/// Shipping `preferenceFindCountLabel`.
pub fn find_count_label(query: &str, count: usize, index: usize) -> String {
    if query.trim().is_empty() {
        String::new()
    } else if count == 0 {
        "No results".into()
    } else {
        format!("{} of {count}", index.min(count - 1) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_rows_and_options_match_shipping() {
        let titles: Vec<_> = SECTIONS.iter().map(|section| section.title).collect();
        assert_eq!(
            titles,
            [
                "Appearance",
                "Capture",
                "Shortcuts",
                "Recording",
                "GIF export",
                "Updates",
                "About"
            ]
        );
        assert_eq!(row("recording.countdown_seconds").title, "Countdown");
        assert_eq!(
            select_accessibility_label("recording.gif_max_colors"),
            "GIF palette colors"
        );
        assert_eq!(
            select_accessibility_label("screenshot_format"),
            "Screenshot format"
        );
        assert_eq!(countdown_label(0), "Off");
        assert_eq!(countdown_label(1), "1 second");
        assert_eq!(countdown_label(10), "10 seconds");
        assert_eq!(fps_label(24), "24 FPS");
        assert_eq!(width_label(640), "640 px");
        assert_eq!(THEMES.len(), 2 * THEME_COLUMNS);
        assert_eq!(
            theme_accessibility_label(&THEMES[8]),
            "Mono: Vercel-like black and white"
        );
        assert_eq!(mini_preview_placement_name("top_right"), "Top right");
        assert_eq!(mini_preview_placement_name("nope"), "Bottom left");
        for key in RECORDING_TOGGLES {
            assert!(!row(&format!("recording.{key}")).title.is_empty());
        }
    }

    #[test]
    fn dynamic_descriptions_follow_state() {
        assert!(mini_previews_in_captures_description(false, true).contains("are off"));
        assert!(mini_previews_in_captures_description(true, true).contains("will show"));
        let will = recording_controls_description(true, true);
        assert_eq!(will.emphasis, "will");
        assert_eq!(
            will.text(),
            "Recording controls will show in screenshots and recordings. Turn this off to keep them out."
        );
        assert_eq!(
            recording_controls_description(true, false).emphasis,
            "won’t"
        );
        assert!(
            recording_controls_description(false, true)
                .text()
                .starts_with("This desktop session cannot")
        );
    }

    #[test]
    fn find_matches_and_counts_like_shipping() {
        assert!(find_matches(
            "Freeze   screen\nwhen capturing",
            "screen when"
        ));
        assert!(!find_matches("Freeze screen", "   "));
        assert_eq!(find_count_label(" ", 3, 0), "");
        assert_eq!(find_count_label("x", 0, 0), "No results");
        assert_eq!(find_count_label("x", 3, 1), "2 of 3");
        assert_eq!(find_count_label("x", 2, 9), "2 of 2");
    }

    #[test]
    fn visible_section_tracks_the_last_card_above_the_probe() {
        let tops = [0., 300., 700.];
        assert_eq!(visible_section(&tops, 80., false), 0);
        assert_eq!(visible_section(&tops, 320., false), 1);
        assert_eq!(visible_section(&tops, 320., true), 2);
        assert_eq!(visible_section(&[], 0., true), 0);
    }

    #[test]
    fn shortcut_help_is_platform_specific_and_native_accurate() {
        for platform in [
            ShortcutPlatform::Macos,
            ShortcutPlatform::Windows,
            ShortcutPlatform::Linux,
        ] {
            let help = shortcut_help(platform);
            assert!(help.intro.starts_with("Defaults match"));
            assert!(help.system_body.contains("native build"));
            assert!(!keyboard_settings_targets(platform).is_empty());
        }
        assert_eq!(SHORTCUT_ROWS.len(), 7);
        assert_eq!(SHORTCUT_ROWS[4].1, ["recording", "video_shortcut"]);
    }
}
