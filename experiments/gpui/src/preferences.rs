use crate::{
    Launch,
    theme::{Theme, font},
};
use anyhow::Result;
use gpui::{prelude::*, *};
use std::{collections::HashSet, fs, path::PathBuf};

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
    status: String,
    dismissed: HashSet<PathBuf>,
    history_filter: &'static str,
    confirm: Option<PathBuf>,
    message: Entity<TextInput>,
    contact: Entity<TextInput>,
    feedback_busy: bool,
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
            dismissed: HashSet::new(),
            history_filter: "all",
            confirm: None,
            message: cx
                .new(|cx| TextInput::new("", "Tell us what happened or what would help…", cx)),
            contact: cx.new(|cx| TextInput::new("", "Email (optional)", cx)),
            feedback_busy: false,
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
        self.status = match settings::save(&self.launch.profile, &self.settings) {
            Ok(_) => "Changes saved".into(),
            Err(e) => format!("Couldn’t save changes: {e:#}"),
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
            cx.notify();
        }
    }
    fn capture_keys(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let shortcut = if cfg!(target_os = "macos") {
            event.keystroke.modifiers.platform
        } else {
            event.keystroke.modifiers.control
        };
        if shortcut && event.keystroke.key.eq_ignore_ascii_case("f") {
            self.open_find(&OpenFind, window, cx);
        } else if event.keystroke.key == "escape" {
            self.close_find(&CloseFind, window, cx);
        }
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
        cards.push(self.card("Appearance","One look across every Captures window. Capture overlays stay dark so they read on any desktop.",vec![
            self.setting_row("Interface theme","Follow the system setting, or lock Captures to light or dark.",appearance_controls,t).into_any_element(),
            div().flex().flex_col().gap(px(4.)).child(div().font_weight(FontWeight::MEDIUM).child("Accent color")).child(div().max_w(px(347.)).text_size(px(12.)).line_height(px(16.2)).text_color(t.subtle).child("Used for the capture action, selection, and focus. Status colors keep their meaning.")).child(div().pt(px(4.)).child(swatches)).into_any_element(),
        ],t));
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
            self.button("format", format!("Screenshot format: {format}"), false, t)
                .on_click(cx.listener(|s, _, _, cx| {
                    s.settings.screenshot_format = match s.settings.screenshot_format.as_str() {
                        "png" => "jpeg",
                        "jpeg" => "webp",
                        _ => "png",
                    }
                    .into();
                    s.persist(cx)
                }))
                .into_any_element(),
        );
        capture.push(
            self.button(
                "countdown",
                format!(
                    "Screenshot countdown: {} seconds",
                    self.settings.screenshot_countdown_seconds
                ),
                false,
                t,
            )
            .on_click(cx.listener(|s, _, _, cx| {
                s.settings.screenshot_countdown_seconds =
                    match s.settings.screenshot_countdown_seconds {
                        0 => 3,
                        3 => 5,
                        5 => 10,
                        _ => 0,
                    };
                s.persist(cx)
            }))
            .into_any_element(),
        );
        cards.push(self.card(
            "Capture",
            "Where captures go and what happens right after you take one.",
            capture,
            t,
        ));
        let mut shortcuts = vec![];
        for (n, v) in [
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
        ] {
            shortcuts.push(self.row(n, v, t).into_any_element())
        }
        shortcuts.push(div().text_color(t.subtle).text_size(px(12.)).child("Global shortcut registration is not connected in this GPUI experiment; values shown are persisted settings.").into_any_element());
        cards.push(self.card(
            "Shortcuts",
            "Shipping shortcut values. OS registration is read-only here.",
            shortcuts,
            t,
        ));
        let mut recording = vec![
            self.button("video-format", format!("Recording format: {vf}"), false, t)
                .on_click(cx.listener(|s, _, _, cx| {
                    s.settings.recording.video_format =
                        match s.settings.recording.video_format.as_str() {
                            "mp4" => "gif",
                            "gif" => "webm",
                            _ => "mp4",
                        }
                        .into();
                    s.persist(cx)
                }))
                .into_any_element(),
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
                                            .child("Changes save automatically.")
                                            .text_size(px(12.))
                                            .text_color(t.subtle),
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
                            .children(cards)
                            .child(
                                div()
                                    .text_color(if self.settings_load_error.is_some() {
                                        t.signal
                                    } else {
                                        t.muted
                                    })
                                    .child(self.status.clone()),
                            ),
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
            .bg(if self.launch.light {
                rgb(0xefeff2)
            } else {
                rgb(0x0b0b0e)
            })
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
    fn history_files(&self) -> Vec<PathBuf> {
        let root = self.launch.profile.join("captures");
        let mut out = vec![];
        if let Ok(rd) = fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                let extension = p
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let matches = self.history_filter == "all"
                    || (self.history_filter == "screenshots"
                        && matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp"))
                    || (self.history_filter == "recordings"
                        && matches!(extension.as_str(), "mp4" | "webm" | "gif"));
                if p.is_file() && matches && !self.dismissed.contains(&p) {
                    out.push(p)
                }
            }
        }
        out.sort();
        out.reverse();
        out
    }
    fn history(&mut self, cx: &mut Context<Self>, t: Theme) -> Stateful<Div> {
        let files = self.history_files();
        let mut list = div().flex().flex_col().gap_3();
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
            let dismiss = p.clone();
            let del = p.clone();
            let confirmed = self.confirm.as_ref() == Some(&p);
            let metadata = fs::metadata(&p).ok();
            let detail = metadata
                .as_ref()
                .map(|m| format!("{} · {:.1} KB", p.display(), m.len() as f64 / 1024.))
                .unwrap_or_else(|| p.display().to_string());
            let mut actions = div()
                .flex()
                .gap_2()
                .child(
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
                )
                .child(
                    self.button(
                        SharedString::from(format!("dismiss-{name}")),
                        "Dismiss",
                        false,
                        t,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.dismissed.insert(dismiss.clone());
                        cx.notify()
                    })),
                );
            actions = actions.child(
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
                        match fs::remove_file(&del) {
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
                    if is_image_path(&p) {
                        div()
                            .h(px(180.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(8.))
                            .bg(t.canvas)
                            .child(img(p.clone()).size_full().object_fit(ObjectFit::Contain))
                            .into_any_element()
                    } else {
                        div()
                            .h(px(80.))
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
        for filter in ["all", "screenshots", "recordings"] {
            filters = filters.child(
                self.button(
                    SharedString::from(format!("history-filter-{filter}")),
                    match filter {
                        "all" => "All",
                        "screenshots" => "Screenshots",
                        _ => "Recordings",
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
            .child(filters)
            .child(div().text_color(t.muted).child(
                "Dismiss only hides an item for this window. Delete always asks for confirmation.",
            ))
            .child(list)
            .child(self.status.clone())
    }
    fn feedback(&mut self, cx: &mut Context<Self>, t: Theme) -> Div {
        div().flex().size_full().child(self.sidebar(Page::Feedback,cx,t)).child(div().flex_1().p_8().flex().flex_col().gap_4().child("Send feedback").text_size(px(16.)).child(div().text_size(px(13.)).text_color(t.muted).child("Nothing is sent automatically. Press Send feedback to contact captur.es explicitly.")).child(self.message.clone()).child(self.contact.clone()).child(self.button("send",if self.feedback_busy{"Sending feedback…"}else{"Send feedback"},false,t).on_click(cx.listener(|s,_,_,cx|{if s.feedback_busy{return}s.feedback_busy=true;let message=s.message.read(cx).value();let contact=s.contact.read(cx).value();if message.trim().is_empty(){s.feedback_busy=false;s.status="Please enter a short description of the issue or idea.".into();cx.notify();return}let endpoint=std::env::var("CAPTURES_FEEDBACK_URL").unwrap_or_else(|_|captures_feedback::DEFAULT_FEEDBACK_URL.into());let task=cx.background_executor().spawn(async move{captures_feedback::FeedbackClient::new(&endpoint).and_then(|c|c.submit(captures_feedback::FeedbackDraft{message,contact:Some(contact),category:"other".into()},captures_feedback::FeedbackContext{app_version:env!("CARGO_PKG_VERSION").into(),os:std::env::consts::OS.into(),os_version:"unknown".into(),arch:std::env::consts::ARCH.into()}))});cx.spawn(async move|this,cx|{let result=task.await;let _=this.update(cx,|s,cx|{s.feedback_busy=false;s.status=match result{Ok(_)=>"Feedback sent. Thank you.".into(),Err(e)=>e};cx.notify()});}).detach();s.status="Sending feedback…".into();cx.notify()}))).child(div().text_color(if self.status.starts_with("Feedback sent"){t.positive}else{t.muted}).child(self.status.clone())))
    }
    fn onboarding(&mut self, cx: &mut Context<Self>, t: Theme) -> Div {
        div().size_full().p_10().flex().flex_col().justify_center().items_center().gap_5().bg(t.canvas).child(div().text_size(px(30.)).font_weight(FontWeight::BOLD).child("Capture what matters")).child(div().max_w(px(520.)).text_color(t.muted).child("Captures keeps screenshots and recordings close at hand. Choose a target, capture it, then refine it in the editor.")).child(self.card("Before your first capture","Screen-recording and microphone permissions are managed by your operating system. This experiment can’t request or verify them yet; no permission is shown as granted here.",vec![],t)).child(self.button("finish","Continue to Preferences",true,t).on_click(cx.listener(|s,_,_,cx|{s.settings.onboarding_completed=true;s.persist(cx);s.nav(Page::Preferences,cx)})))
    }
}
impl Render for Surface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::configured(
            self.launch.light,
            &self.settings.theme,
            &self.settings.custom_theme.accent,
            &self.settings.custom_theme.signal,
        );
        div()
            .key_context("Preferences")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::capture_keys))
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
}
