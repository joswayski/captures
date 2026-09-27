//! Shipping takeover of overlapping OS screenshot shortcuts.
//!
//! Before claiming its chords, shipping Captures (`register_shortcuts_with`)
//! unbinds the stock screenshot keys the saved shortcuts overlap, on every
//! launch and whenever a shortcut changes. There is no consent prompt and no
//! restore: Preferences explains the change and opens the OS keyboard
//! settings, where users can turn the keys back on. This module ports those
//! exact writes behind an injectable [`Runner`]; tests never touch the OS.
//!
//! * macOS: `defaults write com.apple.symbolichotkeys` for ⌘⇧3 / ⌘⇧4 / ⌘⇧5,
//!   then `activateSettings -u`. The AppKit host also disables the returned
//!   ids live in WindowServer, which the plist write alone does not do.
//! * Linux: clears the overlapping GNOME Shell/media-keys `gsettings`, and
//!   KDE Spectacle's rectangular-region key when Super+Shift+S is claimed.
//! * Windows: turns off Print Screen for Snipping Tool when a shortcut uses
//!   Print Screen. Win+Shift+S is intercepted in [`crate::shortcuts`].
//!
//! Only the shortcuts a native host registers count (the seven Preferences
//! rows); the GIF shortcut is not a native binding yet, so it never unbinds
//! a system key.

use std::collections::BTreeSet;

use captures_settings::AppSettings;

use crate::shortcuts::ShortcutPlatform;

/// kCGSHotKeyScreenshot: save picture of screen as a file (⌘⇧3).
pub const MACOS_SCREENSHOT_SAVE_SCREEN: u32 = 28;
/// kCGSHotKeyScreenshotRegion: save picture of selected area (⌘⇧4).
pub const MACOS_SCREENSHOT_SAVE_AREA: u32 = 30;
/// Screenshot and recording options (⌘⇧5).
pub const MACOS_SCREENSHOT_OPTIONS: u32 = 184;
/// Stock (ascii, keycode, modifiers) written back with the disabled flag.
const MACOS_SCREENSHOT_PARAMETERS: [(u32, (u32, u32, u32)); 3] = [
    (MACOS_SCREENSHOT_SAVE_SCREEN, (51, 20, 1_179_648)),
    (MACOS_SCREENSHOT_SAVE_AREA, (52, 21, 1_179_648)),
    (MACOS_SCREENSHOT_OPTIONS, (53, 23, 1_179_648)),
];
const MACOS_ACTIVATE_SETTINGS: &str =
    "/System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings";

pub const GNOME_SHELL_KEYBINDINGS: &str = "org.gnome.shell.keybindings";
pub const GNOME_MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const GNOME_GSETTINGS_BINARIES: [&str; 2] = ["gsettings", "/usr/bin/gsettings"];
const KDE_SPECTACLE_REGION_WRITE_ARGS: [&str; 7] = [
    "--file",
    "kglobalshortcutsrc",
    "--group",
    "org.kde.spectacle.desktop",
    "--key",
    "RectangularRegion",
    "none,none,Capture Rectangular Region",
];
const WINDOWS_SNIPPING_KEYS: [&str; 2] = [
    r"HKCU\Control Panel\Keyboard",
    r"HKCU\Control Panel\Accessibility",
];

/// Set to `1` to skip every OS write (smoke tests and disposable profiles).
pub const SKIP_ENV: &str = "CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER";

/// A GNOME screenshot or screencast binding overlapping a Captures shortcut.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GnomeBinding {
    pub schema: &'static str,
    pub key: &'static str,
}

/// A process launch failure versus a launched command that exited nonzero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError {
    Spawn(String),
    Exit(String),
}

impl RunError {
    fn message(self) -> String {
        match self {
            Self::Spawn(message) | Self::Exit(message) => message,
        }
    }
}

/// Runs OS configuration commands. The real runner waits for each command.
pub trait Runner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), RunError>;
    fn is_file(&self, path: &str) -> bool;
}

/// The process runner shipping uses (`std::process::Command::output`).
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), RunError> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| RunError::Spawn(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Err(RunError::Exit(if stderr.is_empty() {
            stdout
        } else {
            stderr
        }))
    }

    fn is_file(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }
}

