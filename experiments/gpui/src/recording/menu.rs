//! Capture menu dimensions and controls follow capture.css and primitives.css.
use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MenuPicker {
    Aspect,
    Display,
    Fps,
    Resolution,
    Microphone,
}

/// Retarget from the currently displayed value, including rapid reversals.
pub(super) struct Motion {
    from: f32,
    to: f32,
    started: Instant,
}

impl Motion {
    fn value(&self, now: Instant, duration: f32) -> f32 {
        self.from
            + (self.to - self.from)
                * standard_ease((now.duration_since(self.started).as_secs_f32() / duration).min(1.))
    }
    fn retarget(&mut self, to: f32, now: Instant, duration: f32) {
        if self.to != to {
            self.from = self.value(now, duration);
            self.to = to;
            self.started = now;
        }
    }
}

// shared/design.css --ease-standard: cubic-bezier(.2,.8,.2,1).
fn standard_ease(progress: f32) -> f32 {
    cubic_ease(progress, 0.2, 0.8, 0.2, 1.)
}

fn cubic_ease(progress: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if progress <= 0. {
        return 0.;
    }
    if progress >= 1. {
        return 1.;
    }
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..20 {
        let t = (lo + hi) * 0.5;
        let x = 3. * x1 * (1. - t) * (1. - t) * t + 3. * x2 * (1. - t) * t * t + t * t * t;
        if x < progress {
            lo = t;
        } else {
            hi = t;
        }
    }
    let t = (lo + hi) * 0.5;
    3. * y1 * (1. - t) * (1. - t) * t + 3. * y2 * (1. - t) * t * t + t * t * t
}

const REGION_ICON: &str = r#"<path d="M5 9V6a1 1 0 0 1 1-1h3M15 5h3a1 1 0 0 1 1 1v3M19 15v3a1 1 0 0 1-1 1h-3M9 19H6a1 1 0 0 1-1-1v-3"/><rect x="9" y="9" width="6" height="6" rx="1"/>"#;
const WINDOW_ICON: &str =
    r#"<rect x="4" y="6" width="16" height="13" rx="2.5"/><path d="M4 10h16M7 8h.01M10 8h.01"/>"#;
const DISPLAY_ICON: &str =
    r#"<rect x="3" y="4" width="18" height="14" rx="2.5"/><path d="M9 21h6M12 18v3"/>"#;

fn icon(path: &'static str, color: Rgba) -> Img {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" color="#{:02x}{:02x}{:02x}" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"##,
        (color.r * 255.) as u8,
        (color.g * 255.) as u8,
        (color.b * 255.) as u8
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .size(px(15.))
}

pub(super) fn clamp_panel(
    position: (f32, f32),
    size: (f32, f32),
    viewport: (f32, f32),
) -> (f32, f32) {
    (
        position.0.clamp(8., (viewport.0 - size.0 - 8.).max(8.)),
        position.1.clamp(8., (viewport.1 - size.1 - 8.).max(8.)),
    )
}

fn field(label: &'static str, width: f32, child: impl IntoElement, t: Theme) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(10.))
                .line_height(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.glass_muted)
                .child(label),
        )
        .child(child)
}

impl Selector {
    fn animate_menu(
        &mut self,
        key: &'static str,
        value: f32,
        duration: f32,
        window: &mut Window,
    ) -> f32 {
        let now = Instant::now();
        let motion = self.menu_motion.entry(key).or_insert(Motion {
            from: value,
            to: value,
            started: now,
        });
        motion.retarget(value, now, duration);
        if theme::reduced_motion() {
            motion.from = value;
            return value;
        }
        if now.duration_since(motion.started).as_secs_f32() < duration && motion.from != motion.to {
            window.request_animation_frame();
        }
        motion.value(now, duration)
    }

