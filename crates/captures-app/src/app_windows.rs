//! The shipping window model: History, Preferences and first-run setup are
//! separate, resizable top-level windows, and each editor has its own window.
//!
//! Ported from the Tauri host: `show_capture_history`, `show_preferences_target`
//! and `show_onboarding` in `apps/desktop/src-tauri/src/lib.rs`, the screenshot
//! editor window in `screenshot_editor.rs` and the recording editor window in
//! `recording.rs`. Opening a window that is already open shows, restores and
//! focuses it; closing one never closes the others. AppKit mirrors these
//! values in `AppWindows.swift`.

/// One of the application's document windows (not the floating capture,
/// preview, HUD or notice surfaces).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AppWindow {
    History,
    Preferences,
    Setup,
}

/// Title and logical size of a document window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSpec {
    pub title: &'static str,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
}

impl WindowSpec {
    pub const fn size(&self) -> [f32; 2] {
        [self.width, self.height]
    }

    pub const fn min_size(&self) -> [f32; 2] {
        [self.min_width, self.min_height]
    }
}

pub const HISTORY: WindowSpec = WindowSpec {
    title: "Capture History",
    width: 1_020.,
    height: 720.,
    min_width: 640.,
    min_height: 440.,
};

pub const PREFERENCES: WindowSpec = WindowSpec {
    title: "Captures Preferences",
    width: 880.,
    height: 660.,
    min_width: 560.,
    min_height: 440.,
};

/// Shipping `ONBOARDING_WINDOW_WIDTH`/`HEIGHT`; the setup window is titled
/// with the app name.
pub const SETUP: WindowSpec = WindowSpec {
    title: "Captures",
    width: 620.,
    height: 560.,
    min_width: 480.,
    min_height: 440.,
};

/// Shipping `@media (max-width: 720px)` (`windows.css`): at or below this
/// window width Preferences hides its section nav and stacks its inline rows,
/// accent chips use two columns, and History stacks its header and uses one
/// card column.
pub const COMPACT_MAX_WIDTH: f32 = 720.;

/// Shipping `@media (max-height: 600px)`: the setup window drops its lede and
/// tightens its padding. The default 560 pt setup window is always short.
pub const SHORT_MAX_HEIGHT: f32 = 600.;

/// Whether a window of this logical width uses the compact layout.
pub fn compact(width: f32) -> bool {
    width <= COMPACT_MAX_WIDTH
}

/// Whether a window of this logical height uses the short setup layout.
pub fn short(height: f32) -> bool {
    height <= SHORT_MAX_HEIGHT
}

pub const SCREENSHOT_EDITOR_TITLE: &str = "Captures Screenshot Editor";
pub const RECORDING_EDITOR_TITLE: &str = "Captures Editor";

impl AppWindow {
    pub const ALL: [Self; 3] = [Self::History, Self::Preferences, Self::Setup];

    pub const fn spec(self) -> WindowSpec {
        match self {
            Self::History => HISTORY,
            Self::Preferences => PREFERENCES,
            Self::Setup => SETUP,
        }
    }

    /// Shipping `primary_app_window_priority`: lower focuses first when the
    /// app is reactivated. Editors (priority 1) sit between setup and History.
    pub const fn reactivation_priority(self) -> u8 {
        match self {
            Self::Setup => 0,
            Self::History => 2,
            Self::Preferences => 3,
        }
    }
}

/// What a visible, interactive launch shows, as in shipping
/// `interactive_launch_action`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveLaunch {
    /// First-run setup is unfinished.
    Setup,
    /// A quiet (autostart or post-update) launch stays in the tray and shows
    /// the launch notice.
    StartupNotice,
    Preferences,
}

/// Shipping `interactive_launch_action`: a launch that opens files goes
/// straight to their editors and shows none of these windows.
pub fn interactive_launch(
    onboarding_complete: bool,
    launched_quietly: bool,
    opening_files: bool,
) -> Option<InteractiveLaunch> {
    if opening_files {
        None
    } else if !onboarding_complete {
        Some(InteractiveLaunch::Setup)
    } else if launched_quietly {
        Some(InteractiveLaunch::StartupNotice)
    } else {
        Some(InteractiveLaunch::Preferences)
    }
}

/// Shipping `primary_app_window_priority` for an open screenshot or recording
/// editor window.
pub const EDITOR_REACTIVATION_PRIORITY: u8 = 1;

/// A window reopening may focus: a document window, or an open editor
/// identified by the host's own handle `E`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryWindow<E> {
    App(AppWindow),
    Editor(E),
}

impl<E> PrimaryWindow<E> {
    pub const fn reactivation_priority(&self) -> u8 {
        match self {
            Self::App(window) => window.reactivation_priority(),
            Self::Editor(_) => EDITOR_REACTIVATION_PRIORITY,
        }
    }
}

/// What reopening the app (Dock click, empty relaunch) does, as in shipping
/// `app_reactivation`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reactivation<E> {
    ShowSetup,
    RestoreRecordingControls,
    Focus(PrimaryWindow<E>),
    ShowPreferences,
}

