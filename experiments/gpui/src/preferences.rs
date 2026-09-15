use crate::{
    Launch,
    theme::{Theme, font},
};
use anyhow::Result;
use gpui::{prelude::*, *};
use std::{collections::HashSet, fs, path::PathBuf};

pub mod history;
pub mod input;
pub mod settings;
use input::TextInput;
use settings::Settings;

actions!(preferences, [OpenFind, CloseFind]);

fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp")
    )
}

fn editor_view_for_path(path: &std::path::Path) -> &'static str {
    if is_image_path(path) {
        "screenshot-editor"
    } else {
        "recording-editor"
    }
}

fn block_changes_after_load_error(settings: &mut Settings, load_failed: bool) -> bool {
    if load_failed {
        *settings = Settings::default();
    }
    load_failed
}

fn preference_search_targets() -> [&'static str; 7] {
    [
        "appearance interface theme accent color custom recording signal",
        "capture save clipboard previews position screenshots recording controls freeze cursor format countdown",
        "shortcuts keyboard new capture region window full screen record",
        "recording format frames resolution countdown microphone desktop audio mono cursor clicks editor",
        "gif export frames per second maximum width palette colors",
        "updates preview changelog show what changed",
        "about feedback launch sign in",
    ]
}

fn normalize_hex(value: &str) -> Option<String> {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", digits.to_ascii_uppercase()))
}