    fn menu_button(
        &self,
        id: impl Into<ElementId>,
        active: bool,
        child: impl IntoElement,
        t: Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(28.))
            .px(px(12.))
            .rounded(px(6.))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .flex_shrink_0()
            .bg(if active {
                rgba(0xffffff1a)
            } else {
                rgba(0x00000000)
            })
            .text_color(if active { t.glass_text } else { t.glass_muted })
            .hover(move |s| s.bg(rgba(0xffffff12)).text_color(t.glass_text))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(child)
    }

    fn picker_button(
        &self,
        id: &'static str,
        picker: MenuPicker,
        label: impl IntoElement,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let measured = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let recorded = measured.clone();
        self.menu_button(id, false, label, t)
            .h(px(36.))
            .w_full()
            .min_w(px(40.))
            .border_1()
            .border_color(t.glass_border)
            .bg(rgba(0x0000004d))
            .justify_between()
            .child(icon(r#"<path d="m8 10 4 4 4-4"/>"#, t.glass_muted))
            .child(
                canvas(move |bounds, _, _| recorded.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0()
                    .size_full(),
            )
            .on_click(cx.listener(move |s, _, _, cx| {
                s.menu_anchor = measured.get();
                s.menu_open = (s.menu_open != Some(picker)).then_some(picker);
                if picker == MenuPicker::Microphone {
                    s.microphone_devices = microphone_devices();
                }
                cx.notify();
            }))
    }

    #[allow(clippy::too_many_arguments)]
    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        width: f32,
        value: bool,
        position: f32,
        enabled: bool,
        t: Theme,
        click: impl Fn(&mut Selector) + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let switch = div()
            .relative()
            .w(px(30.))
            .h(px(18.))
            .rounded_full()
            .border_1()
            .border_color(if value { t.accent } else { t.glass_border })
            .bg(if value { t.accent } else { rgba(0x00000059) })
            .child(
                div()
                    .absolute()
                    .left(px(2. + 12. * position))
                    .top(px(2.))
                    .size(px(12.))
                    .rounded_full()
                    .bg(if value { rgb(0x000000) } else { t.glass_muted }),
            );
        field(
            label,
            width,
            self.menu_button(
                id,
                false,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(switch)
                    .child(if !enabled {
                        "Unavailable"
                    } else if value {
                        "On"
                    } else {
                        "Off"
                    }),
                t,
            )
            .h(px(36.))
            .px_0()
            .text_size(px(11.))
            .justify_start()
            .opacity(if enabled { 1. } else { 0.5 })
            .when(enabled, |b| {
                b.on_click(cx.listener(move |s, _, _, cx| {
                    click(s);
                    cx.notify();
                }))
            }),
            t,
        )
    }

    fn dropdown(
        &self,
        picker: MenuPicker,
        window: &Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = |id: ElementId, label: String, selected: bool| {
            self.menu_button(id, selected, label, t)
                .h(px(32.))
                .w_full()
                .justify_between()
                .child(if selected { "✓" } else { "" })
        };
        let mut rows = Vec::new();
        match picker {
            MenuPicker::Aspect => {
                for (i, value) in [
                    RegionAspect::Free,
                    RegionAspect::Square,
                    RegionAspect::Landscape4x3,
                    RegionAspect::Landscape3x2,
                    RegionAspect::Landscape16x9,
                    RegionAspect::Portrait9x16,
                ]
                .into_iter()
                .enumerate()
                {
                    rows.push(
                        row(
                            ("aspect", i).into(),
                            value.label().into(),
                            self.region_aspect == value,
                        )
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.region_aspect = value;
                            s.selection =
                                s.selection.map(|r| r.with_aspect(value, s.overlay_size()));
                            s.menu_open = None;
                            cx.notify();
                        })),
                    );
                }
            }
            MenuPicker::Display => {
                for (i, display) in self.displays.iter().enumerate() {
                    rows.push(
                        row(
                            ("display", i).into(),
                            display.name.clone(),
                            self.display == i,
                        )
                        .on_click(
                            cx.listener(move |s, _, window, cx| s.switch_display(i, window, cx)),
                        ),
                    );
                }
            }
            MenuPicker::Fps => {
                for fps in [60_u16, 30, 15] {
                    rows.push(
                        row(
                            ("fps", fps as usize).into(),
                            fps.to_string(),
                            self.settings.video_fps == fps,
                        )
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.settings.video_fps = fps;
                            s.menu_open = None;
                            cx.notify();
                        })),
                    );
                }
            }
            MenuPicker::Resolution => {
                for (i, (label, value)) in [
                    ("Original", captures_recording::MaxResolution::Original),
                    ("1080p", captures_recording::MaxResolution::P1080),
                    ("720p", captures_recording::MaxResolution::P720),
                ]
                .into_iter()
                .enumerate()
                {
                    rows.push(
                        row(
                            ("resolution", i).into(),
                            label.into(),
                            self.settings.video_max_resolution == value,
                        )
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.settings.video_max_resolution = value;
                            s.menu_open = None;
                            cx.notify();
                        })),
                    );
                }
            }
            MenuPicker::Microphone => {
                rows.push(
                    row(
                        "microphone-off".into(),
                        "Off".into(),
                        self.settings.microphone_device_id.is_none(),
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.settings.microphone_device_id = None;
                        s.menu_open = None;
                        cx.notify();
                    })),
                );
                for (i, device) in self.microphone_devices.iter().enumerate() {
                    let id = device.id.clone();
                    rows.push(
                        row(
                            ("microphone", i).into(),
                            device.name.clone(),
                            self.settings.microphone_device_id.as_ref() == Some(&id),
                        )
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.settings.microphone_device_id = Some(id.clone());
                            s.menu_open = None;
                            cx.notify();
                        })),
                    );
                }
            }
        }
        let height = (rows.len() as f32 * 34. + 10.).min(240.);
        let above =
            self.menu_anchor.bottom() + px(height + 6.) > window.viewport_size().height - px(8.);
        deferred(
            anchored()
                .anchor(if above {
                    Corner::BottomRight
                } else {
                    Corner::TopRight
                })
                .position(point(
                    self.menu_anchor.right(),
                    if above {
                        self.menu_anchor.top() - px(6.)
                    } else {
                        self.menu_anchor.bottom() + px(6.)
                    },
                ))
                .snap_to_window_with_margin(Edges::all(px(8.)))
                .child(
                    div()
                        .id("capture-picker-list")
                        .occlude()
                        .w(self.menu_anchor.size.width.max(px(130.)).min(px(360.)))
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .p(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.glass_border)
                        .bg(rgb(0x19191e))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .on_mouse_down_out(cx.listener(|s, _, _, cx| {
                            s.menu_open = None;
                            cx.notify();
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .children(rows),
                ),
        )
        .into_any_element()
    }

    fn switch_mode(&mut self, mode: ActionMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.gif = false;
        self.menu_open = None;
        cx.notify();
    }

    fn switch_display(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.display {
            self.menu_open = None;
            cx.notify();
            return;
        }
        // A new native overlay is necessary: changing an index alone leaves the
        // old monitor's frozen pixels and window origin in use.
        let result = (|| -> anyhow::Result<Self> {
            let mut next = Selector::new(self.launch.clone())?;
            next.mode = self.mode;
            next.gif = self.gif;
            next.target = self.target;
            next.settings = self.settings.clone();
            next.preferences = self.preferences.clone();
            next.display = index;
            next.frozen = None;
            next.backdrop = None;
            if !next.launch.mock && next.preferences.freeze_screen {
                let frame = Arc::new(XcapBackend.capture_display(&next.displays[index].id)?);
                let mut image = frame.image.clone();
                for p in image.pixels_mut() {
                    p.0.swap(0, 2);
                }
                next.backdrop = Some(Arc::new(RenderImage::new([image::Frame::new(image)])));
                next.frozen = Some(frame);
            }
            Ok(next)
        })();
        match result.and_then(|next| show_selector(next, cx)) {
            Ok(handle) => {
                let _ = crate::present_window(handle.into(), cx);
                window.remove_window();
            }
            Err(error) => {
                self.status = format!("Could not switch display: {error:#}");
                self.menu_open = None;
                cx.notify();
            }
        }
    }

    pub(super) fn render_capture_menu(
        &mut self,
        t: Theme,
        selection: Option<Rect>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let narrow = f32::from(viewport.width) <= 820.;
        let recording = self.mode == ActionMode::Recording;
        let pointer = pointer_features_available();
        let expanded = self.animate_menu("mode", if recording { 1. } else { 0. }, 0.28, window);
        let target_position = self.animate_menu(
            "target",
            match self.target {
                TargetMode::Region => 0.,
                TargetMode::Window => 1.,
                TargetMode::Display => 2.,
            },
            0.28,
            window,
        );
        let cursor_position =
            self.animate_menu("cursor", f32::from(self.settings.show_cursor), 0.2, window);
        let click_position = self.animate_menu(
            "click",
            f32::from(self.settings.highlight_clicks),
            0.2,
            window,
        );
        let audio_position = self.animate_menu(
            "audio",
            f32::from(self.settings.capture_system_audio),
            0.2,
            window,
        );
        let panel_width = if narrow {
            f32::from(viewport.width) - 32.
        } else {
            854. + expanded * 48.
                + if self.target == TargetMode::Display {
                    40.
                } else if self.target == TargetMode::Window {
                    -158.
                } else {
                    0.
                }
        };
        let option_height = if narrow { 200. } else { 78. };
        let panel_height = 86. + option_height * expanded;
        let position = clamp_panel(
            self.panel_position.unwrap_or((
                (f32::from(viewport.width) - panel_width) / 2.,
                f32::from(viewport.height) - 26. - panel_height,
            )),
            (panel_width, panel_height),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );

        let action_switch = div()
            .relative()
            .flex()
            .flex_shrink_0()
            .p(px(3.))
            .border_1()
            .border_color(rgba(0x00000000))
            .rounded(px(10.))
            .bg(rgba(0x00000052))
            .child(
                div()
                    .absolute()
                    .left(px(3. + 96. * expanded))
                    .top(px(3.))
                    .w(px(96.))
                    .h(px(28.))
                    .rounded(px(6.))
                    .bg(rgba(0xffffff1a)),
            )
            .child(
                self.menu_button(
                    "menu-screenshot",
                    !recording,
                    div()
                        .flex()
                        .gap(px(6.))
                        .items_center()
                        .child(icon(REGION_ICON, t.glass_text))
                        .child("Screenshot"),
                    t,
                )
                .w(px(96.))
                .bg(rgba(0x00000000))
                .on_click(cx.listener(|s, _, _, cx| s.switch_mode(ActionMode::Screenshot, cx))),
            )
            .child(
                self.menu_button(
                    "menu-record",
                    recording,
                    div()
                        .flex()
                        .gap(px(6.))
                        .items_center()
                        .child(div().size(px(9.)).rounded_full().bg(t.signal))
                        .child("Record"),
                    t,
                )
                .w(px(96.))
                .bg(rgba(0x00000000))
                .on_click(cx.listener(|s, _, _, cx| s.switch_mode(ActionMode::Recording, cx))),
            );
        let target_switch = div()
            .relative()
            .flex()
            .flex_shrink_0()
            .p(px(3.))
            .border_1()
            .border_color(rgba(0x00000000))
            .rounded(px(10.))
            .bg(rgba(0x00000052))
            .child(
                div()
                    .absolute()
                    .left(px(3. + if narrow { 36. } else { 96. } * target_position))
                    .top(px(3.))
                    .w(px(if narrow { 36. } else { 96. }))
                    .h(px(28.))
                    .rounded(px(6.))
                    .bg(rgba(0xffffff1a)),
            )
            .children(
                [
                    (TargetMode::Region, "target-region", "Region", REGION_ICON),
                    (TargetMode::Window, "target-window", "Window", WINDOW_ICON),
                    (
                        TargetMode::Display,
                        "target-display",
                        "Full screen",
                        DISPLAY_ICON,
                    ),
                ]
                .into_iter()
                .map(|(target, id, label, path)| {
                    let enabled = target != TargetMode::Window || !self.windows.is_empty();
                    self.menu_button(
                        id,
                        self.target == target,
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .child(icon(path, t.glass_text))
                            .when(!narrow, |e| e.child(label)),
                        t,
                    )
                    .w(px(if narrow { 36. } else { 96. }))
                    .bg(rgba(0x00000000))
                    .opacity(if enabled { 1. } else { 0.35 })
                    .when(enabled, |b| {
                        b.on_click(cx.listener(move |s, _, window, cx| {
                            s.target = target;
                            s.menu_open = None;
                            s.selected_window = None;
                            if target == TargetMode::Display
                                && s.preferences.auto_start_on_selection
                            {
                                if s.mode == ActionMode::Screenshot {
                                    s.capture(&ClickEvent::default(), window, cx);
                                } else {
                                    s.record(&ClickEvent::default(), window, cx);
                                }
                            }
                            cx.notify();
                        }))
                    })
                }),
            );
        let selector = match self.target {
            TargetMode::Region => div()
                .w(px(150.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.glass_muted)
                        .child("Aspect"),
                )
                .child(div().flex_1().child(self.picker_button(
                    "aspect-picker",
                    MenuPicker::Aspect,
                    self.region_aspect.label(),
                    t,
                    cx,
                ))),
            TargetMode::Display => div().w(px(190.)).flex_shrink_0().child(
                self.picker_button(
                    "display-picker",
                    MenuPicker::Display,
                    self.chosen_display()
                        .map_or("Display".into(), |d| d.name.clone()),
                    t,
                    cx,
                ),
            ),
            TargetMode::Window => div(),
        };
        let can_start = self.target().is_some() && !self.busy;
        let ping = if recording && can_start && !theme::reduced_motion() {
            window.request_animation_frame();
            cubic_ease(
                (self.menu_motion["mode"].started.elapsed().as_secs_f32() % 1.1 / 0.825).min(1.),
                0.,
                0.,
                0.2,
                1.,
            )
        } else {
            1.
        };
        let primary = self
            .menu_button(
                "menu-primary",
                false,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(if recording {
                        div()
                            .relative()
                            .size(px(10.))
                            .rounded_full()
                            .bg(t.signal)
                            .border_1()
                            .border_color(black())
                            .child(
                                div()
                                    .absolute()
                                    .left(px(-5. * ping))
                                    .top(px(-5. * ping))
                                    .size(px(10. * (1. + ping)))
                                    .rounded_full()
                                    .bg(t.signal)
                                    .opacity(0.7 * (1. - ping)),
                            )
                            .into_any_element()
                    } else {
                        icon(REGION_ICON, rgb(0x000000)).into_any_element()
                    })
                    .child(if recording {
                        "Start recording"
                    } else {
                        "Capture"
                    }),
                t,
            )
            .h(px(40.))
            .min_w(px(if recording { 152. } else { 112. }))
            .px(px(16.))
            .rounded(px(10.))
            .bg(t.accent)
            .text_color(black())
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .opacity(if can_start { 1. } else { 0.4 })
            .hover(move |s| s.bg(t.accent).text_color(black()))
            .when(can_start, |b| {
                if recording {
                    b.on_click(cx.listener(Self::record))
                } else {
                    b.on_click(cx.listener(Self::capture))
                }
            });
        let top = div()
            .h(px(56.))
            .px(px(8.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                self.menu_button(
                    "menu-close",
                    false,
                    icon(r#"<path d="m7 7 10 10M17 7 7 17"/>"#, t.glass_muted),
                    t,
                )
                .size(px(32.))
                .px_0()
                .on_click(cx.listener(|s, _, window, _| {
                    s.lifecycle.cancel();
                    window.remove_window();
                })),
            )
            .child(action_switch)
            .child(
                div()
                    .w(px(1.))
                    .h(px(24.))
                    .flex_shrink_0()
                    .bg(t.glass_border),
            )
            .child(target_switch)
            .child(selector)
            .when(
                !self.preferences.auto_start_on_selection || self.busy || !self.status.is_empty(),
                |d| d.child(primary),
            );
        let width = |wide| {
            if narrow {
                (panel_width - 34.) / 2.
            } else {
                wide
            }
        };
        let options = div()
            .px(px(12.))
            .py(px(12.))
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(field(
                "FPS",
                width(76.),
                self.picker_button(
                    "fps-picker",
                    MenuPicker::Fps,
                    self.settings.video_fps.to_string(),
                    t,
                    cx,
                ),
                t,
            ))
            .child(field(
                "MAX RESOLUTION",
                width(132.),
                self.picker_button(
                    "resolution-picker",
                    MenuPicker::Resolution,
                    match self.settings.video_max_resolution {
                        captures_recording::MaxResolution::Original => "Original",
                        captures_recording::MaxResolution::P1080 => "1080p",
                        captures_recording::MaxResolution::P720 => "720p",
                    },
                    t,
                    cx,
                ),
                t,
            ))
            .child(self.toggle(
                "cursor-toggle",
                "SHOW CURSOR",
                width(92.),
                self.settings.show_cursor,
                cursor_position,
                pointer,
                t,
                |s| {
                    s.settings.show_cursor = !s.settings.show_cursor;
                    if !s.settings.show_cursor {
                        s.settings.highlight_clicks = false;
                    }
                },
                cx,
            ))
            .child(self.toggle(
                "click-toggle",
                "SHOW CLICKS",
                width(92.),
                self.settings.highlight_clicks,
                click_position,
                pointer,
                t,
                |s| {
                    s.settings.highlight_clicks = !s.settings.highlight_clicks;
                    if s.settings.highlight_clicks {
                        s.settings.show_cursor = true;
                    }
                },
                cx,
            ))
            .child(self.toggle(
                "audio-toggle",
                "DESKTOP AUDIO",
                width(118.),
                self.settings.capture_system_audio,
                audio_position,
                true,
                t,
                |s| s.settings.capture_system_audio = !s.settings.capture_system_audio,
                cx,
            ))
            .child(field(
                "MICROPHONE",
                width(240.),
                self.picker_button(
                    "microphone-picker",
                    MenuPicker::Microphone,
                    self.settings
                        .microphone_device_id
                        .as_ref()
                        .map_or("Off".to_owned(), |id| {
                            self.microphone_devices
                                .iter()
                                .find(|d| &d.id == id)
                                .map_or("Selected microphone".into(), |d| d.name.clone())
                        }),
                    t,
                    cx,
                ),
                t,
            ));
        let note = if !self.status.is_empty() {
            self.status.clone()
        } else if self.preferences.auto_start_on_selection {
            "Auto-capture is on. Selecting a target starts immediately.".into()
        } else if recording && cfg!(target_os = "linux") {
            "Controls may appear in recordings · Press Enter to confirm".into()
        } else {
            format!(
                "These controls won’t show in {} · Press Enter to confirm",
                if recording {
                    "recordings"
                } else {
                    "screenshots"
                }
            )
        };
        let measured = self.panel_bounds.clone();
        let panel = div()
            .absolute()
            .left(px(position.0))
            .top(px(position.1))
            .w(px(panel_width))
            .h(px(panel_height))
            .rounded(px(18.))
            .border_1()
            .border_color(t.glass_border)
            .bg(rgba(0x0f0f12ed))
            .flex()
            .flex_col()
            .child(
                canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0()
                    .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, event: &MouseDownEvent, _, cx| {
                    let origin = s.panel_bounds.get().origin;
                    s.panel_drag = Some((
                        (f32::from(event.position.x), f32::from(event.position.y)),
                        (f32::from(origin.x), f32::from(origin.y)),
                    ));
                    s.menu_open = None;
                    cx.stop_propagation();
                }),
            )
            .child(top)
            .when(expanded > 0., |p| {
                p.child(
                    div()
                        .h(px(option_height * expanded))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .border_t_1()
                        .border_color(t.glass_border)
                        .opacity(expanded)
                        .child(options),
                )
            })
            .child(
                div()
                    .h(px(28.))
                    .pb(px(8.))
                    .px(px(12.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(t.glass_muted)
                    .child(note),
            );
        div()
            .absolute()
            .inset_0()
            .when(
                self.target == TargetMode::Region && selection.is_none(),
                |root| {
                    root.child(
                        div()
                            .absolute()
                            .top(relative(0.16))
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .px(px(16.))
                                    .py(px(12.))
                                    .rounded(px(12.))
                                    .bg(t.glass)
                                    .border_1()
                                    .border_color(t.glass_border)
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Drag to select a region"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(t.glass_muted)
                                            .child("Shift for square · Esc to cancel"),
                                    ),
                            ),
                    )
                },
            )
            .when_some(
                selection.filter(|_| self.target == TargetMode::Region),
                |root, r| {
                    root.child(
                        div()
                            .absolute()
                            .left(px(r.x + r.width / 2. - 38.))
                            .top(px((r.y - 32.).max(8.)))
                            .px(px(9.))
                            .py(px(5.))
                            .rounded(px(7.))
                            .bg(t.glass)
                            .border_1()
                            .border_color(t.glass_border)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} × {}", r.width.round(), r.height.round())),
                    )
                },
            )
            .child(panel)
            .when_some(self.menu_open, |root, picker| {
                root.child(self.dropdown(picker, window, t, cx))
            })
            .into_any_element()
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) fn microphone_devices() -> Vec<captures_recording::AudioDevice> {
    captures_recording_xcap::microphone_devices()
}
#[cfg(target_os = "macos")]
pub(super) fn microphone_devices() -> Vec<captures_recording::AudioDevice> {
    captures_recording_macos::microphone_devices()
}
pub(super) fn pointer_features_available() -> bool {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        captures_recording_xcap::pointer_features_available()
    }
    #[cfg(target_os = "macos")]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Motion, clamp_panel, standard_ease};
    use std::time::{Duration, Instant};
    #[test]
    fn menu_motion_uses_source_curve_and_reverses_without_jumping() {
        // Parametric t=.5 gives x=.275 and y=.8 for the source Bezier.
        assert!((standard_ease(0.275) - 0.8).abs() < 0.00001);
        let now = Instant::now();
        let mut motion = Motion {
            from: 0.,
            to: 1.,
            started: now,
        };
        let halfway = now + Duration::from_millis(100);
        let value = motion.value(halfway, 0.2);
        motion.retarget(0., halfway, 0.2);
        assert_eq!(motion.value(halfway, 0.2), value);
        assert_eq!(motion.value(halfway + Duration::from_millis(200), 0.2), 0.);
    }
    #[test]
    fn dragging_clamps_the_entire_panel_on_both_axes() {
        assert_eq!(
            clamp_panel((1500., 990.), (902., 164.), (1600., 1000.)),
            (690., 828.)
        );
        assert_eq!(
            clamp_panel((-40., -100.), (902., 164.), (1600., 1000.)),
            (8., 8.)
        );
        assert_eq!(
            clamp_panel((400., 510.), (902., 164.), (1600., 1000.)),
            (400., 510.)
        );
    }
}
