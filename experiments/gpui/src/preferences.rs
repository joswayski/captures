//! Custom GPUI Preferences window for the Linux rewrite.
mod input;

use crate::{
    history,
    settings::Settings,
    ui::{Theme, button, metric, root, theme},
};
use captures_recording::{AudioDevice, MaxResolution};
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, IntoElement,
    KeyDownEvent, Render, ScrollHandle, SharedString, Stateful, TitlebarOptions, UpdateGlobal,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use input::{InputEvent, TextInput};
use std::{borrow::BorrowMut, path::PathBuf};

const SECTIONS: [(&str, &str); 7] = [
    ("appearance", "Appearance"),
    ("capture", "Capture"),
    ("shortcuts", "Shortcuts"),
    ("recording", "Recording"),
    ("gif", "GIF export"),
    ("updates", "Updates"),
    ("about", "About"),
];

const THEMES: [(&str, &str, &str); 10] = [
    ("mustard", "Mustard", "#ffca28"),
    ("ember", "Ember", "#ff7a45"),
    ("rose", "Rose", "#ff5ba7"),
    ("violet", "Violet", "#c026d3"),
    ("cobalt", "Cobalt", "#2563eb"),
    ("aqua", "Aqua", "#31cbd8"),
    ("mint", "Mint", "#67d5a5"),
    ("lime", "Lime", "#b6db45"),
    ("mono", "Mono", "#ededed"),
    ("custom", "Custom", "#32d3ff"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shortcut {
    NewCapture,
    Region,
    Window,
    Display,
    RecordRegion,
    RecordWindow,
    RecordDisplay,
    RecordGif,
}

impl Shortcut {
    fn label(self) -> &'static str {
        match self {
            Self::NewCapture => "New Capture",
            Self::Region => "Region",
            Self::Window => "Window",
            Self::Display => "Full Screen",
            Self::RecordRegion => "Record Region",
            Self::RecordWindow => "Record Window",
            Self::RecordDisplay => "Record Full Screen",
            Self::RecordGif => "Record GIF",
        }
    }

    fn value(self, settings: &Settings) -> &str {
        match self {
            Self::NewCapture => &settings.new_capture_shortcut,
            Self::Region => &settings.region_shortcut,
            Self::Window => &settings.window_shortcut,
            Self::Display => &settings.display_shortcut,
            Self::RecordRegion => &settings.recording.video_shortcut,
            Self::RecordWindow => &settings.recording.window_shortcut,
            Self::RecordDisplay => &settings.recording.display_shortcut,
            Self::RecordGif => &settings.recording.gif_shortcut,
        }
    }

    fn set(self, settings: &mut Settings, value: String) {
        match self {
            Self::NewCapture => settings.new_capture_shortcut = value,
            Self::Region => settings.region_shortcut = value,
            Self::Window => settings.window_shortcut = value,
            Self::Display => settings.display_shortcut = value,
            Self::RecordRegion => settings.recording.video_shortcut = value,
            Self::RecordWindow => settings.recording.window_shortcut = value,
            Self::RecordDisplay => settings.recording.display_shortcut = value,
            Self::RecordGif => settings.recording.gif_shortcut = value,
        }
    }
}

struct Preferences {
    settings: Settings,
    section: &'static str,
    status: Option<(bool, String)>,
    find_open: bool,
    search_query: String,
    search: Entity<TextInput>,
    output: Entity<TextInput>,
    accent: Entity<TextInput>,
    signal: Entity<TextInput>,
    recording_shortcut: Option<Shortcut>,
    focus_handle: FocusHandle,
    microphones: Vec<AudioDevice>,
    scroll_handle: ScrollHandle,
}

impl Preferences {
    fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroll_handle = ScrollHandle::new();
        let search = cx.new(|cx| TextInput::new("", "Find a preference…", cx));
        cx.subscribe_in(&search, window, |this, _, event, _, cx| {
            if let InputEvent::Changed(query) = event {
                this.search_query = query.clone();
                if let Some(section) = section_for_query(query) {
                    this.section = section;
                    let index = SECTIONS
                        .iter()
                        .position(|(id, _)| *id == section)
                        .unwrap_or_default();
                    this.scroll_handle.scroll_to_top_of_item(index + 1);
                }
                cx.notify();
            }
        })
        .detach();

        let output = cx.new(|cx| {
            TextInput::new(
                settings.output_directory.to_string_lossy().into_owned(),
                "/home/you/Pictures/Captures",
                cx,
            )
        });
        cx.subscribe(&output, |this, _, event, cx| {
            if let InputEvent::Submitted(value) = event {
                let path = PathBuf::from(value);
                this.change(|settings| settings.output_directory = path, cx);
            }
        })
        .detach();

        let accent = cx.new(|cx| TextInput::new(settings.custom_accent.clone(), "#32d3ff", cx));
        cx.subscribe(&accent, |this, _, event, cx| {
            if let InputEvent::Submitted(value) = event {
                this.change(
                    |settings| {
                        settings.theme = "custom".into();
                        settings.custom_accent = value.clone();
                    },
                    cx,
                );
            }
        })
        .detach();
        let signal = cx.new(|cx| TextInput::new(settings.custom_signal.clone(), "#ff4fc3", cx));
        cx.subscribe(&signal, |this, _, event, cx| {
            if let InputEvent::Submitted(value) = event {
                this.change(
                    |settings| {
                        settings.theme = "custom".into();
                        settings.custom_signal = value.clone();
                    },
                    cx,
                );
            }
        })
        .detach();

        Self {
            settings,
            section: "appearance",
            status: None,
            find_open: false,
            search_query: String::new(),
            search,
            output,
            accent,
            signal,
            recording_shortcut: None,
            focus_handle: cx.focus_handle(),
            microphones: Vec::new(),
            scroll_handle,
        }
    }

    fn change(&mut self, update: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut next = self.settings.clone();
        update(&mut next);
        match next.save() {
            Ok(()) => {
                self.settings = next.clone();
                Settings::set_global(cx, next);
                crate::app::settings_changed(cx.borrow_mut());
                self.status = Some((true, "Changes saved".into()));
            }
            Err(error) => self.status = Some((false, error)),
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_shortcut.is_none() {
            if event.keystroke.modifiers.control && event.keystroke.key.eq_ignore_ascii_case("f") {
                self.find_open = true;
                window.focus(&self.search.read(cx).handle());
                cx.stop_propagation();
                cx.notify();
            } else if self.find_open && event.keystroke.key.eq_ignore_ascii_case("escape") {
                self.find_open = false;
                self.search.update(cx, TextInput::clear);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let Some(field) = self.recording_shortcut else {
            return;
        };
        cx.stop_propagation();
        if event.keystroke.key.eq_ignore_ascii_case("escape") {
            self.recording_shortcut = None;
            self.status = None;
            cx.notify();
            return;
        }
        if [
            "control", "ctrl", "shift", "alt", "super", "meta", "cmd", "fn",
        ]
        .contains(&event.keystroke.key.to_ascii_lowercase().as_str())
        {
            return;
        }
        let Some(shortcut) = shortcut_from_keystroke(event) else {
            self.status = Some((
                false,
                "Use a modifier with the key, or press Print Screen.".into(),
            ));
            cx.notify();
            return;
        };
        self.change(move |settings| field.set(settings, shortcut), cx);
        if self.status.as_ref().is_some_and(|(saved, _)| *saved) {
            self.recording_shortcut = None;
        }
    }

    fn nav_button(
        &self,
        index: usize,
        id: &'static str,
        label: &'static str,
        colors: Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let selected = self.section == id;
        let scroll_handle = self.scroll_handle.clone();
        div()
            .id(SharedString::from(format!("preferences-nav-{id}")))
            .w_full()
            .px(metric("--s-5"))
            .py(metric("--s-4"))
            .rounded(metric("--r-sm"))
            .cursor_pointer()
            .text_color(if selected {
                colors.text()
            } else {
                colors.muted()
            })
            .when(selected, |style| style.bg(colors.color("--surface-active")))
            .hover(move |style| style.bg(colors.color("--surface-hover")))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = id;
                scroll_handle.scroll_to_top_of_item(index + 1);
                cx.notify();
            }))
    }

    fn choice(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        colors: Theme,
    ) -> Stateful<gpui::Div> {
        div()
            .id(id.into())
            .flex()
            .items_center()
            .justify_center()
            .h(metric("--h-md"))
            .px(metric("--s-5"))
            .rounded(metric("--r-sm"))
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                colors.border()
            })
            .bg(if selected {
                Hsla {
                    a: if colors.dark { 0.14 } else { 0.13 },
                    ..colors.accent
                }
            } else {
                colors.canvas()
            })
            .text_color(colors.text())
            .cursor_pointer()
            .hover(move |style| style.bg(colors.color("--surface-hover")))
            .child(label.into())
    }

    fn switch_row(
        &self,
        id: impl Into<gpui::ElementId>,
        title: &'static str,
        description: &'static str,
        enabled: bool,
        colors: Theme,
    ) -> Stateful<gpui::Div> {
        div()
            .id(id)
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(metric("--s-6"))
            .py(metric("--s-5"))
            .border_b_1()
            .border_color(colors.border())
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(metric("--s-2"))
                    .child(title)
                    .child(
                        div()
                            .text_size(metric("--text-sm"))
                            .text_color(colors.muted())
                            .child(description),
                    ),
            )
            .child(
                div()
                    .w(px(42.))
                    .h(px(24.))
                    .p(px(3.))
                    .rounded(px(12.))
                    .bg(if enabled {
                        colors.accent
                    } else {
                        colors.color("--surface-sunken")
                    })
                    .child(
                        div()
                            .size(px(18.))
                            .rounded(px(9.))
                            .bg(if enabled && colors.dark {
                                colors.canvas()
                            } else {
                                colors.raised()
                            })
                            .when(enabled, |style| style.ml(px(18.))),
                    ),
            )
    }

    fn card(
        &self,
        title: &'static str,
        description: &'static str,
        colors: Theme,
        body: impl IntoElement,
    ) -> gpui::Div {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(metric("--s-5"))
            .p(metric("--s-6"))
            .rounded(metric("--r-lg"))
            .border_1()
            .border_color(colors.border())
            .bg(colors.raised())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(metric("--s-2"))
                    .child(
                        div()
                            .text_size(metric("--text-lg"))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(metric("--text-sm"))
                            .text_color(colors.muted())
                            .child(description),
                    ),
            )
            .child(body)
    }

    fn appearance(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let modes = [("system", "System"), ("light", "Light"), ("dark", "Dark")];
        let mode_row = div()
            .flex()
            .gap(metric("--s-3"))
            .children(modes.into_iter().map(|(id, label)| {
                self.choice(
                    format!("appearance-{id}"),
                    label,
                    self.settings.appearance == id,
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(|settings| settings.appearance = id.into(), cx)
                }))
            }));
        let themes =
            div()
                .grid()
                .grid_cols(5)
                .gap(metric("--s-3"))
                .children(THEMES.into_iter().map(|(id, label, hex)| {
                    let swatch = parse_hex(if id == "custom" {
                        &self.settings.custom_accent
                    } else {
                        hex
                    });
                    self.choice(
                        format!("theme-{id}"),
                        label,
                        self.settings.theme == id,
                        colors,
                    )
                    .justify_start()
                    .gap(metric("--s-3"))
                    .child(div().size(px(16.)).rounded(px(8.)).bg(swatch))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(|settings| settings.theme = id.into(), cx)
                    }))
                }));
        let custom = div().when(self.settings.theme == "custom", |container| {
            container
                .mt(metric("--s-6"))
                .grid()
                .grid_cols(2)
                .gap(metric("--s-5"))
                .child(self.field("Accent — press Enter to save", self.accent.clone(), colors))
                .child(self.field(
                    "Recording signal — press Enter to save",
                    self.signal.clone(),
                    colors,
                ))
        });
        let mode = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(metric("--s-6"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(metric("--s-2"))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Interface theme"),
                    )
                    .child(
                        div()
                            .text_size(metric("--text-sm"))
                            .text_color(colors.muted())
                            .child("Follow the system, or lock Captures to light or dark."),
                    ),
            )
            .child(mode_row);
        self.card("Appearance", "One look across every Captures window. Capture overlays stay dark so they read on any desktop.", colors,
            div().flex().flex_col().gap(metric("--s-6")).child(mode)
                .child(self.labeled("Accent color", "Used for capture, selection, and focus. Status colors retain their meaning.", themes, colors)).child(custom)).into_any_element()
    }

    fn capture(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = &self.settings;
        let format = div().flex().gap(metric("--s-3")).children(
            [("png", "PNG"), ("jpeg", "JPEG"), ("webp", "WebP")]
                .into_iter()
                .map(|(id, label)| {
                    self.choice(
                        format!("shot-format-{id}"),
                        label,
                        settings.screenshot_format == id,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(|s| s.screenshot_format = id.into(), cx)
                    }))
                }),
        );
        let countdown = div()
            .flex()
            .flex_wrap()
            .gap(metric("--s-3"))
            .children((0u8..=10).map(|value| {
                self.choice(
                    format!("shot-delay-{value}"),
                    if value == 0 {
                        SharedString::from("Off")
                    } else {
                        SharedString::from(format!("{value}s"))
                    },
                    settings.screenshot_countdown_seconds == value,
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.screenshot_countdown_seconds = value, cx)
                }))
            }));
        let corners = ["Bottom left", "Bottom right", "Top left", "Top right"];
        let corner =
            div()
                .flex()
                .gap(metric("--s-3"))
                .children(corners.into_iter().enumerate().map(|(value, label)| {
                    self.choice(
                        format!("preview-corner-{value}"),
                        label,
                        settings.mini_preview_placement == value as u32,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(|s| s.mini_preview_placement = value as u32, cx)
                    }))
                }));
        let toggles = div().flex().flex_col()
            .child(self.switch_row("auto-copy", "Automatically copy captures to the clipboard", "Turn this off to preserve existing clipboard contents.", settings.auto_copy_to_clipboard, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.auto_copy_to_clipboard = !s.auto_copy_to_clipboard, cx))))
            .child(self.switch_row("auto-start", "Start capture as soon as a target is selected", "Otherwise press Enter in the capture menu to confirm.", settings.auto_start_on_selection, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.auto_start_on_selection = !s.auto_start_on_selection, cx))))
            .child(self.switch_row("mini-previews", "Show mini previews after screenshots", "Keeps a quick-access preview stack on your desktop.", settings.show_mini_previews, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.show_mini_previews = !s.show_mini_previews, cx))))
            .child(self.switch_row("include-previews", "Show mini previews in captures", "Off keeps Captures previews out of screenshots and recordings.", settings.include_mini_previews_in_captures, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.include_mini_previews_in_captures = !s.include_mini_previews_in_captures, cx))))
            .child(self.switch_row("include-controls", "Show recording controls in captures", "Linux sessions that cannot exclude controls should use Hide controls on the recording bar.", settings.include_recording_controls_in_captures, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.include_recording_controls_in_captures = !s.include_recording_controls_in_captures, cx))))
            .child(self.switch_row("freeze-screen", "Freeze screen when capturing", "Holds menus, hover states, and motion still while selecting.", settings.freeze_screen, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.freeze_screen = !s.freeze_screen, cx))))
            .child(self.switch_row("shot-cursor", "Show cursor in screenshots", "Includes the pointer in still captures.", settings.show_cursor_in_screenshots, colors).on_click(cx.listener(|this, _, _, cx| this.change(|s| s.show_cursor_in_screenshots = !s.show_cursor_in_screenshots, cx))));
        self.card(
            "Capture",
            "Where captures go and what happens right after you take one.",
            colors,
            div()
                .flex()
                .flex_col()
                .gap(metric("--s-6"))
                .child(self.field(
                    "Save captures to — press Enter to save",
                    self.output.clone(),
                    colors,
                ))
                .child(toggles)
                .child(self.labeled(
                    "Mini preview position",
                    "Choose the screen corner used for previews.",
                    corner,
                    colors,
                ))
                .child(self.labeled(
                    "Screenshot format",
                    "Used when saving or exporting; history remains lossless.",
                    format,
                    colors,
                ))
                .child(self.labeled(
                    "Screenshot countdown",
                    "Wait before capturing; Esc cancels.",
                    countdown,
                    colors,
                )),
        )
        .into_any_element()
    }

    fn shortcuts(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let fields = [
            Shortcut::NewCapture,
            Shortcut::Region,
            Shortcut::Window,
            Shortcut::Display,
            Shortcut::RecordRegion,
            Shortcut::RecordWindow,
            Shortcut::RecordDisplay,
            Shortcut::RecordGif,
        ];
        self.card("Shortcuts", "Select a shortcut, press the combination you want, or press Esc to cancel. Desktop shortcut conflicts must be resolved in your Linux settings.", colors,
            div().flex().flex_col().children(fields.into_iter().map(|field| {
                let recording = self.recording_shortcut == Some(field);
                div().flex().items_center().justify_between().py(metric("--s-4")).border_b_1().border_color(colors.border())
                    .child(field.label()).child(self.choice(format!("shortcut-{field:?}"), if recording { SharedString::from("Press shortcut…") } else { SharedString::from(field.value(&self.settings).to_owned()) }, recording, colors)
                        .on_click(cx.listener(move |this, _, window, cx| { this.recording_shortcut = Some(field); window.focus(&this.focus_handle); cx.notify(); })))
            }))).into_any_element()
    }

    fn recording(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let r = &self.settings.recording;
        let formats = div().flex().gap(metric("--s-3")).children(
            [("mp4", "MP4"), ("gif", "GIF"), ("webm", "WebM")]
                .into_iter()
                .map(|(id, label)| {
                    self.choice(
                        format!("video-format-{id}"),
                        label,
                        r.video_format == id,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(|s| s.recording.video_format = id.into(), cx)
                    }))
                }),
        );
        let fps = div()
            .flex()
            .gap(metric("--s-3"))
            .children([60u16, 30, 15].into_iter().map(|value| {
                self.choice(
                    format!("video-fps-{value}"),
                    format!("{value} FPS"),
                    r.video_fps == value,
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.recording.video_fps = value, cx)
                }))
            }));
        let resolutions = [
            (MaxResolution::Original, "Original"),
            (MaxResolution::P1080, "1080p"),
            (MaxResolution::P720, "720p"),
        ];
        let resolution = div()
            .flex()
            .gap(metric("--s-3"))
            .children(resolutions.into_iter().map(|(value, label)| {
                self.choice(
                    format!("resolution-{label}"),
                    label,
                    r.video_max_resolution == value,
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.recording.video_max_resolution = value, cx)
                }))
            }));
        let delay = div()
            .flex()
            .flex_wrap()
            .gap(metric("--s-3"))
            .children((0u8..=10).map(|value| {
                self.choice(
                    format!("record-delay-{value}"),
                    if value == 0 {
                        SharedString::from("Off")
                    } else {
                        SharedString::from(format!("{value}s"))
                    },
                    r.countdown_seconds == value,
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.recording.countdown_seconds = value, cx)
                }))
            }));
        let mic_options = std::iter::once((None, "Off".to_owned())).chain(
            self.microphones
                .iter()
                .map(|device| (Some(device.id.clone()), device.name.clone())),
        );
        let microphones =
            div()
                .flex()
                .flex_wrap()
                .gap(metric("--s-3"))
                .children(mic_options.enumerate().map(|(index, (value, label))| {
                    self.choice(
                        format!("microphone-{index}"),
                        label,
                        r.microphone_device_id == value,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let value = value.clone();
                        this.change(|s| s.recording.microphone_device_id = value, cx)
                    }))
                }));
        let toggles = div()
            .flex()
            .flex_col()
            .child(
                self.switch_row(
                    "desktop-audio",
                    "Record desktop audio",
                    "Records sound playing through the system output.",
                    r.capture_system_audio,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(
                        |s| s.recording.capture_system_audio = !s.recording.capture_system_audio,
                        cx,
                    )
                })),
            )
            .child(
                self.switch_row(
                    "mono",
                    "Export recording audio in mono",
                    "Combines recorded channels during export.",
                    r.mono_audio,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(|s| s.recording.mono_audio = !s.recording.mono_audio, cx)
                })),
            )
            .child(
                self.switch_row(
                    "record-cursor",
                    "Show cursor in recordings",
                    "Includes the pointer when the backend supports cursor control.",
                    r.show_cursor,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(|s| s.recording.show_cursor = !s.recording.show_cursor, cx)
                })),
            )
            .child(
                self.switch_row(
                    "clicks",
                    "Show clicks in recordings",
                    "Highlights pointer clicks when supported by the capture backend.",
                    r.highlight_clicks,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(
                        |s| s.recording.highlight_clicks = !s.recording.highlight_clicks,
                        cx,
                    )
                })),
            )
            .child(
                self.switch_row(
                    "keys",
                    "Show keystrokes in recordings",
                    "Displays supported keyboard input in the recording.",
                    r.show_keystrokes,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(
                        |s| s.recording.show_keystrokes = !s.recording.show_keystrokes,
                        cx,
                    )
                })),
            )
            .child(
                self.switch_row(
                    "open-editor",
                    "Open the editor after recording",
                    "History keeps an independent recovery copy for 30 days.",
                    r.open_editor_after_recording,
                    colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change(
                        |s| {
                            s.recording.open_editor_after_recording =
                                !s.recording.open_editor_after_recording
                        },
                        cx,
                    )
                })),
            );
        self.card(
            "Recording",
            "Defaults for new screen recordings. You can still change them in the capture menu.",
            colors,
            div()
                .flex()
                .flex_col()
                .gap(metric("--s-6"))
                .child(self.labeled(
                    "Recording format",
                    "H.264 MP4 is captured first; GIF and WebM are converted during export.",
                    formats,
                    colors,
                ))
                .child(self.labeled(
                    "Frames per second",
                    "Choose a supported recording frame rate.",
                    fps,
                    colors,
                ))
                .child(self.labeled(
                    "Maximum resolution",
                    "Limit recording output size.",
                    resolution,
                    colors,
                ))
                .child(self.labeled("Countdown", "Delay before recording starts.", delay, colors))
                .child(self.labeled(
                    "Default microphone",
                    "Used when a recording starts with microphone audio.",
                    microphones,
                    colors,
                ))
                .child(toggles),
        )
        .into_any_element()
    }

    fn gif(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let r = &self.settings.recording;
        let fps = [8u16, 10, 12, 15, 20, 24, 30];
        let widths = [320u32, 480, 640, 800, 1200];
        let palettes = [64u16, 96, 128, 256];
        self.card(
            "GIF export",
            "Starting point when a recording is exported as an animated GIF.",
            colors,
            div()
                .flex()
                .flex_col()
                .gap(metric("--s-6"))
                .child(
                    self.labeled(
                        "Frames per second",
                        "Animation smoothness and output size.",
                        div().flex().flex_wrap().gap(metric("--s-3")).children(
                            fps.into_iter().map(|value| {
                                self.choice(
                                    format!("gif-fps-{value}"),
                                    format!("{value} FPS"),
                                    r.gif_fps == value,
                                    colors,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change(|s| s.recording.gif_fps = value, cx)
                                    },
                                ))
                            }),
                        ),
                        colors,
                    ),
                )
                .child(
                    self.labeled(
                        "Maximum width",
                        "Scale the GIF while preserving its aspect ratio.",
                        div().flex().flex_wrap().gap(metric("--s-3")).children(
                            widths.into_iter().map(|value| {
                                self.choice(
                                    format!("gif-width-{value}"),
                                    format!("{value} px"),
                                    r.gif_max_width == value,
                                    colors,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change(|s| s.recording.gif_max_width = value, cx)
                                    },
                                ))
                            }),
                        ),
                        colors,
                    ),
                )
                .child(
                    self.labeled(
                        "Palette colors",
                        "More colors improve fidelity and increase size.",
                        div()
                            .flex()
                            .gap(metric("--s-3"))
                            .children(palettes.into_iter().map(|value| {
                                self.choice(
                                    format!("gif-colors-{value}"),
                                    value.to_string(),
                                    r.gif_max_colors == value,
                                    colors,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change(|s| s.recording.gif_max_colors = value, cx)
                                    },
                                ))
                            })),
                        colors,
                    ),
                ),
        )
        .into_any_element()
    }

    fn updates(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.card("Updates", "Update behavior for this isolated GPUI distribution.", colors,
            div().flex().flex_col().gap(metric("--s-6"))
                .child(div().p(metric("--s-5")).rounded(metric("--r-sm")).bg(colors.color("--surface-sunken")).child("This build has no automatic update channel yet. Install updates using the same source used to install Captures."))
                .child(self.switch_row("show-changelog", "Show what’s new on update notices", "Saved for a future distribution channel; no update check is performed today.", self.settings.show_update_changelog, colors)
                    .on_click(cx.listener(|this,_,_,cx| this.change(|s| s.show_update_changelog = !s.show_update_changelog,cx))))).into_any_element()
    }

    fn about(&self, colors: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.card("About", "Captures is in active development. This GPUI build is an isolated rewrite.", colors,
            div().flex().flex_col().gap(metric("--s-6"))
                .child(div().p(metric("--s-5")).rounded(metric("--r-sm")).bg(colors.color("--surface-sunken")).child("Feedback submission is not connected in this distribution. No report has been sent."))
                .child(self.switch_row("launch-login", "Launch Captures when I sign in", "The preference is persisted; parent app integration applies startup registration.", self.settings.launch_at_login, colors)
                    .on_click(cx.listener(|this,_,_,cx| this.change(|s| s.launch_at_login = !s.launch_at_login,cx))))).into_any_element()
    }

    fn labeled(
        &self,
        title: &'static str,
        description: &'static str,
        control: impl IntoElement,
        colors: Theme,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(metric("--s-3"))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .text_size(metric("--text-sm"))
                    .text_color(colors.muted())
                    .child(description),
            )
            .child(control)
    }
    fn field(&self, label: &'static str, input: Entity<TextInput>, colors: Theme) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(metric("--s-3"))
            .child(
                div()
                    .text_size(metric("--text-sm"))
                    .text_color(colors.muted())
                    .child(label),
            )
            .child(input)
    }
}