pub fn open(launch: Launch, cx: &mut App) -> Result<()> {
    input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-f", OpenFind, Some("Preferences")),
        KeyBinding::new("escape", CloseFind, Some("Preferences")),
    ]);
    let title = match launch.view.as_str() {
        "history" => "Capture History",
        "feedback" => "Send feedback",
        "onboarding" => "Welcome to Captures",
        _ => "Preferences",
    };
    let bounds = Bounds::centered(None, size(px(880.), px(660.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| {
            let surface = cx.new(|cx| Surface::new(launch, cx));
            surface.read(cx).focus.focus(window);
            surface
        },
    )?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Preferences,
    History,
    Feedback,
    Onboarding,
}
struct Surface {
    focus: FocusHandle,
    launch: Launch,
    page: Page,
    settings: Settings,
    settings_load_error: Option<String>,
    preference_section: usize,
    preferences_scroll: ScrollHandle,
    find: Entity<TextInput>,
    find_open: bool,
    find_index: usize,
    open_select: Option<&'static str>,
    microphones: Vec<captures_recording::AudioDevice>,
    editing_shortcut: Option<usize>,
    shortcut_release_pending: bool,
    status: String,
    poster_pending: HashSet<PathBuf>,
    history_filter: &'static str,
    confirm: Option<PathBuf>,
    message: Entity<TextInput>,
    contact: Entity<TextInput>,
    feedback_busy: bool,
    feedback_category: &'static str,
    custom_accent: Entity<TextInput>,
    custom_signal: Entity<TextInput>,
}
impl Surface {
    fn new(launch: Launch, cx: &mut Context<Self>) -> Self {
        let loaded = settings::load(&launch.profile);
        let settings_load_error = loaded.as_ref().err().map(|error| format!("Couldn’t read settings: {error:#}. Fix or remove settings.json before changing preferences."));
        let settings = loaded.unwrap_or_default();
        let page = match launch.view.as_str() {
            "history" => Page::History,
            "feedback" => Page::Feedback,
            "onboarding" => Page::Onboarding,
            _ => Page::Preferences,
        };
        let find = cx.new(|cx| TextInput::new("", "Find settings", cx));
        cx.observe(&find, |_, _, cx| cx.notify()).detach();
        let custom_accent = settings.custom_theme.accent.to_uppercase();
        let custom_signal = settings.custom_theme.signal.to_uppercase();
        Self {
            focus: cx.focus_handle(),
            launch,
            page,
            settings,
            status: settings_load_error.clone().unwrap_or_default(),
            settings_load_error,
            preference_section: 0,
            preferences_scroll: ScrollHandle::new(),
            find,
            find_open: false,
            find_index: 0,
            open_select: None,
            microphones: Vec::new(),
            editing_shortcut: None,
            shortcut_release_pending: false,
            poster_pending: HashSet::new(),
            history_filter: "all",
            confirm: None,
            message: cx.new(|cx| {
                TextInput::new("", "What happened? What did you expect?", cx).multiline(8_000)
            }),
            contact: cx.new(|cx| TextInput::new("", "X handle, GitHub username, email…", cx)),
            feedback_busy: false,
            feedback_category: "bug",
            custom_accent: cx.new(|cx| TextInput::new(custom_accent, "#32D3FF", cx)),
            custom_signal: cx.new(|cx| TextInput::new(custom_signal, "#FF4FC3", cx)),
        }
    }
    fn persist(&mut self, cx: &mut Context<Self>) {
        if block_changes_after_load_error(&mut self.settings, self.settings_load_error.is_some()) {
            // A malformed file is deliberately read-only. Event handlers update the
            // model optimistically, so restore the only known-safe in-memory value.
            self.status = self.settings_load_error.clone().unwrap();
            cx.notify();
            return;
        }
        let previous = match settings::load(&self.launch.profile) {
            Ok(previous) => previous,
            Err(error) => {
                self.status = format!("Couldn’t read previous settings: {error:#}");
                cx.notify();
                return;
            }
        };
        self.status = match crate::integration::reconcile_settings(&self.settings, cx) {
            Err(error) => {
                self.settings = previous;
                format!("Couldn’t apply settings: {error:#}")
            }
            Ok(()) => match settings::save(&self.launch.profile, &self.settings) {
                Ok(()) => {
                    cx.set_global(crate::theme::CurrentSettings(self.settings.clone()));
                    cx.refresh_windows();
                    "Changes saved".into()
                }
                Err(error) => {
                    let rollback = crate::integration::reconcile_settings(&previous, cx);
                    self.settings = previous;
                    match rollback {
                        Ok(()) => format!("Couldn’t save changes: {error:#}"),
                        Err(rollback) => format!(
                            "Couldn’t save changes: {error:#}. Native rollback failed: {rollback:#}"
                        ),
                    }
                }
            },
        };
        cx.notify()
    }
    fn nav(&mut self, p: Page, cx: &mut Context<Self>) {
        self.page = p;
        self.status.clear();
        cx.notify()
    }
    fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Preferences {
            self.find_open = true;
            self.find.focus_handle(cx).focus(window);
            cx.notify();
        }
    }
    fn close_find(&mut self, _: &CloseFind, _: &mut Window, cx: &mut Context<Self>) {
        if self.find_open {
            self.find_open = false;
            self.find_index = 0;
            cx.notify();
        }
    }
    fn assign_shortcut(&mut self, index: usize, value: String) {
        *match index {
            0 => &mut self.settings.new_capture_shortcut,
            1 => &mut self.settings.region_shortcut,
            2 => &mut self.settings.window_shortcut,
            3 => &mut self.settings.display_shortcut,
            4 => &mut self.settings.recording.video_shortcut,
            5 => &mut self.settings.recording.window_shortcut,
            6 => &mut self.settings.recording.display_shortcut,
            _ => &mut self.settings.recording.gif_shortcut,
        } = value;
    }

    fn capture_keys(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.editing_shortcut {
            cx.stop_propagation();
            let key = event.keystroke.key.as_str();
            if matches!(
                key,
                "shift" | "control" | "alt" | "super" | "command" | "meta"
            ) {
                return;
            }
            if key != "escape" {
                let mut parts = Vec::new();
                let mods = event.keystroke.modifiers;
                if mods.platform {
                    parts.push(
                        if cfg!(target_os = "macos") {
                            "Command"
                        } else {
                            "Super"
                        }
                        .to_owned(),
                    );
                }
                if mods.control {
                    parts.push("Control".into());
                }
                if mods.alt {
                    parts.push("Alt".into());
                }
                if mods.shift {
                    parts.push("Shift".into());
                }
                parts.push(match key {
                    "printscreen" => "PrintScreen".into(),
                    "space" => "Space".into(),
                    _ => key.to_uppercase(),
                });
                let value = if key == "backspace" {
                    String::new()
                } else {
                    parts.join("+")
                };
                self.assign_shortcut(index, value);
            }
            self.editing_shortcut = None;
            self.shortcut_release_pending = true;
            cx.notify();
            return;
        }
        let shortcut = if cfg!(target_os = "macos") {
            event.keystroke.modifiers.platform
        } else {
            event.keystroke.modifiers.control
        };
        if shortcut && event.keystroke.key.eq_ignore_ascii_case("f") {
            self.open_find(&OpenFind, window, cx);
        } else if event.keystroke.key == "escape" {
            if self.open_select.take().is_some() {
                cx.notify();
            } else {
                self.close_find(&CloseFind, window, cx);
            }
        } else if self.find_open && event.keystroke.key == "enter" {
            let count = self.preference_matches(cx).len();
            if count > 0 {
                self.find_index = if event.keystroke.modifiers.shift {
                    (self.find_index + count - 1) % count
                } else {
                    (self.find_index + 1) % count
                };
                let section = self.preference_matches(cx)[self.find_index];
                self.preference_section = section;
                self.preferences_scroll.scroll_to_top_of_item(section);
                cx.notify();
            }
        }
    }

    fn preference_matches(&self, cx: &App) -> Vec<usize> {
        let query = self.find.read(cx).value().trim().to_ascii_lowercase();
        if query.is_empty() {
            return vec![];
        }
        preference_search_targets()
            .iter()
            .enumerate()
            .filter_map(|(index, target)| target.contains(&query).then_some(index))
            .collect()
    }

    fn choose_select(&mut self, key: &'static str, value: &str, cx: &mut Context<Self>) {
        match key {
            "microphone" => {
                self.settings.recording.microphone_device_id =
                    (!value.is_empty()).then(|| value.to_owned())
            }
            "format" => self.settings.screenshot_format = value.into(),
            "countdown" => self.settings.screenshot_countdown_seconds = value.parse().unwrap_or(0),
            "video-format" => self.settings.recording.video_format = value.into(),
            "video-fps" => self.settings.recording.video_fps = value.parse().unwrap_or(60),
            "video-resolution" => self.settings.recording.video_max_resolution = value.into(),
            "recording-countdown" => {
                self.settings.recording.countdown_seconds = value.parse().unwrap_or(3)
            }
            "gif-fps" => self.settings.recording.gif_fps = value.parse().unwrap_or(15),
            "gif-width" => self.settings.recording.gif_max_width = value.parse().unwrap_or(800),
            "gif-colors" => self.settings.recording.gif_max_colors = value.parse().unwrap_or(256),
            _ => return,
        }
        self.open_select = None;
        self.persist(cx);
    }

    fn menu_select(
        &self,
        key: &'static str,
        label: String,
        options: &[(&str, &str)],
        cx: &mut Context<Self>,
        t: Theme,
    ) -> Stateful<Div> {
        let open = self.open_select == Some(key);
        let mut control = self
            .select(
                SharedString::from(format!("select-{key}")),
                format!("{label}  ▾"),
                t,
            )
            .relative()
            .on_click(cx.listener(move |s, _, _, cx| {
                s.open_select = (s.open_select != Some(key)).then_some(key);
                if key == "microphone" && s.open_select.is_some() {
                    let task = cx.background_executor().spawn(async {
                        #[cfg(target_os = "macos")]
                        {
                            captures_recording_macos::microphone_devices()
                        }
                        #[cfg(any(target_os = "linux", target_os = "windows"))]
                        {
                            captures_recording_xcap::microphone_devices()
                        }
                    });
                    cx.spawn(async move |this, cx| {
                        let devices = task.await;
                        let _ = this.update(cx, |s, cx| {
                            s.microphones = devices;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }));
        if open {
            let mut menu = div()
                .absolute()
                .top(px(36.))
                .right_0()
                .min_w(px(150.))
                .p_1()
                .rounded(px(8.))
                .border_1()
                .border_color(t.border)
                .bg(t.raised)
                .shadow_lg()
                .flex()
                .flex_col();
            for (value, option_label) in options {
                let value = value.to_string();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("{key}-{value}")))
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .hover(|d| d.bg(t.hover))
                        .cursor_pointer()
                        .child(option_label.to_string())
                        .on_click(cx.listener(move |s, _, _, cx| {
                            cx.stop_propagation();
                            s.choose_select(key, &value, cx);
                        })),
                );
            }
            control = control.child(menu);
        }
        control
    }
    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        active: bool,
        t: Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded(px(7.))
            .border_1()
            .border_color(t.border)
            .bg(if active { t.hover } else { t.field })
            .text_color(t.text)
            .cursor_pointer()
            .child(label.into())
    }
    fn card(&self, title: &str, desc: &str, children: Vec<AnyElement>, t: Theme) -> Div {
        div()
            .w_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(14.))
            .border_1()
            .border_color(t.border)
            .bg(t.raised)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .max_w(px(347.))
                            .text_color(t.subtle)
                            .text_size(px(12.))
                            .line_height(px(16.2))
                            .child(desc.to_string()),
                    ),
            )
            .children(children.into_iter().enumerate().map(|(index, child)| {
                div()
                    .pt(px(if index == 0 { 2. } else { 12. }))
                    .when(index > 0, |row| row.border_t_1().border_color(t.border))
                    .child(child)
            }))
    }
    fn row(&self, label: &str, value: &str, t: Theme) -> Div {
        div()
            .flex()
            .justify_between()
            .items_center()
            .gap_3()
            .child(div().child(label.to_string()))
            .child(div().text_color(t.muted).child(value.to_string()))
    }
    fn setting_row(
        &self,
        title: &str,
        description: &str,
        control: impl IntoElement,
        t: Theme,
    ) -> Div {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .max_w(px(347.))
                            .text_size(px(12.))
                            .line_height(px(16.2))
                            .text_color(t.subtle)
                            .child(description.to_string()),
                    ),
            )
            .child(control)
    }
    fn toggle(&self, id: impl Into<ElementId>, on: bool, t: Theme) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(32.))
            .h(px(19.))
            .p(px(2.))
            .rounded(px(10.))
            .border_1()
            .border_color(if on { t.accent } else { t.border })
            .bg(if on { t.accent } else { t.canvas })
            .flex()
            .cursor_pointer()
            .child(
                div()
                    .ml(if on { px(13.) } else { px(0.) })
                    .size(px(13.))
                    .rounded(px(7.))
                    .bg(if on { rgb(0x242424) } else { t.subtle }),
            )
    }
    fn select(
        &self,
        id: impl Into<ElementId>,
        value: impl Into<SharedString>,
        t: Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .min_w(px(120.))
            .px_3()
            .py_2()
            .rounded(px(7.))
            .border_1()
            .border_color(t.border)
            .bg(t.field)
            .cursor_pointer()
            .child(value.into())
    }
    fn preferences(&mut self, cx: &mut Context<Self>, t: Theme) -> Div {
        let appearance = self.settings.appearance.clone();
        let format = self.settings.screenshot_format.clone();
        let vf = self.settings.recording.video_format.clone();
        let mut cards = vec![];
        let mut appearance_controls = div()
            .flex()
            .rounded(px(8.))
            .p(px(2.))
            .bg(t.canvas)
            .border_1()
            .border_color(t.border);
        for mode in ["system", "light", "dark"] {
            appearance_controls = appearance_controls.child(
                self.button(
                    SharedString::from(format!("appearance-{mode}")),
                    match mode {
                        "system" => "System",
                        "light" => "Light",
                        _ => "Dark",
                    },
                    appearance == mode,
                    t,
                )
                .py(px(6.))
                .text_size(px(12.))
                .line_height(px(16.))
                .border_0()
                .on_click(cx.listener(move |s, _, _, cx| {
                    s.settings.appearance = mode.into();
                    s.launch.light = mode != "dark";
                    s.persist(cx)
                })),
            );
        }
        let palette = [
            ("mustard", "Mustard", 0xffca28),
            ("ember", "Ember", 0xff7a45),
            ("rose", "Rose", 0xff5ba7),
            ("violet", "Violet", 0xc026d3),
            ("cobalt", "Cobalt", 0x2563eb),
            ("aqua", "Aqua", 0x31cbd8),
            ("mint", "Mint", 0x67d5a5),
            ("lime", "Lime", 0xb6db45),
            ("mono", "Mono", 0xededed),
            ("custom", "Custom", 0xe066ff),
        ];
        let mut swatches = div().grid().grid_cols(5).gap(px(4.)).text_size(px(12.));
        for (id, name, color) in palette {
            let active = self.settings.theme == id;
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("theme-{id}")))
                    .h(px(34.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if active { t.border } else { rgba(0x00000000) })
                    .bg(if active { t.field } else { rgba(0x00000000) })
                    .cursor_pointer()
                    .child(div().size(px(18.)).rounded(px(4.)).bg(rgb(color)))
                    .child(name)
                    .child(if active { "✓" } else { "" })
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.settings.theme = id.into();
                        s.persist(cx)
                    })),
            );
        }
        let mut appearance_children = vec![
            self.setting_row("Interface theme","Follow the system setting, or lock Captures to light or dark.",appearance_controls,t).into_any_element(),
            div().flex().flex_col().gap(px(4.)).child(div().font_weight(FontWeight::MEDIUM).child("Accent color")).child(div().max_w(px(347.)).text_size(px(12.)).line_height(px(16.2)).text_color(t.subtle).child("Used for the capture action, selection, and focus. Status colors keep their meaning.")).child(div().pt(px(4.)).child(swatches)).into_any_element(),
        ];
        if self.settings.theme == "custom" {
            appearance_children.push(div().flex().flex_col().gap_3().child(div().font_weight(FontWeight::MEDIUM).child("Custom colors")).child(div().text_size(px(12.)).text_color(t.subtle).child("Open either RGB picker or enter a hex value. Supporting shades stay readable.")).children(vec![
                self.setting_row("Accent", "Capture actions, selections, focus, and editing.", div().w(px(150.)).child(self.custom_accent.clone()), t).into_any_element(),
                self.setting_row("Recording signal", "Recording indicators, errors, and destructive actions.", div().w(px(150.)).child(self.custom_signal.clone()), t).into_any_element(),
                div().flex().justify_end().gap_2()
                    .child(self.button("reset-colors", "Reset colors", false, t).on_click(cx.listener(|s,_,_,cx| {
                        s.settings.custom_theme = Default::default();
                        s.custom_accent.update(cx, |input, cx| *input = TextInput::new("#32D3FF", "#32D3FF", cx));
                        s.custom_signal.update(cx, |input, cx| *input = TextInput::new("#FF4FC3", "#FF4FC3", cx));
                        s.persist(cx)
                    })))
                    .child(self.button("apply-colors", "Apply colors", true, t).on_click(cx.listener(|s,_,_,cx| {
                        let accent = normalize_hex(&s.custom_accent.read(cx).value());
                        let signal = normalize_hex(&s.custom_signal.read(cx).value());
                        match (accent, signal) {
                            (Some(accent), Some(signal)) => { s.settings.custom_theme.accent = accent; s.settings.custom_theme.signal = signal; s.persist(cx); }
                            _ => { s.status = "Enter colors as six-digit hex values, such as #32D3FF.".into(); cx.notify(); }
                        }
                    }))).into_any_element(),
            ]).into_any_element());
        }
        cards.push(self.card("Appearance","One look across every Captures window. Capture overlays stay dark so they read on any desktop.", appearance_children,t));
        let capture_toggles = [
            (
                "copy",
                "Automatically copy captures to the clipboard",
                self.settings.auto_copy_to_clipboard,
            ),
            (
                "auto",
                "Start capture as soon as a target is selected",
                self.settings.auto_start_on_selection,
            ),
            (
                "preview",
                "Show mini previews after screenshots",
                self.settings.show_mini_previews,
            ),
            (
                "include-preview",
                "Show mini previews in screenshots and recordings",
                self.settings.include_mini_previews_in_captures,
            ),
            (
                "controls",
                "Show recording controls in captures",
                self.settings.include_recording_controls_in_captures,
            ),
            (
                "freeze",
                "Freeze screen when capturing",
                self.settings.freeze_screen,
            ),
            (
                "cursor-shot",
                "Show cursor in screenshots",
                self.settings.show_cursor_in_screenshots,
            ),
        ];
        let mut capture: Vec<AnyElement> = vec![
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child("Save captures to")
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            self.select("output-path", self.settings.output_directory.clone(), t)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            self.select("choose-directory", "Choose…", t)
                                .min_w(px(80.))
                                .on_click(cx.listener(|_s, _, window, cx| {
                                    let receiver = cx.prompt_for_paths(PathPromptOptions {
                                        files: false,
                                        directories: true,
                                        multiple: false,
                                        prompt: Some("Choose capture folder".into()),
                                    });
                                    cx.spawn_in(window, async move |this, cx| {
                                        if let Ok(Ok(Some(paths))) = receiver.await {
                                            let _ = this.update(cx, |s, cx| {
                                                if let Some(path) = paths.first() {
                                                    s.settings.output_directory =
                                                        path.to_string_lossy().into_owned();
                                                    s.persist(cx)
                                                }
                                            });
                                        }
                                    })
                                    .detach();
                                })),
                        ),
                )
                .into_any_element(),
        ];
        for (id, label, on) in capture_toggles {
            capture.push(
                self.setting_row(label, match id {"copy"=>"Turn this off to preserve existing text or other clipboard contents.","auto"=>"Drawing a region, choosing a window, or clicking Full screen immediately starts the capture.","preview"=>"Turn this off to keep the quick-access preview stack hidden.","freeze"=>"Holds hover states, tooltips, menus, and motion still while you choose a target.","cursor-shot"=>"Includes the pointer in still captures.",_=>"Choose whether Captures interface surfaces appear in captured media."}, self.toggle(id,on,t)
                .on_click(cx.listener(move |s, _, _, cx| {
                    match id {
                        "copy" => {
                            s.settings.auto_copy_to_clipboard = !s.settings.auto_copy_to_clipboard
                        }
                        "auto" => {
                            s.settings.auto_start_on_selection = !s.settings.auto_start_on_selection
                        }
                        "preview" => s.settings.show_mini_previews = !s.settings.show_mini_previews,
                        "include-preview" => {
                            s.settings.include_mini_previews_in_captures =
                                !s.settings.include_mini_previews_in_captures
                        }
                        "controls" => {
                            s.settings.include_recording_controls_in_captures =
                                !s.settings.include_recording_controls_in_captures
                        }
                        "freeze" => s.settings.freeze_screen = !s.settings.freeze_screen,
                        _ => {
                            s.settings.show_cursor_in_screenshots =
                                !s.settings.show_cursor_in_screenshots
                        }
                    };
                    s.persist(cx)
                })),t)
                .into_any_element(),
            )
        }
        capture.push(
            self.setting_row(
                "Screenshot format",
                "Used when you save or export. Capture History keeps a lossless PNG until then.",
                self.menu_select(
                    "format",
                    format.to_uppercase(),
                    &[("png", "PNG"), ("jpeg", "JPEG"), ("webp", "WebP")],
                    cx,
                    t,
                ),
                t,
            )
            .into_any_element(),
        );
        capture.push(
            self.setting_row(
                "Screenshot countdown",
                "Wait before capturing so you can open menus or hover states. Press Esc to cancel.",
                self.menu_select(
                    "countdown",
                    if self.settings.screenshot_countdown_seconds == 0 {
                        "Off".into()
                    } else {
                        format!("{} seconds", self.settings.screenshot_countdown_seconds)
                    },
                    &[
                        ("0", "Off"),
                        ("1", "1 second"),
                        ("3", "3 seconds"),
                        ("5", "5 seconds"),
                        ("10", "10 seconds"),
                    ],
                    cx,
                    t,
                ),
                t,
            )
            .into_any_element(),
        );
        cards.push(self.card(
            "Capture",
            "Where captures go and what happens right after you take one.",
            capture,
            t,
        ));
        let mut shortcuts = vec![];
        for (index, (n, v)) in [
            ("New Capture", &self.settings.new_capture_shortcut),
            ("Region", &self.settings.region_shortcut),
            ("Window", &self.settings.window_shortcut),
            ("Full Screen", &self.settings.display_shortcut),
            ("Record Region", &self.settings.recording.video_shortcut),
            ("Record Window", &self.settings.recording.window_shortcut),
            (
                "Record Full Screen",
                &self.settings.recording.display_shortcut,
            ),
            ("Record GIF", &self.settings.recording.gif_shortcut),
        ]
        .into_iter()
        .enumerate()
        {
            let value = if self.editing_shortcut == Some(index) {
                "Press shortcut…"
            } else if v.is_empty() {
                "Not set"
            } else {
                v.as_str()
            };
            shortcuts.push(
                self.setting_row(
                    n,
                    "",
                    self.button(
                        SharedString::from(format!("shortcut-{index}")),
                        value.to_owned(),
                        self.editing_shortcut == Some(index),
                        t,
                    )
                    .on_click(cx.listener(move |s, _, window, cx| {
                        s.editing_shortcut = Some(index);
                        s.focus.focus(window);
                        crate::integration::set_shortcut_capture(true, cx);
                        cx.spawn(async |this, cx| {
                            loop {
                                Timer::after(std::time::Duration::from_millis(16)).await;
                                let active = this
                                    .update(cx, |s, cx| {
                                        if let Some(index) = s.editing_shortcut
                                            && let Some(value) =
                                                crate::integration::take_captured_shortcut(cx)
                                        {
                                            s.assign_shortcut(index, value);
                                            s.editing_shortcut = None;
                                            s.persist(cx);
                                        }
                                        s.editing_shortcut.is_some() || s.shortcut_release_pending
                                    })
                                    .unwrap_or(false);
                                if !active {
                                    let _ = cx.update(|cx| {
                                        crate::integration::set_shortcut_capture(false, cx)
                                    });
                                    break;
                                }
                            }
                        })
                        .detach();
                        cx.notify();
                    })),
                    t,
                )
                .into_any_element(),
            )
        }
        shortcuts.push(div().text_color(t.subtle).text_size(px(12.)).child(if crate::integration::global_shortcuts_supported() { "Click a shortcut to change it. Escape cancels; Backspace clears. Conflicts preserve the previous shortcut." } else { "System-wide shortcuts are unavailable on Wayland. Use the tray or app controls." }).into_any_element());
        cards.push(self.card(
            "Shortcuts",
            "Capture from anywhere without opening a window.",
            shortcuts,
            t,
        ));
        let microphones = std::iter::once(("", "Off"))
            .chain(
                self.microphones
                    .iter()
                    .map(|device| (device.id.as_str(), device.name.as_str())),
            )
            .collect::<Vec<_>>();
        let microphone_label = self
            .settings
            .recording
            .microphone_device_id
            .as_ref()
            .map(|id| {
                self.microphones
                    .iter()
                    .find(|device| &device.id == id)
                    .map(|device| device.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .unwrap_or_else(|| "Off".into());
        let mut recording = vec![self.setting_row("Recording format", "Recordings are captured as H.264 MP4. GIF and WebM are converted when you save or export.", self.menu_select("video-format", vf.to_uppercase(), &[("mp4","MP4"),("gif","GIF"),("webm","WebM")], cx, t), t).into_any_element(),
            self.setting_row("Frames per second", "Default recording frame rate.", self.menu_select("video-fps", format!("{} FPS", self.settings.recording.video_fps), &[("60","60 FPS"),("30","30 FPS"),("15","15 FPS")], cx, t), t).into_any_element(),
            self.setting_row("Maximum resolution", "Scale recordings while preserving aspect ratio.", self.menu_select("video-resolution", match self.settings.recording.video_max_resolution.as_str() {"p1080"=>"1080p", "p720"=>"720p", _=>"Original"}.into(), &[("original","Original"),("p1080","1080p"),("p720","720p")], cx, t), t).into_any_element(),
            self.setting_row("Countdown", "Delay before a recording starts.", self.menu_select("recording-countdown", if self.settings.recording.countdown_seconds == 0 {"Off".into()} else {format!("{} seconds", self.settings.recording.countdown_seconds)}, &[("0","Off"),("1","1 second"),("3","3 seconds"),("5","5 seconds"),("10","10 seconds")], cx, t), t).into_any_element(),
            self.setting_row("Default microphone", "Used when a recording starts with microphone audio.", self.menu_select("microphone", microphone_label, &microphones, cx, t), t).into_any_element(),
        ];
        for (id, label, on) in [
            (
                "desktop-audio",
                "Record desktop audio",
                self.settings.recording.capture_system_audio,
            ),
            (
                "mono",
                "Export recording audio in mono",
                self.settings.recording.mono_audio,
            ),
            (
                "record-cursor",
                "Show cursor in recordings",
                self.settings.recording.show_cursor,
            ),
            (
                "clicks",
                "Show clicks in recordings",
                self.settings.recording.highlight_clicks,
            ),
            (
                "editor",
                "Open editor after recording",
                self.settings.recording.open_editor_after_recording,
            ),
        ] {
            recording.push(
                self.setting_row(label, match id {"desktop-audio"=>"Records sound playing through the system output.","editor"=>"The recording is kept in Capture History for 30 days, so closing the editor never loses it.",_=>"Default for new screen recordings."},self.toggle(id,on,t)
                .on_click(cx.listener(move |s, _, _, cx| {
                    match id {
                        "desktop-audio" => {
                            s.settings.recording.capture_system_audio =
                                !s.settings.recording.capture_system_audio
                        }
                        "mono" => {
                            s.settings.recording.mono_audio = !s.settings.recording.mono_audio
                        }
                        "record-cursor" => {
                            s.settings.recording.show_cursor = !s.settings.recording.show_cursor
                        }
                        "clicks" => {
                            s.settings.recording.highlight_clicks =
                                !s.settings.recording.highlight_clicks
                        }
                        _ => {
                            s.settings.recording.open_editor_after_recording =
                                !s.settings.recording.open_editor_after_recording
                        }
                    };
                    s.persist(cx)
                })),t)
                .into_any_element(),
            )
        }
        cards.push(self.card(
            "Recording",
            "Defaults for new screen recordings. You can still change them in the capture menu.",
            recording,
            t,
        ));
        cards.push(self.card(
            "GIF export",
            "Starting point when a recording is exported as an animated GIF.",
            vec![
                    self.setting_row(
                        "Frames per second",
                        "Animation frame rate.",
                        self.select(
                            "gif-fps",
                            format!("{} FPS", self.settings.recording.gif_fps),
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            let values = [8, 10, 12, 15, 20, 24, 30];
                            let i = values
                                .iter()
                                .position(|v| *v == s.settings.recording.gif_fps)
                                .unwrap_or(2);
                            s.settings.recording.gif_fps = values[(i + 1) % values.len()];
                            s.persist(cx)
                        })),
                        t,
                    )
                    .into_any_element(),
                    self.setting_row(
                        "Maximum width",
                        "Scale wide exports while preserving aspect ratio.",
                        self.select(
                            "gif-width",
                            format!("{} px", self.settings.recording.gif_max_width),
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            let v = [320, 480, 640, 800, 1200];
                            let i = v
                                .iter()
                                .position(|x| *x == s.settings.recording.gif_max_width)
                                .unwrap_or(3);
                            s.settings.recording.gif_max_width = v[(i + 1) % v.len()];
                            s.persist(cx)
                        })),
                        t,
                    )
                    .into_any_element(),
                    self.setting_row(
                        "Palette colors",
                        "Maximum colors in each GIF palette.",
                        self.select(
                            "gif-colors",
                            self.settings.recording.gif_max_colors.to_string(),
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            let v = [64, 96, 128, 256];
                            let i = v
                                .iter()
                                .position(|x| *x == s.settings.recording.gif_max_colors)
                                .unwrap_or(3);
                            s.settings.recording.gif_max_colors = v[(i + 1) % v.len()];
                            s.persist(cx)
                        })),
                        t,
                    )
                    .into_any_element(),
                ],
            t,
        ));
        cards.push(self.card(
            "Updates",
            "Keep Captures current and choose how much detail appears when an update is ready.",
            vec![
                    self.setting_row(
                        "Show what changed",
                        "List every Preview since your installed version in update notices.",
                        self.toggle("changelog", self.settings.show_update_changelog, t)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.settings.show_update_changelog =
                                    !s.settings.show_update_changelog;
                                s.persist(cx)
                            })),
                        t,
                    )
                    .into_any_element(),
                    self.setting_row(
                        "Preview updates",
                        "Automatic per-merge builds are experimental Previews.",
                        self.select("check-update", "Check for updates", t),
                        t,
                    )
                    .into_any_element(),
                ],
            t,
        ));
        cards.push(self.card("About", "Captures is in active development. Telling us what breaks is the fastest way to fix it.", vec![
            self.setting_row("Send feedback", "Report a bug or share an idea.", self.select("about-feedback","Open",t).on_click(cx.listener(|s,_,_,cx|s.nav(Page::Feedback,cx))),t).into_any_element(),
            self.setting_row("Launch Captures when I sign in", "Start quietly and keep capture shortcuts ready.", self.toggle("launch-login",self.settings.launch_at_login,t).on_click(cx.listener(|s,_,_,cx|{s.settings.launch_at_login = !s.settings.launch_at_login;s.persist(cx)})),t).into_any_element(),
        ],t));
        let query = if self.find_open {
            self.find.read(cx).value().trim().to_ascii_lowercase()
        } else {
            String::new()
        };
        let searchable = [
            "appearance interface theme accent color",
            "capture save clipboard previews screenshot cursor",
            "shortcuts keyboard",
            "recording audio cursor clicks editor",
            "gif export frames palette width",
            "updates preview changelog",
            "about feedback launch sign in",
        ];
        let match_count = if query.is_empty() {
            0
        } else {
            searchable
                .iter()
                .filter(|text| text.contains(&query))
                .count()
        };
        if !query.is_empty() {
            cards = cards
                .into_iter()
                .zip(searchable)
                .filter_map(|(card, text)| text.contains(&query).then_some(card))
                .collect();
        }
        div()
            .flex()
            .size_full()
            .child(self.sidebar(Page::Preferences, cx, t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .min_h(px(70.))
                            .px(px(24.))
                            .border_b_1()
                            .border_color(t.border)
                            .items_center()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .child("Preferences")
                                            .text_size(px(18.))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        div()
                                            .child(if self.status.is_empty() {
                                                "Changes save automatically.".to_owned()
                                            } else {
                                                self.status.clone()
                                            })
                                            .text_size(px(12.))
                                            .text_color(if self.status.starts_with("Couldn’t") {
                                                t.signal
                                            } else {
                                                t.subtle
                                            }),
                                    ),
                            )
                            .child(
                                div().flex().items_center().gap_3().child(
                                    self.button("history-top", "Capture History…", false, t)
                                        .on_click(
                                            cx.listener(|s, _, _, cx| s.nav(Page::History, cx)),
                                        ),
                                ),
                            )
                            .when(self.find_open, |header| {
                                header.child(
                                    div()
                                        .ml_4()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().w(px(260.)).child(self.find.clone()))
                                        .child(
                                            div()
                                                .min_w(px(54.))
                                                .text_size(px(12.))
                                                .text_color(t.subtle)
                                                .child(if query.is_empty() {
                                                    "".into()
                                                } else {
                                                    format!("{match_count} found")
                                                }),
                                        )
                                        .child(self.button("close-find", "×", false, t).on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.find_open = false;
                                                cx.notify();
                                            }),
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("preferences-scroll")
                            .flex_1()
                            .w_full()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.preferences_scroll)
                            .max_w(px(720.))
                            .p(px(24.))
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .children(cards),
                    ),
            )
    }
    fn sidebar(&self, active: Page, cx: &mut Context<Self>, t: Theme) -> Div {
        let mut d = div()
            .w(px(196.))
            .flex_none()
            .h_full()
            .py(px(16.))
            .px(px(12.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .border_r_1()
            .border_color(t.border)
            .bg(t.canvas)
            .child(
                div()
                    .mb_4()
                    .font_weight(FontWeight::BOLD)
                    .child("◉  Captures"),
            );
        for (index, name) in [
            "Appearance",
            "Capture",
            "Shortcuts",
            "Recording",
            "GIF export",
            "Updates",
            "About",
        ]
        .into_iter()
        .enumerate()
        {
            d = d.child(
                self.button(
                    ("nav-section", index),
                    name,
                    active == Page::Preferences && self.preference_section == index,
                    t,
                )
                .w_full()
                .border_0()
                .h(px(32.))
                .py_0()
                .flex()
                .items_center()
                .bg(
                    if active == Page::Preferences && self.preference_section == index {
                        t.hover
                    } else {
                        rgba(0)
                    },
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    s.page = Page::Preferences;
                    s.preference_section = index;
                    s.preferences_scroll.scroll_to_top_of_item(index);
                    cx.notify()
                })),
            )
        }
        d
    }
    fn history_files(&mut self) -> Vec<PathBuf> {
        let entries = match history::load(&self.launch.profile) {
            Ok(entries) => entries,
            Err(error) => {
                self.status = format!("Couldn’t read Capture History: {error:#}");
                return Vec::new();
            }
        };
        entries
            .into_iter()
            .map(|entry| entry.path)
            .filter(|path| {
                let extension = path
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                self.history_filter == "all"
                    || (self.history_filter == "screenshot"
                        && matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp"))
                    || (self.history_filter == "video"
                        && matches!(extension.as_str(), "mp4" | "webm"))
                    || (self.history_filter == "gif" && extension == "gif")
            })
            .collect()
    }
    fn history(&mut self, cx: &mut Context<Self>, t: Theme) -> Stateful<Div> {
        let files = self.history_files();
        let mut list = div().grid().grid_cols(3).gap_4();
        if files.is_empty() {
            list = list.child(self.card(
                "No captures yet",
                "Screenshots and recordings stored in this profile will appear here.",
                vec![],
                t,
            ))
        }
        for p in files {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let open = p.clone();
            let del = p.clone();
            let confirmed = self.confirm.as_ref() == Some(&p);
            let metadata = fs::metadata(&p).ok();
            let detail = metadata
                .as_ref()
                .map(|m| format!("{} · {:.1} KB", p.display(), m.len() as f64 / 1024.))
                .unwrap_or_else(|| p.display().to_string());
            let extension = p
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let is_video = matches!(extension.as_str(), "mp4" | "webm");
            let poster = self
                .launch
                .profile
                .join("history-posters")
                .join(format!("{name}.png"));
            if is_video && !poster.is_file() && self.poster_pending.insert(p.clone()) {
                let source = p.clone();
                let destination = poster.clone();
                let task = cx.background_executor().spawn(async move {
                    fs::create_dir_all(destination.parent().unwrap())?;
                    captures_media::MediaToolchain::from_command_names().create_poster(
                        &source,
                        &destination,
                        &captures_media::CancelToken::default(),
                    )?;
                    anyhow::Ok((source, destination))
                });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |surface, cx| {
                        match result {
                            Ok((source, _)) => {
                                surface.poster_pending.remove(&source);
                            }
                            Err(error) => {
                                surface.status = format!("Couldn’t create video poster: {error:#}")
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            let actions = div().flex().gap_2().child(
                self.button(
                    SharedString::from(format!("open-{name}")),
                    "Open editor",
                    false,
                    t,
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    let mut l = s.launch.clone();
                    l.path = Some(open.clone());
                    if let Err(e) = crate::open_view(editor_view_for_path(&open), l, cx) {
                        s.status = format!("Couldn’t open editor: {e:#}");
                        cx.notify()
                    }
                })),
            );
            let actions = actions.child(
                self.button(
                    SharedString::from(format!("delete-{name}")),
                    if confirmed {
                        "Confirm delete"
                    } else {
                        "Delete…"
                    },
                    confirmed,
                    t,
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    if s.confirm.as_ref() == Some(&del) {
                        match history::delete(&s.launch.profile, &del) {
                            Ok(_) => s.status = "Capture deleted".into(),
                            Err(e) => s.status = format!("Couldn’t delete: {e}"),
                        };
                        s.confirm = None
                    } else {
                        s.confirm = Some(del.clone())
                    }
                    cx.notify()
                })),
            );
            list = list.child(self.card(
                &name,
                &detail,
                vec![
                    if is_image_path(&p) || extension == "gif" {
                        div()
                            .h(px(180.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(8.))
                            .bg(t.canvas)
                            .child(img(p.clone()).size_full().object_fit(ObjectFit::Contain))
                            .into_any_element()
                    } else if poster.is_file() {
                        div()
                            .h(px(180.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(8.))
                            .bg(t.canvas)
                            .child(img(poster).size_full().object_fit(ObjectFit::Contain))
                            .into_any_element()
                    } else {
                        div()
                            .h(px(180.))
                            .w_full()
                            .rounded(px(8.))
                            .bg(t.canvas)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("Recording")
                            .into_any_element()
                    },
                    actions.into_any_element(),
                ],
                t,
            ))
        }
        let mut filters = div().flex().gap_2();
        for filter in ["all", "screenshot", "video", "gif"] {
            filters = filters.child(
                self.button(
                    SharedString::from(format!("history-filter-{filter}")),
                    match filter {
                        "all" => "All",
                        "screenshot" => "Screenshots",
                        "video" => "Video",
                        _ => "GIF",
                    },
                    self.history_filter == filter,
                    t,
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    s.history_filter = filter;
                    cx.notify()
                })),
            );
        }
        div()
            .id("history-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(28.))
                            .font_weight(FontWeight::BOLD)
                            .child("Capture History"),
                    )
                    .child(
                        self.button("back-preferences", "Preferences…", false, t)
                            .on_click(cx.listener(|s, _, _, cx| s.nav(Page::Preferences, cx))),
                    ),
            )
            .child(div().text_size(px(11.)).text_color(t.subtle).child("ON THIS DEVICE"))
            .child(div().text_color(t.muted).child("Screenshots, videos, GIFs, and interrupted recordings you can recover all appear here for 30 days."))
            .child(filters)
            .children(self.recording_drafts(cx, t))
            .child(list)
            .child(self.status.clone())
    }

    fn recording_drafts(&self, cx: &mut Context<Self>, t: Theme) -> Vec<AnyElement> {
        let store =
            captures_recording::DraftStore::new(self.launch.profile.join("recording-drafts"));
        let Ok(drafts) = store.list() else {
            return vec![];
        };
        if drafts.is_empty() {
            return vec![];
        }
        let mut rows = div().flex().flex_col().gap_3();
        for draft in drafts {
            let id = draft.session_id.clone();
            let discard = id.clone();
            let segment_count = draft
                .segments
                .iter()
                .filter(|segment| segment.complete)
                .count();
            let copy = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(FontWeight::MEDIUM).child(
                    if draft.options.kind == captures_recording::RecordingKind::Gif {
                        "Interrupted GIF recording"
                    } else {
                        "Interrupted video recording"
                    },
                ))
                .child(div().text_size(px(12.)).text_color(t.subtle).child(format!(
                    "{segment_count} playable segment{} · recover in the recording editor",
                    if segment_count == 1 { "" } else { "s" }
                )));
            let actions = div()
                .flex()
                .gap_2()
                .child(
                    self.button(
                        SharedString::from(format!("recover-{id}")),
                        "Recover",
                        true,
                        t,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        let mut launch = s.launch.clone();
                        launch.path = Some(s.launch.profile.join("recording-drafts").join(&id));
                        if let Err(e) = crate::open_view("recording-editor", launch, cx) {
                            s.status = format!("Couldn’t recover recording: {e:#}");
                            cx.notify()
                        }
                    })),
                )
                .child(
                    self.button(
                        SharedString::from(format!("discard-{discard}")),
                        "Discard",
                        false,
                        t,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        let store = captures_recording::DraftStore::new(
                            s.launch.profile.join("recording-drafts"),
                        );
                        s.status = match store.remove(&discard) {
                            Ok(_) => "Interrupted recording discarded.".into(),
                            Err(e) => format!("Couldn’t discard recording: {e}"),
                        };
                        cx.notify()
                    })),
                );
            rows = rows.child(
                div()
                    .p_4()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(copy)
                    .child(actions),
            );
        }
        vec![self.card("Recording recovery","These recordings stopped before Captures could finish saving them. Recover one to add its playable segments to Capture History, or discard it.",vec![rows.into_any_element()],t).into_any_element()]
    }
    fn feedback(&mut self, cx: &mut Context<Self>, t: Theme) -> Stateful<Div> {
        let mut categories = div().grid().grid_cols(3).gap_3();
        for (id, label, description) in [
            ("bug", "Bug", "Something is broken or unexpected"),
            ("idea", "Idea", "A feature or improvement"),
            ("other", "Other", "Anything else"),
        ] {
            categories = categories.child(
                div()
                    .id(SharedString::from(format!("feedback-{id}")))
                    .min_h(px(62.))
                    .p_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if self.feedback_category == id {
                        t.accent
                    } else {
                        t.border
                    })
                    .bg(if self.feedback_category == id {
                        t.hover
                    } else {
                        t.raised
                    })
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_weight(FontWeight::MEDIUM).child(label))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.subtle)
                            .child(description),
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.feedback_category = id;
                        let placeholder = match id {
                            "bug" => "What happened? What did you expect?",
                            "idea" => "What's the idea? What problem would it solve?",
                            _ => "What would you like us to know?",
                        };
                        s.message
                            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
                        cx.notify();
                    })),
            );
        }
        let submit = self
            .button(
                "send",
                if self.feedback_busy {
                    "Sending…"
                } else {
                    "Send feedback"
                },
                true,
                t,
            )
            .on_click(cx.listener(|s, _, _, cx| {
                if s.feedback_busy {
                    return;
                }
                let message = s.message.read(cx).value();
                let contact = s.contact.read(cx).value();
                if message.trim().is_empty() {
                    s.status = "Please enter a message.".into();
                    cx.notify();
                    return;
                }
                s.feedback_busy = true;
                let category = s.feedback_category.to_owned();
                let endpoint = std::env::var("CAPTURES_FEEDBACK_URL")
                    .unwrap_or_else(|_| captures_feedback::DEFAULT_FEEDBACK_URL.into());
                let task = cx.background_executor().spawn(async move {
                    captures_feedback::FeedbackClient::new(&endpoint).and_then(|c| {
                        c.submit(
                            captures_feedback::FeedbackDraft {
                                message: message.trim().into(),
                                contact: (!contact.trim().is_empty())
                                    .then(|| contact.trim().into()),
                                category,
                            },
                            captures_feedback::FeedbackContext {
                                app_version: env!("CARGO_PKG_VERSION").into(),
                                os: std::env::consts::OS.into(),
                                os_version: "unknown".into(),
                                arch: std::env::consts::ARCH.into(),
                            },
                        )
                    })
                });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |s, cx| {
                        s.feedback_busy = false;
                        s.status = match result {
                            Ok(_) => "Thanks — feedback sent.".into(),
                            Err(e) => e,
                        };
                        cx.notify()
                    });
                })
                .detach();
                s.status = "Sending…".into();
                cx.notify()
            }));
        div().id("feedback-scroll").size_full().overflow_y_scroll().bg(t.canvas).child(div().max_w(px(640.)).mx_auto().p_8().flex().flex_col().gap_5()
            .child(div().text_size(px(11.)).text_color(t.subtle).child("CAPTURES"))
            .child(div().text_size(px(28.)).font_weight(FontWeight::BOLD).child("Send feedback"))
            .child(div().text_color(t.subtle).line_height(px(19.)).child("Tell us what broke, what is missing, or what you wish worked better. Captures sends what you type here plus the app and system details listed below."))
            .child(self.card("Category","Choose the closest match.",vec![categories.into_any_element(),div().flex().flex_col().gap_2().child("Message").child(div().h(px(150.)).child(self.message.clone())).into_any_element(),div().flex().flex_col().gap_2().child("Contact  ·  OPTIONAL").child(self.contact.clone()).child(div().text_size(px(11.)).text_color(t.subtle).child("Optional — we may use this if we need to ask a follow-up question.")).into_any_element()],t))
            .child(self.card("Included automatically","",vec![self.row("App version",env!("CARGO_PKG_VERSION"),t).into_any_element(),self.row("System",&format!("{} · {}",std::env::consts::OS,std::env::consts::ARCH),t).into_any_element()],t))
            .child(div().flex().items_center().justify_between().child(div().text_color(if self.status.starts_with("Thanks"){t.positive}else{t.signal}).child(self.status.clone())).child(submit)))
    }
    fn onboarding(&mut self, cx: &mut Context<Self>, t: Theme) -> Div {
        let screen_ready = captures_capture::XcapBackend
            .ensure_permission(false)
            .is_ok();
        let platform = std::env::consts::OS;
        let description = match platform {
            "macos" => {
                "This allows Captures to read the pixels you choose to capture. macOS keeps everything else hidden."
            }
            "windows" => {
                "Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private."
            }
            _ => {
                "Your desktop may show its own screen-sharing picker when a capture starts. There is nothing to approve ahead of time."
            }
        };
        let permission_action = if screen_ready {
            self.button("screen-ready", "✓  Ready", false, t)
        } else {
            self.button(
                "screen-permission",
                if cfg!(target_os = "macos") {
                    "Allow access"
                } else {
                    "Check again"
                },
                false,
                t,
            )
            .on_click(cx.listener(|s, _, _, cx| {
                s.status = match captures_capture::XcapBackend.ensure_permission(true) {
                    Ok(_) => "Screen capture access is ready.".into(),
                    Err(e) => format!("Screen access still needs approval: {e}"),
                };
                cx.notify()
            }))
        };
        let permission = div()
            .p_5()
            .grid()
            .grid_cols(3)
            .gap_4()
            .items_start()
            .child(div().text_size(px(20.)).child("▣"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Screen capture"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(t.subtle)
                            .child(description),
                    ),
            )
            .child(permission_action);
        let permissions = div()
            .rounded(px(14.))
            .border_1()
            .border_color(t.border)
            .bg(t.raised)
            .child(permission);
        #[cfg(target_os = "macos")]
        let permissions = {
            let mic_ready = captures_recording_macos::microphone_authorized();
            permissions.child(div().border_t_1().border_color(t.border).p_5().grid().grid_cols(3).gap_4().child("♩").child(div().child("Microphone  ·  Optional").child(div().text_size(px(12.)).text_color(t.subtle).child("Allow it now so a recording does not pause to ask, or wait until you pick a mic."))).child(self.button("mic-permission",if mic_ready{"✓  Granted"}else{"Allow microphone"},false,t).on_click(cx.listener(|s,_,_,cx|{if !captures_recording_macos::microphone_authorized(){captures_recording_macos::request_microphone_access();}s.status="Microphone permission status refreshed.".into();cx.notify()}))))
        };
        let stage = div().max_w(px(620.)).h_full().mx_auto().p_8().flex().flex_col().justify_center().gap_6()
            .child(div().size(px(40.)).rounded(px(10.)).bg(t.accent).flex().items_center().justify_center().text_size(px(22.)).child("⌖"))
            .child(div().text_size(px(11.)).text_color(t.subtle).child("WELCOME TO CAPTURES"))
            .child(div().text_size(px(28.)).font_weight(FontWeight::BOLD).child(if cfg!(target_os="macos"){"Required permissions"}else{"You’re ready to capture"}))
            .child(div().text_color(t.subtle).line_height(px(19.)).child("Captures only reads the pixels you choose to capture. Nothing is uploaded, and nothing leaves this computer unless you send it somewhere."))
            .child(permissions).child(div().text_color(t.signal).child(self.status.clone()))
            .child(div().flex().justify_end().child(self.button("finish","Start capturing",true,t).on_click(cx.listener(|s,_,_,cx|{s.settings.onboarding_completed=true;s.persist(cx);s.nav(Page::Preferences,cx)}))));
        div().size_full().bg(t.canvas).child(stage)
    }
}
impl Render for Surface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::from_settings(&self.settings, &self.launch, window);
        div()
            .key_context("Preferences")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::capture_keys))
            .on_key_up(cx.listener(|s, _, _, cx| {
                if std::mem::take(&mut s.shortcut_release_pending) {
                    // Register only after release, otherwise the OS steals the
                    // key-up event and leaves shortcut recording suppressed.
                    s.persist(cx);
                    crate::integration::set_shortcut_capture(false, cx);
                }
            }))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::close_find))
            .font_family(font())
            .text_size(px(13.))
            .text_color(t.text)
            .bg(t.canvas)
            .size_full()
            .child(match self.page {
                Page::Preferences => self.preferences(cx, t).into_any_element(),
                Page::History => self.history(cx, t).into_any_element(),
                Page::Feedback => self.feedback(cx, t).into_any_element(),
                Page::Onboarding => self.onboarding(cx, t).into_any_element(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn editor_routes_by_capture_extension() {
        assert_eq!(
            editor_view_for_path(std::path::Path::new("shot.PNG")),
            "screenshot-editor"
        );
        assert_eq!(
            editor_view_for_path(std::path::Path::new("capture.mp4")),
            "recording-editor"
        );
    }
    #[test]
    fn malformed_settings_guard_rejects_in_memory_changes() {
        let mut settings = Settings {
            theme: "cobalt".into(),
            ..Default::default()
        };
        assert!(block_changes_after_load_error(&mut settings, true));
        assert_eq!(settings.theme, Settings::default().theme);
    }

    #[test]
    fn custom_colors_require_exact_rgb_hex_and_normalize_case() {
        assert_eq!(normalize_hex(" ab12ef ").as_deref(), Some("#AB12EF"));
        assert_eq!(normalize_hex("#32D3FF").as_deref(), Some("#32D3FF"));
        assert_eq!(normalize_hex("#12345"), None);
        assert_eq!(normalize_hex("#12zz45"), None);
    }

    #[test]
    fn find_index_covers_every_shipping_preferences_section() {
        let targets = preference_search_targets();
        assert_eq!(targets.len(), 7);
        assert!(targets[1].contains("countdown"));
        assert!(targets[3].contains("microphone"));
        assert!(targets[6].contains("feedback"));
    }
}