/// `visible` lists the document and editor windows currently shown (in any
/// order); the first of equal priority wins.
pub fn reactivation<E: Copy>(
    onboarding_complete: bool,
    restore_recording_controls: bool,
    visible: &[PrimaryWindow<E>],
) -> Reactivation<E> {
    if !onboarding_complete {
        return Reactivation::ShowSetup;
    }
    if restore_recording_controls {
        return Reactivation::RestoreRecordingControls;
    }
    visible
        .iter()
        .copied()
        .min_by_key(PrimaryWindow::reactivation_priority)
        .map_or(Reactivation::ShowPreferences, Reactivation::Focus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_match_the_shipping_window_builders() {
        let lib = include_str!("../../../apps/desktop/src-tauri/src/lib.rs");
        let compact = lib.split_whitespace().collect::<String>();
        for (spec, width, height, min_width, min_height) in [
            (HISTORY, "1_020.0", "720.0", "640.0", "440.0"),
            (PREFERENCES, "880.0", "660.0", "560.0", "440.0"),
        ] {
            let builder = format!(
                ".title(\"{}\").inner_size({width},{height}).min_inner_size({min_width},{min_height})",
                spec.title
            )
            .split_whitespace()
            .collect::<String>();
            assert!(compact.contains(&builder), "{builder}");
        }
        assert!(lib.contains("const ONBOARDING_WINDOW_WIDTH: f64 = 620.0;"));
        assert!(lib.contains("const ONBOARDING_WINDOW_HEIGHT: f64 = 560.0;"));
        assert!(compact.contains(
            ".title(\"Captures\").inner_size(ONBOARDING_WINDOW_WIDTH,ONBOARDING_WINDOW_HEIGHT).min_inner_size(480.0,440.0)"
        ));
        assert_eq!(SETUP.size(), [620., 560.]);
        assert_eq!(SETUP.min_size(), [480., 440.]);
        let screenshot = include_str!("../../../apps/desktop/src-tauri/src/screenshot_editor.rs");
        assert!(screenshot.contains(&format!(".title(\"{SCREENSHOT_EDITOR_TITLE}\")")));
        let recording = include_str!("../../../apps/desktop/src-tauri/src/recording.rs");
        assert!(recording.contains(&format!(".title(\"{RECORDING_EDITOR_TITLE}\")")));
    }

    #[test]
    fn breakpoints_match_the_shipping_media_queries() {
        let css = include_str!("../../../apps/desktop/ui/src/styles/windows.css");
        assert!(css.contains("@media (max-width: 720px)"));
        assert!(css.contains("@media (max-height: 600px)"));
        // Default History is wide; its minimum and Preferences' are compact.
        assert!(!compact(HISTORY.width) && compact(HISTORY.min_width));
        assert!(!compact(PREFERENCES.width) && compact(PREFERENCES.min_width));
        assert!(short(SETUP.height) && !short(HISTORY.height));
    }

    #[test]
    fn every_window_is_larger_than_its_minimum() {
        for window in AppWindow::ALL {
            let spec = window.spec();
            assert!(spec.width >= spec.min_width && spec.height >= spec.min_height);
        }
    }

    #[test]
    fn reactivation_follows_shipping_priority() {
        use AppWindow::*;
        use PrimaryWindow::App;
        let lib = include_str!("../../../apps/desktop/src-tauri/src/lib.rs");
        assert!(lib.contains("fn primary_app_window_priority(label: &str) -> Option<u8>"));
        assert_eq!(
            reactivation::<u8>(false, true, &[App(History)]),
            Reactivation::ShowSetup
        );
        assert_eq!(
            reactivation::<u8>(true, true, &[App(History)]),
            Reactivation::RestoreRecordingControls
        );
        assert_eq!(
            reactivation::<u8>(true, false, &[App(Preferences), App(History)]),
            Reactivation::Focus(App(History))
        );
        assert_eq!(
            reactivation::<u8>(true, false, &[App(Preferences)]),
            Reactivation::Focus(App(Preferences))
        );
        assert_eq!(
            reactivation::<u8>(true, false, &[]),
            Reactivation::ShowPreferences
        );
    }

    #[test]
    fn reactivation_focuses_an_open_editor_before_history_and_preferences() {
        use AppWindow::*;
        use PrimaryWindow::{App, Editor};
        assert_eq!(
            reactivation(true, false, &[App(Preferences), App(History), Editor(7)]),
            Reactivation::Focus(Editor(7))
        );
        assert_eq!(
            reactivation(true, false, &[Editor(7), Editor(9)]),
            Reactivation::Focus(Editor(7))
        );
        // Setup outranks an editor; unfinished setup always shows setup.
        assert_eq!(
            reactivation(true, false, &[Editor(7), App(Setup)]),
            Reactivation::Focus(App(Setup))
        );
        assert_eq!(
            reactivation(false, false, &[Editor(7)]),
            Reactivation::ShowSetup
        );
        assert_eq!(
            reactivation(true, true, &[Editor(7)]),
            Reactivation::RestoreRecordingControls
        );
        let ranked: [PrimaryWindow<u8>; 4] =
            [App(Setup), Editor(0), App(History), App(Preferences)];
        let priorities = ranked.map(|window| window.reactivation_priority());
        assert_eq!(priorities, [0, 1, 2, 3]);
    }

    #[test]
    fn interactive_launch_matches_shipping() {
        use InteractiveLaunch::*;
        let lib = include_str!("../../../apps/desktop/src-tauri/src/lib.rs");
        assert!(lib.contains("fn interactive_launch_action("));
        // (onboarding complete, quiet, opening files)
        assert_eq!(interactive_launch(false, false, false), Some(Setup));
        assert_eq!(interactive_launch(false, true, false), Some(Setup));
        assert_eq!(interactive_launch(true, true, false), Some(StartupNotice));
        assert_eq!(interactive_launch(true, false, false), Some(Preferences));
        for complete in [false, true] {
            for quiet in [false, true] {
                assert_eq!(interactive_launch(complete, quiet, true), None);
            }
        }
    }
}