impl Focusable for Preferences {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Preferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        let sections = [
            self.appearance(colors, cx),
            self.capture(colors, cx),
            self.shortcuts(colors, cx),
            self.recording(colors, cx),
            self.gif(colors, cx),
            self.updates(colors, cx),
            self.about(colors, cx),
        ];
        root(colors)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex_row()
            .child(
                div()
                    .w(px(196.))
                    .h_full()
                    .px(metric("--s-5"))
                    .py(metric("--s-6"))
                    .border_r_1()
                    .border_color(colors.border())
                    .bg(colors.color("--surface-sunken"))
                    .flex()
                    .flex_col()
                    .gap(metric("--s-3"))
                    .child(
                        div()
                            .pb(metric("--s-6"))
                            .flex()
                            .items_center()
                            .gap(metric("--s-4"))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(metric("--text-lg"))
                            .child(
                                div()
                                    .size(px(26.))
                                    .rounded(metric("--r-md"))
                                    .bg(colors.accent)
                                    .text_color(colors.canvas())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("⌾"),
                            )
                            .child("Captures"),
                    )
                    .children(
                        SECTIONS
                            .into_iter()
                            .enumerate()
                            .map(|(index, (id, label))| {
                                self.nav_button(index, id, label, colors, cx)
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(metric("--s-6"))
                            .py(metric("--s-5"))
                            .border_b_1()
                            .border_color(colors.border())
                            .bg(colors.raised())
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(metric("--s-6"))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(24.))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child("Preferences"),
                                    )
                                    .child(
                                        div()
                                            .text_size(metric("--text-sm"))
                                            .text_color(colors.muted())
                                            .child("Changes save automatically."),
                                    ),
                            )
                            .when(self.find_open, |header| {
                                header.child(div().w(px(260.)).child(self.search.clone()))
                            })
                            .child(button("open-history", "Capture History…", colors).on_click(
                                |_, _, cx| {
                                    if let Err(error) = history::open(cx) {
                                        eprintln!("Could not open history: {error}");
                                    }
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("preferences-scroll")
                            .flex_1()
                            .overflow_x_hidden()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .p(metric("--s-6"))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(720.))
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap(metric("--s-4"))
                                    .when(!self.search_query.is_empty(), |container| {
                                        container.child(
                                            div()
                                                .text_size(metric("--text-sm"))
                                                .text_color(colors.muted())
                                                .child(format!(
                                                    "Best match for “{}”",
                                                    self.search_query
                                                )),
                                        )
                                    })
                                    .when_some(
                                        self.status.clone(),
                                        |container, (saved, message)| {
                                            container.child(
                                                div()
                                                    .p(metric("--s-4"))
                                                    .rounded(metric("--r-sm"))
                                                    .border_1()
                                                    .border_color(if saved {
                                                        colors.accent
                                                    } else {
                                                        parse_hex("#ef4650")
                                                    })
                                                    .child(message),
                                            )
                                        },
                                    ),
                            )
                            .children(sections.into_iter().enumerate().map(|(index, section)| {
                                div()
                                    .id(SharedString::from(format!("preferences-section-{index}")))
                                    .w_full()
                                    .max_w(px(720.))
                                    .mx_auto()
                                    .mb(metric("--s-4"))
                                    .child(section)
                            })),
                    ),
            )
    }
}