/// What a takeover changed or failed to change.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Takeover {
    /// macOS symbolic hotkeys the host should also disable in WindowServer.
    pub macos_symbolic_hotkeys: Vec<u32>,
    /// Shipping logs these and still registers its shortcuts.
    pub errors: Vec<String>,
}

fn registered_shortcuts(settings: &AppSettings) -> [&str; 7] {
    [
        settings.new_capture_shortcut.as_str(),
        settings.region_shortcut.as_str(),
        settings.window_shortcut.as_str(),
        settings.display_shortcut.as_str(),
        settings.recording.video_shortcut.as_str(),
        settings.recording.window_shortcut.as_str(),
        settings.recording.display_shortcut.as_str(),
    ]
}

fn canonical_token(token: &str) -> String {
    let normalized = token.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "control" | "ctrl" => "control".to_owned(),
        "shift" => "shift".to_owned(),
        "alt" | "option" => "alt".to_owned(),
        "super" | "cmd" | "command" | "meta" | "win" => "super".to_owned(),
        "commandorcontrol" | "commandorctrl" | "cmdorctrl" | "cmdorcontrol" => {
            "commandorcontrol".to_owned()
        }
        "printscreen" | "prtscn" | "prtsc" | "print" => "printscreen".to_owned(),
        other => other
            .strip_prefix("digit")
            .or_else(|| other.strip_prefix("key"))
            .unwrap_or(other)
            .to_owned(),
    }
}

fn canonical_parts(shortcut: &str) -> Option<(BTreeSet<String>, String)> {
    let mut tokens: Vec<String> = shortcut
        .split('+')
        .map(canonical_token)
        .filter(|token| !token.is_empty())
        .collect();
    let key = tokens.pop()?;
    Some((tokens.into_iter().collect(), key))
}

fn matches(shortcut: &str, modifiers: &[&str], key: &str) -> bool {
    let Some((mods, parsed)) = canonical_parts(shortcut) else {
        return false;
    };
    let expected: BTreeSet<String> = modifiers.iter().map(|m| canonical_token(m)).collect();
    parsed == canonical_token(key) && mods == expected
}

fn any_matches(settings: &AppSettings, modifiers: &[&str], key: &str) -> bool {
    registered_shortcuts(settings)
        .iter()
        .any(|shortcut| matches(shortcut, modifiers, key))
}

fn command_shift(shortcut: &str, key: &str) -> bool {
    let Some((mods, parsed)) = canonical_parts(shortcut) else {
        return false;
    };
    parsed == key
        && mods.contains("shift")
        && (mods.contains("super") || mods.contains("commandorcontrol"))
        && !mods.contains("control")
        && mods.len() == 2
}

