use super::*;
use captures_recording::MaxResolution;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Picker {
    Aspect,
    Display,
    Fps,
    Resolution,
    Microphone,
    Format,
}

impl Selector {
    fn recording_switch(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        available: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = ui::theme(cx);
        div()
            .id(id)
            .flex()
            .flex_col()
            .justify_end()
            .gap(ui::metric("--s-3"))
            .opacity(if available { 1. } else { 0.45 })
            .when(available, |row| row.cursor_pointer())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(ui::metric("--text-xs"))
                    .text_color(p.glass_text().opacity(0.64))
                    .child(label),
            )
            .child(
                div()
                    .h(ui::metric("--h-lg"))
                    .flex()
                    .items_center()
                    .gap(ui::metric("--s-3"))
                    .child(
                        div()
                            .w(px(34.))
                            .h(px(20.))
                            .p(px(3.))
                            .rounded(px(10.))
                            .bg(if enabled && available {
                                p.accent
                            } else {
                                p.glass_text().opacity(0.14)
                            })
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(7.))
                                    .bg(p.glass_text())
                                    .when(enabled && available, |knob| knob.ml(px(14.))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(ui::metric("--text-xs"))
                            .child(if !available {
                                "Unavailable"
                            } else if enabled {
                                "On"
                            } else {
                                "Off"
                            }),
                    ),
            )
    }

    fn choices(&self, picker: Picker) -> (usize, Vec<String>) {
        let s = &self.settings.recording;
        match picker {
            Picker::Aspect => (
                self.aspect_index,
                ASPECTS.iter().map(|(name, _)| name.to_string()).collect(),
            ),
            Picker::Display => (
                self.prepared
                    .displays
                    .iter()
                    .position(|d| d.id == self.prepared.frame.descriptor.id)
                    .unwrap_or(0),
                self.prepared
                    .displays
                    .iter()
                    .map(|d| d.name.clone())
                    .collect(),
            ),
            Picker::Fps => (
                [60, 30, 15]
                    .iter()
                    .position(|fps| *fps == s.video_fps)
                    .unwrap_or(0),
                vec!["60".into(), "30".into(), "15".into()],
            ),
            Picker::Resolution => (
                match s.video_max_resolution {
                    MaxResolution::Original => 0,
                    MaxResolution::P1080 => 1,
                    MaxResolution::P720 => 2,
                },
                vec!["Original".into(), "1080p".into(), "720p".into()],
            ),
            Picker::Microphone => {
                let selected = self
                    .microphones
                    .iter()
                    .position(|d| Some(&d.id) == s.microphone_device_id.as_ref())
                    .map_or(0, |i| i + 1);
                (
                    selected,
                    std::iter::once("Off".to_owned())
                        .chain(self.microphones.iter().map(|d| d.name.clone()))
                        .collect(),
                )
            }
            Picker::Format => (
                usize::from(self.kind == 2),
                vec!["Video".into(), "GIF".into()],
            ),
        }
    }

    pub(super) fn picker(&self, picker: Picker, cx: &mut Context<Self>) -> Div {
        let p = ui::theme(cx);
        let (selected, choices) = self.choices(picker);
        let id = SharedString::from(format!("picker-{picker:?}"));
        let label = choices.get(selected).cloned().unwrap_or_default();
        let mut root = div().relative().child(
            self.glass_button("picker", format!("{label} ▾"), false, cx)
                .id(id.clone())
                .h(ui::metric("--h-lg"))
                .border_1()
                .border_color(p.glass_text().opacity(0.11))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker = (this.picker != Some(picker)).then_some(picker);
                    if picker == Picker::Microphone && !this.microphones_loaded {
                        this.microphones_loaded = true;
                        let task =
                            cx.background_spawn(async { crate::recording::microphone_devices() });
                        cx.spawn(async move |entity, cx| {
                            let devices = task.await;
                            let _ = entity.update(cx, |this, cx| {
                                this.microphones = devices;
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    cx.notify();
                })),
        );
        if self.picker == Some(picker) {
            let mut menu = div()
                .absolute()
                .bottom(ui::metric("--h-xl"))
                .left_0()
                .min_w(px(120.))
                .p(ui::metric("--s-2"))
                .flex()
                .flex_col()
                .rounded(ui::metric("--r-md"))
                .bg(p.color("--glass-raised"))
                .border_1()
                .border_color(p.glass_text().opacity(0.2));
            for (index, label) in choices.into_iter().enumerate() {
                menu = menu.child(
                    self.glass_button("choice", label, index == selected, cx)
                        .id(SharedString::from(format!("{id}-{index}")))
                        .justify_start()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.picker = None;
                            match picker {
                                Picker::Aspect => {
                                    this.aspect_index = index;
                                    if this.rect.width > 0. && this.rect.height > 0. {
                                        let (w, h) = this.dimensions();
                                        this.rect = drag_rect(
                                            Drag::New(this.rect.x, this.rect.y),
                                            this.rect.x + this.rect.width,
                                            this.rect.y + this.rect.height,
                                            w,
                                            h,
                                            ASPECTS[index].1,
                                        );
                                    }
                                }
                                Picker::Display => {
                                    let id = this.prepared.displays[index].id.clone();
                                    window.remove_window();
                                    app::select_display(this.mode, this.kind, id, cx);
                                }
                                Picker::Fps => {
                                    this.settings.recording.video_fps = [60, 30, 15][index]
                                }
                                Picker::Resolution => {
                                    this.settings.recording.video_max_resolution = [
                                        MaxResolution::Original,
                                        MaxResolution::P1080,
                                        MaxResolution::P720,
                                    ][index]
                                }
                                Picker::Microphone => {
                                    this.settings.recording.microphone_device_id =
                                        index.checked_sub(1).map(|i| this.microphones[i].id.clone())
                                }
                                Picker::Format => this.kind = if index == 0 { 1 } else { 2 },
                            }
                            cx.notify();
                        })),
                );
            }
            root = root.child(menu);
        }
        root
    }

    pub(super) fn recording_controls(&self, cx: &mut Context<Self>) -> Div {
        let p = ui::theme(cx);
        let s = &self.settings.recording;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let pointer_controls = captures_recording_xcap::pointer_features_available();
        #[cfg(target_os = "macos")]
        let pointer_controls = true;
        let field = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(ui::metric("--s-3"))
                .child(
                    div()
                        .text_size(ui::metric("--text-xs"))
                        .text_color(p.glass_text().opacity(0.64))
                        .child(label),
                )
                .child(control)
        };
        div()
            .flex()
            .items_end()
            .justify_between()
            .gap(ui::metric("--s-4"))
            .px(ui::metric("--s-6"))
            .pb(ui::metric("--s-5"))
            .child(field(
                "Format",
                self.picker(Picker::Format, cx).into_any_element(),
            ))
            .child(field(
                "FPS",
                self.picker(Picker::Fps, cx).into_any_element(),
            ))
            .child(field(
                "Max resolution",
                self.picker(Picker::Resolution, cx).into_any_element(),
            ))
            .child(
                self.recording_switch(
                    "record-cursor",
                    "Show cursor",
                    s.show_cursor,
                    pointer_controls,
                    cx,
                )
                .when(pointer_controls, |switch| {
                    switch.on_click(cx.listener(|this, _, _, cx| {
                        this.settings.recording.show_cursor = !this.settings.recording.show_cursor;
                        if !this.settings.recording.show_cursor {
                            this.settings.recording.highlight_clicks = false;
                        }
                        cx.notify();
                    }))
                }),
            )
            .child(
                self.recording_switch(
                    "record-clicks",
                    "Show clicks",
                    s.highlight_clicks,
                    pointer_controls,
                    cx,
                )
                .when(pointer_controls, |switch| {
                    switch.on_click(cx.listener(|this, _, _, cx| {
                        this.settings.recording.highlight_clicks =
                            !this.settings.recording.highlight_clicks;
                        if this.settings.recording.highlight_clicks {
                            this.settings.recording.show_cursor = true;
                        }
                        cx.notify();
                    }))
                }),
            )
            .child(
                self.recording_switch(
                    "record-audio",
                    "Desktop audio",
                    s.capture_system_audio,
                    true,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.recording.capture_system_audio =
                        !this.settings.recording.capture_system_audio;
                    cx.notify();
                })),
            )
            .child(field(
                "Microphone",
                self.picker(Picker::Microphone, cx).into_any_element(),
            ))
    }
}