fn section_for_query(query: &str) -> Option<&'static str> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return None;
    }
    [
        ("appearance", "appearance interface theme accent color light dark custom signal"),
        ("capture", "capture save folder directory clipboard preview position countdown freeze cursor screenshot format"),
        ("shortcuts", "shortcut hotkey keyboard region window full screen record"),
        ("recording", "recording video format fps resolution countdown microphone audio mono cursor clicks keystrokes editor"),
        ("gif", "gif animation fps width palette colors export"),
        ("updates", "updates update changelog release preview"),
        ("about", "about feedback launch login startup sign in"),
    ].into_iter().find_map(|(section, words)| words.contains(&query).then_some(section))
}

fn shortcut_from_keystroke(event: &KeyDownEvent) -> Option<String> {
    let key = match event.keystroke.key.to_ascii_lowercase().as_str() {
        "print" | "printscreen" => "PrintScreen".to_owned(),
        "return" | "enter" => "Enter".to_owned(),
        "space" => "Space".to_owned(),
        key if key.len() == 1 => key.to_ascii_uppercase(),
        key => {
            let mut chars = key.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())?
        }
    };
    let modifiers = event.keystroke.modifiers;
    if !modifiers.control
        && !modifiers.shift
        && !modifiers.alt
        && !modifiers.platform
        && key != "PrintScreen"
    {
        return None;
    }
    let mut parts = Vec::new();
    if modifiers.control {
        parts.push("Control".to_owned());
    }
    if modifiers.shift {
        parts.push("Shift".to_owned());
    }
    if modifiers.alt {
        parts.push("Alt".to_owned());
    }
    if modifiers.platform {
        parts.push("Super".to_owned());
    }
    parts.push(key);
    let shortcut = parts.join("+");
    shortcut
        .parse::<global_hotkey::hotkey::HotKey>()
        .ok()
        .map(|_| shortcut)
}