/// Stock macOS Screenshot hotkey ids the shortcuts overlap, sorted.
pub fn macos_conflicts(settings: &AppSettings) -> Vec<u32> {
    let mut ids = Vec::new();
    for shortcut in registered_shortcuts(settings) {
        for (key, id) in [
            ("3", MACOS_SCREENSHOT_SAVE_SCREEN),
            ("4", MACOS_SCREENSHOT_SAVE_AREA),
            ("5", MACOS_SCREENSHOT_OPTIONS),
        ] {
            if command_shift(shortcut, key) {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// `defaults write` arguments that persist a disabled hotkey via cfprefsd.
pub fn macos_defaults_write_args(id: u32) -> Vec<String> {
    let entry = match MACOS_SCREENSHOT_PARAMETERS
        .iter()
        .find(|(candidate, _)| *candidate == id)
    {
        Some((_, (ascii, keycode, modifiers))) => format!(
            "<dict><key>enabled</key><false/><key>value</key><dict><key>type</key><string>standard</string><key>parameters</key><array><integer>{ascii}</integer><integer>{keycode}</integer><integer>{modifiers}</integer></array></dict></dict>"
        ),
        None => "<dict><key>enabled</key><false/></dict>".to_owned(),
    };
    vec![
        "write".to_owned(),
        "com.apple.symbolichotkeys".to_owned(),
        "AppleSymbolicHotKeys".to_owned(),
        "-dict-add".to_owned(),
        id.to_string(),
        entry,
    ]
}

/// GNOME screenshot/screencast bindings the shortcuts overlap, sorted.
pub fn gnome_conflicts(settings: &AppSettings) -> Vec<GnomeBinding> {
    let binding = |schema, key| GnomeBinding { schema, key };
    let mut bindings = Vec::new();
    if any_matches(settings, &[], "PrintScreen") || any_matches(settings, &["Super", "Shift"], "S")
    {
        bindings.push(binding(GNOME_SHELL_KEYBINDINGS, "show-screenshot-ui"));
        bindings.push(binding(GNOME_MEDIA_KEYS, "screenshot"));
    }
    if any_matches(settings, &["Shift"], "PrintScreen") {
        bindings.push(binding(GNOME_SHELL_KEYBINDINGS, "screenshot"));
        bindings.push(binding(GNOME_MEDIA_KEYS, "area-screenshot"));
    }
    if any_matches(settings, &["Alt"], "PrintScreen") {
        bindings.push(binding(GNOME_SHELL_KEYBINDINGS, "screenshot-window"));
        bindings.push(binding(GNOME_MEDIA_KEYS, "window-screenshot"));
    }
    if any_matches(settings, &["Control", "Shift", "Alt"], "R") {
        bindings.push(binding(GNOME_SHELL_KEYBINDINGS, "show-screen-recording-ui"));
    }
    bindings.sort();
    bindings.dedup();
    bindings
}

/// A shortcut uses Print Screen, which Windows may route to Snipping Tool.
pub fn uses_print_screen(settings: &AppSettings) -> bool {
    let print = canonical_token("PrintScreen");
    registered_shortcuts(settings)
        .iter()
        .any(|shortcut| canonical_parts(shortcut).is_some_and(|(_, key)| key == print))
}

/// A shortcut is Win/Super+Shift+S (Snipping Tool / Spectacle region).
pub fn uses_super_shift_s(settings: &AppSettings) -> bool {
    any_matches(settings, &["Super", "Shift"], "S")
}

fn take_over_macos(ids: &[u32], runner: &mut impl Runner) -> Result<(), String> {
    for id in ids {
        let args = macos_defaults_write_args(*id);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        runner.run("defaults", &args).map_err(RunError::message)?;
    }
    if runner.is_file(MACOS_ACTIVATE_SETTINGS) {
        let _ = runner.run(MACOS_ACTIVATE_SETTINGS, &["-u"]);
    }
    Ok(())
}

fn clear_gsettings_key(binding: GnomeBinding, runner: &mut impl Runner) -> Result<(), String> {
    let mut last_error = String::new();
    for binary in GNOME_GSETTINGS_BINARIES {
        for value in ["[]", "['']"] {
            match runner.run(binary, &["set", binding.schema, binding.key, value]) {
                Ok(()) => return Ok(()),
                Err(error) => last_error = error.message(),
            }
        }
    }
    Err(last_error)
}

fn take_over_kde(runner: &mut impl Runner) {
    let wrote = ["kwriteconfig6", "kwriteconfig5"]
        .into_iter()
        .any(|binary| runner.run(binary, &KDE_SPECTACLE_REGION_WRITE_ARGS).is_ok());
    if !wrote {
        return;
    }
    for qdbus in ["qdbus6", "qdbus"] {
        let reload = runner.run(
            qdbus,
            &[
                "org.kde.kglobalaccel",
                "/kglobalaccel",
                "org.kde.KGlobalAccel.reloadConfig",
            ],
        );
        // Shipping stops at the first qdbus that launches, whatever it returns.
        if !matches!(reload, Err(RunError::Spawn(_))) {
            break;
        }
    }
}

fn take_over_windows_print_screen(runner: &mut impl Runner) -> Result<(), String> {
    for key in WINDOWS_SNIPPING_KEYS {
        runner
            .run(
                "reg",
                &[
                    "add",
                    key,
                    "/v",
                    "PrintScreenKeyForSnippingEnabled",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "0",
                    "/f",
                ],
            )
            .map_err(RunError::message)?;
    }
    Ok(())
}

/// Unbind the system screenshot keys `settings` overlaps on `platform`.
pub fn take_over(
    settings: &AppSettings,
    platform: ShortcutPlatform,
    runner: &mut impl Runner,
) -> Takeover {
    let mut takeover = Takeover::default();
    match platform {
        ShortcutPlatform::Macos => {
            let ids = macos_conflicts(settings);
            if !ids.is_empty() {
                if let Err(error) = take_over_macos(&ids, runner) {
                    takeover.errors.push(format!(
                        "could not persist disabled macOS Screenshot shortcuts: {error}"
                    ));
                }
                takeover.macos_symbolic_hotkeys = ids;
            }
        }
        ShortcutPlatform::Linux => {
            let errors: Vec<String> = gnome_conflicts(settings)
                .into_iter()
                .filter_map(|binding| clear_gsettings_key(binding, runner).err())
                .collect();
            if !errors.is_empty() {
                takeover.errors.push(format!(
                    "could not disable overlapping GNOME screenshot shortcuts: {}",
                    errors.join("; ")
                ));
            }
            if uses_super_shift_s(settings) {
                take_over_kde(runner);
            }
        }
        ShortcutPlatform::Windows => {
            if uses_print_screen(settings)
                && let Err(error) = take_over_windows_print_screen(runner)
            {
                takeover.errors.push(format!(
                    "could not disable Windows Print Screen snipping: {error}"
                ));
            }
        }
    }
    takeover
}

/// The platform this process runs on.
pub const fn current_platform() -> ShortcutPlatform {
    if cfg!(target_os = "macos") {
        ShortcutPlatform::Macos
    } else if cfg!(target_os = "windows") {
        ShortcutPlatform::Windows
    } else {
        ShortcutPlatform::Linux
    }
}

/// Live-host takeover with real commands, unless [`SKIP_ENV`] is `1`.
pub fn take_over_current_os(settings: &AppSettings) -> Takeover {
    if std::env::var_os(SKIP_ENV).is_some_and(|value| value == "1") {
        return Takeover::default();
    }
    take_over(settings, current_platform(), &mut SystemRunner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records every command; nothing reaches the OS.
    #[derive(Default)]
    struct Fake {
        commands: Vec<Vec<String>>,
        missing: Vec<&'static str>,
        failing: Vec<&'static str>,
        files: Vec<&'static str>,
    }

    impl Runner for Fake {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<(), RunError> {
            self.commands.push(
                std::iter::once(program.to_owned())
                    .chain(args.iter().map(|arg| (*arg).to_owned()))
                    .collect(),
            );
            if self.missing.contains(&program) {
                return Err(RunError::Spawn(format!("{program}: not found")));
            }
            if self.failing.contains(&program) {
                return Err(RunError::Exit(format!("{program} failed")));
            }
            Ok(())
        }

        fn is_file(&self, path: &str) -> bool {
            self.files.contains(&path)
        }
    }

    fn programs(fake: &Fake) -> Vec<&str> {
        fake.commands
            .iter()
            .map(|command| command[0].as_str())
            .collect()
    }

    fn macos_defaults() -> AppSettings {
        let mut settings = AppSettings {
            new_capture_shortcut: "CommandOrControl+Shift+Space".into(),
            region_shortcut: "CommandOrControl+Shift+4".into(),
            window_shortcut: "CommandOrControl+Shift+W".into(),
            display_shortcut: "CommandOrControl+Shift+3".into(),
            ..AppSettings::default()
        };
        settings.recording.video_shortcut = "CommandOrControl+Shift+5".into();
        settings.recording.window_shortcut = "CommandOrControl+Alt+W".into();
        settings.recording.display_shortcut = "CommandOrControl+Alt+F".into();
        settings.recording.gif_shortcut = "CommandOrControl+Shift+6".into();
        settings
    }

    fn linux_defaults() -> AppSettings {
        let mut settings = AppSettings {
            new_capture_shortcut: "PrintScreen".into(),
            region_shortcut: "Super+Shift+S".into(),
            window_shortcut: "Alt+PrintScreen".into(),
            display_shortcut: "Shift+PrintScreen".into(),
            ..AppSettings::default()
        };
        settings.recording.video_shortcut = "Control+Shift+Alt+R".into();
        settings.recording.window_shortcut = "Control+Shift+Alt+W".into();
        settings.recording.display_shortcut = "Control+Shift+Alt+F".into();
        settings
    }

    fn without_kde(fake: &Fake) -> Vec<&str> {
        programs(fake)
            .into_iter()
            .filter(|program| !program.contains("gsettings"))
            .collect()
    }

    #[test]
    fn macos_conflicts_match_shipping() {
        assert_eq!(macos_conflicts(&macos_defaults()), [28, 30, 184]);
        let mut control = macos_defaults();
        control.region_shortcut = "Ctrl+Shift+4".into();
        control.display_shortcut = "Ctrl+Shift+3".into();
        control.recording.video_shortcut = "Ctrl+Shift+5".into();
        assert!(macos_conflicts(&control).is_empty());
        let mut recorded = control.clone();
        recorded.region_shortcut = "Super+Shift+Digit4".into();
        assert_eq!(macos_conflicts(&recorded), [MACOS_SCREENSHOT_SAVE_AREA]);
        recorded.region_shortcut = "Command+Option+Shift+4".into();
        assert!(macos_conflicts(&recorded).is_empty());
        // The GIF shortcut is not a native binding, so it frees nothing.
        let mut gif = control;
        gif.recording.gif_shortcut = "Command+Shift+5".into();
        assert!(macos_conflicts(&gif).is_empty());
    }

    #[test]
    fn macos_writes_cfprefsd_entries_then_activates() {
        let args = macos_defaults_write_args(30);
        assert_eq!(
            args[..5],
            [
                "write",
                "com.apple.symbolichotkeys",
                "AppleSymbolicHotKeys",
                "-dict-add",
                "30"
            ]
        );
        assert!(args[5].contains("<key>enabled</key><false/>"));
        assert!(args[5].contains("<integer>52</integer><integer>21</integer>"));
        assert!(macos_defaults_write_args(28)[5].contains("<integer>20</integer>"));
        assert!(macos_defaults_write_args(184)[5].contains("<integer>23</integer>"));
        assert_eq!(
            macos_defaults_write_args(1)[5],
            "<dict><key>enabled</key><false/></dict>"
        );

        let mut fake = Fake {
            files: vec![MACOS_ACTIVATE_SETTINGS],
            ..Fake::default()
        };
        let takeover = take_over(&macos_defaults(), ShortcutPlatform::Macos, &mut fake);
        assert_eq!(takeover.macos_symbolic_hotkeys, [28, 30, 184]);
        assert!(takeover.errors.is_empty());
        assert_eq!(
            programs(&fake),
            ["defaults", "defaults", "defaults", MACOS_ACTIVATE_SETTINGS]
        );
        assert_eq!(fake.commands[3][1], "-u");

        // A failed write stops persisting but the live disable still runs.
        let mut failing = Fake {
            failing: vec!["defaults"],
            ..Fake::default()
        };
        let takeover = take_over(&macos_defaults(), ShortcutPlatform::Macos, &mut failing);
        assert_eq!(takeover.macos_symbolic_hotkeys, [28, 30, 184]);
        assert_eq!(programs(&failing), ["defaults"]);
        assert_eq!(
            takeover.errors,
            ["could not persist disabled macOS Screenshot shortcuts: defaults failed"]
        );
    }

    #[test]
    fn nothing_overlapping_changes_nothing() {
        let mut fake = Fake::default();
        let mut settings = macos_defaults();
        settings.region_shortcut = "Ctrl+Alt+F1".into();
        settings.display_shortcut = "Ctrl+Alt+F2".into();
        settings.recording.video_shortcut = "Ctrl+Alt+F3".into();
        for platform in [
            ShortcutPlatform::Macos,
            ShortcutPlatform::Linux,
            ShortcutPlatform::Windows,
        ] {
            assert_eq!(
                take_over(&settings, platform, &mut fake),
                Takeover::default()
            );
        }
        assert!(fake.commands.is_empty());
    }

    #[test]
    fn gnome_and_kde_bindings_match_shipping() {
        let bindings = gnome_conflicts(&linux_defaults());
        for (schema, key) in [
            (GNOME_SHELL_KEYBINDINGS, "show-screenshot-ui"),
            (GNOME_SHELL_KEYBINDINGS, "screenshot"),
            (GNOME_SHELL_KEYBINDINGS, "screenshot-window"),
            (GNOME_SHELL_KEYBINDINGS, "show-screen-recording-ui"),
            (GNOME_MEDIA_KEYS, "screenshot"),
            (GNOME_MEDIA_KEYS, "area-screenshot"),
            (GNOME_MEDIA_KEYS, "window-screenshot"),
        ] {
            assert!(bindings.contains(&GnomeBinding { schema, key }), "{key}");
        }
        assert_eq!(bindings.len(), 7);
        assert!(gnome_conflicts(&macos_defaults()).is_empty());

        let mut fake = Fake::default();
        let takeover = take_over(&linux_defaults(), ShortcutPlatform::Linux, &mut fake);
        assert!(takeover.errors.is_empty());
        let gsettings = fake
            .commands
            .iter()
            .filter(|command| command[0] == "gsettings")
            .count();
        assert_eq!(gsettings, 7);
        assert_eq!(
            fake.commands[0],
            [
                "gsettings",
                "set",
                GNOME_MEDIA_KEYS,
                "area-screenshot",
                "[]"
            ]
        );
        assert_eq!(without_kde(&fake), ["kwriteconfig6", "qdbus6"]);
        assert_eq!(fake.commands[7][1..], KDE_SPECTACLE_REGION_WRITE_ARGS);
    }

    #[test]
    fn gsettings_falls_back_like_shipping_and_reports_failures() {
        let mut settings = linux_defaults();
        settings.region_shortcut = "Ctrl+Shift+1".into();
        settings.window_shortcut = "Ctrl+Shift+2".into();
        settings.display_shortcut = "Ctrl+Shift+3".into();
        settings.recording.video_shortcut = "Ctrl+Shift+4".into();
        // PrintScreen alone: two bindings, no KDE (no Super+Shift+S).
        let mut fake = Fake {
            failing: vec!["gsettings", "/usr/bin/gsettings"],
            ..Fake::default()
        };
        let takeover = take_over(&settings, ShortcutPlatform::Linux, &mut fake);
        // Each binding tries both binaries with both empty values.
        assert_eq!(fake.commands.len(), 8);
        assert_eq!(fake.commands[1][4], "['']");
        assert_eq!(fake.commands[2][0], "/usr/bin/gsettings");
        assert_eq!(takeover.errors.len(), 1);
        assert!(takeover.errors[0].starts_with("could not disable overlapping GNOME"));
        assert!(without_kde(&fake).is_empty());
    }

    #[test]
    fn kde_reload_only_follows_a_successful_write() {
        let mut settings = macos_defaults();
        settings.region_shortcut = "Super+Shift+S".into();
        let mut missing = Fake {
            missing: vec!["kwriteconfig6", "kwriteconfig5"],
            failing: vec!["gsettings", "/usr/bin/gsettings"],
            ..Fake::default()
        };
        take_over(&settings, ShortcutPlatform::Linux, &mut missing);
        assert_eq!(without_kde(&missing), ["kwriteconfig6", "kwriteconfig5"]);

        let mut fallback = Fake {
            missing: vec!["kwriteconfig6", "qdbus6"],
            ..Fake::default()
        };
        take_over(&settings, ShortcutPlatform::Linux, &mut fallback);
        assert_eq!(
            without_kde(&fallback),
            ["kwriteconfig6", "kwriteconfig5", "qdbus6", "qdbus"]
        );
    }

    #[test]
    fn windows_turns_off_print_screen_snipping_only_when_used() {
        let mut settings = linux_defaults();
        let mut fake = Fake::default();
        let takeover = take_over(&settings, ShortcutPlatform::Windows, &mut fake);
        assert!(takeover.errors.is_empty());
        assert_eq!(fake.commands.len(), 2);
        assert_eq!(
            fake.commands[0],
            [
                "reg",
                "add",
                r"HKCU\Control Panel\Keyboard",
                "/v",
                "PrintScreenKeyForSnippingEnabled",
                "/t",
                "REG_DWORD",
                "/d",
                "0",
                "/f"
            ]
        );
        assert_eq!(fake.commands[1][2], r"HKCU\Control Panel\Accessibility");

        settings.new_capture_shortcut = "Ctrl+Shift+Space".into();
        settings.window_shortcut = "Ctrl+Shift+W".into();
        settings.display_shortcut = "Ctrl+Shift+F".into();
        assert!(!uses_print_screen(&settings));
        assert!(uses_super_shift_s(&settings));
        let mut unused = Fake::default();
        take_over(&settings, ShortcutPlatform::Windows, &mut unused);
        assert!(unused.commands.is_empty());

        let mut failing = Fake {
            failing: vec!["reg"],
            ..Fake::default()
        };
        let takeover = take_over(&linux_defaults(), ShortcutPlatform::Windows, &mut failing);
        assert_eq!(failing.commands.len(), 1);
        assert_eq!(
            takeover.errors,
            ["could not disable Windows Print Screen snipping: reg failed"]
        );
    }
}