fn parse_hex(value: &str) -> Hsla {
    let rgb = u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or(0x32d3ff);
    gpui::rgba((rgb << 8) | 0xff).into()
}

pub fn open(cx: &mut App) -> anyhow::Result<()> {
    input::bind_keys(cx);
    if cx.try_global::<Settings>().is_none() {
        cx.set_global(Settings::load().map_err(anyhow::Error::msg)?);
    }
    let settings = cx.global::<Settings>().clone();
    let devices = cx.background_spawn(async { captures_recording_xcap::microphone_devices() });
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(980.), px(720.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Captures GPUI Preferences".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(760.), px(560.))),
            ..Default::default()
        },
        |window, cx| {
            let preferences = cx.new(|cx| Preferences::new(settings, window, cx));
            window.focus(&preferences.read(cx).focus_handle);
            preferences
        },
    )?;
    cx.spawn(async move |cx| {
        let microphones = devices.await;
        let _ = window.update(cx, |preferences, _, cx| {
            preferences.microphones = microphones;
            cx.notify();
        });
    })
    .detach();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    #[test]
    fn search_routes_specific_preferences_to_their_section() {
        assert_eq!(section_for_query("microphone"), Some("recording"));
        assert_eq!(section_for_query("palette"), Some("gif"));
        assert_eq!(section_for_query("save folder"), Some("capture"));
        assert_eq!(section_for_query("does not exist"), None);
    }

    #[test]
    fn shortcut_capture_requires_modifiers_except_print_screen() {
        let plain = KeyDownEvent {
            keystroke: Keystroke {
                key: "k".into(),
                ..Default::default()
            },
            is_held: false,
        };
        assert_eq!(shortcut_from_keystroke(&plain), None);
        let modified = KeyDownEvent {
            keystroke: Keystroke {
                key: "k".into(),
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                key_char: Some("K".into()),
            },
            is_held: false,
        };
        assert_eq!(
            shortcut_from_keystroke(&modified),
            Some("Control+Shift+K".into())
        );
        let print = KeyDownEvent {
            keystroke: Keystroke {
                key: "print".into(),
                ..Default::default()
            },
            is_held: false,
        };
        assert_eq!(shortcut_from_keystroke(&print), Some("PrintScreen".into()));
    }
}
