//! GPU-rendered target selector; only backend work leaves the UI thread.
mod controls;
#[path = "capture/geometry.rs"]
mod geometry;
use controls::Picker;

use crate::{app, settings::Settings, ui};
use captures_capture::{
    CaptureMode, DisplayDescriptor, DisplayFrame, LogicalRect, PointerCursor, WindowDescriptor,
    XcapBackend,
};
use captures_recording::{CaptureRect, RecordingTarget};
use geometry::{Drag, contains, drag_at, drag_rect};
use gpui::{prelude::*, *};
use std::sync::Arc;

pub struct Prepared {
    pub frame: DisplayFrame,
    pub displays: Vec<DisplayDescriptor>,
    pub windows: Vec<WindowDescriptor>,
    pub texture: Arc<RenderImage>,
}

pub struct Selection {
    pub display: DisplayDescriptor,
    pub target: RecordingTarget,
    pub image: Arc<image::RgbaImage>,
    pub rect: LogicalRect,
    pub kind: u32,
    pub settings: Settings,
    pub pointer: (i32, i32),
    pub window: Option<WindowDescriptor>,
}

pub fn prepare(display: Option<String>, pointer: (i32, i32)) -> Result<Prepared, String> {
    if !captures_session::capture_session_available() {
        return Err("Capture is unavailable: the desktop is locked, inactive, or its session cannot be verified.".into());
    }
    let backend = XcapBackend;
    let frame = match display {
        Some(id) => backend.capture_display(&id),
        None => backend.capture_display_at_point(Some(pointer)),
    }
    .map_err(|e| e.to_string())?;
    let displays = backend.displays().map_err(|e| e.to_string())?;
    let windows = backend.windows().map_err(|e| e.to_string())?;
    let texture = ui::render_image(frame.image.clone());
    Ok(Prepared {
        frame,
        displays,
        windows,
        texture,
    })
}

pub fn pixels(selection: &Selection) -> Result<image::RgbaImage, String> {
    if !captures_session::capture_session_available() {
        return Err("Capture cancelled: desktop session unavailable".into());
    }
    let pointer = PointerCursor {
        position: selection.pointer,
        image: None,
    };
    if let RecordingTarget::Window { window_id } = &selection.target {
        let mut image = XcapBackend
            .capture_window(window_id)
            .map_err(|e| e.to_string())?;
        if selection.settings.show_cursor_in_screenshots
            && let Some(window) = &selection.window
        {
            captures_capture::overlay_pointer_cursor_on_window(&mut image, window, &pointer, 1.);
        }
        return Ok(image);
    }
    let refreshed;
    let source = if selection.settings.freeze_screen {
        &*selection.image
    } else {
        refreshed = XcapBackend
            .capture_display(&selection.display.id)
            .map_err(|e| e.to_string())?
            .image;
        &refreshed
    };
    let scale = selection
        .display
        .overlay_to_buffer_scale(source.width(), source.height());
    let rect = selection
        .rect
        .to_physical(scale, source.width(), source.height());
    if rect.width == 0 || rect.height == 0 {
        return Err("Select an area larger than zero pixels".into());
    }
    let mut image =
        image::imageops::crop_imm(source, rect.x, rect.y, rect.width, rect.height).to_image();
    if selection.settings.show_cursor_in_screenshots {
        captures_capture::overlay_pointer_cursor_in_crop(
            &mut image,
            &selection.display,
            rect.x,
            rect.y,
            source.width(),
            source.height(),
            &pointer,
            1.,
        );
    }
    Ok(image)
}

pub struct Selector {
    prepared: Prepared,
    settings: Settings,
    pub mode: CaptureMode,
    pub kind: u32,
    rect: LogicalRect,
    window_target: Option<WindowDescriptor>,
    drag: Option<Drag>,
    panel_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    panel_origin: Point<Pixels>,
    options: bool,
    aspect_index: usize,
    picker: Option<Picker>,
    microphones: Vec<captures_recording::AudioDevice>,
    microphones_loaded: bool,
    focus: FocusHandle,
    pointer: (i32, i32),
}

const ASPECTS: [(&str, Option<f64>); 5] = [
    ("Free", None),
    ("1:1", Some(1.)),
    ("4:3", Some(4. / 3.)),
    ("16:9", Some(16. / 9.)),
    ("9:16", Some(9. / 16.)),
];

pub fn open(
    prepared: Prepared,
    settings: Settings,
    mode: CaptureMode,
    kind: u32,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Selector>> {
    let (x, y, w, h) = prepared.frame.descriptor.overlay_geometry();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: size(px(w as f32), px(h as f32)),
        })),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::PopUp,
        is_movable: false,
        focus: true,
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        window.set_window_title("Captures GPUI Select target");
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            Selector {
                prepared,
                settings,
                mode,
                kind,
                rect: LogicalRect::default(),
                window_target: None,
                drag: None,
                panel_drag: None,
                panel_origin: point(
                    px(((w - 854.) / 2.).max(16.) as f32),
                    px((h - 112.).max(16.) as f32),
                ),
                options: false,
                aspect_index: 0,
                picker: None,
                microphones: vec![],
                microphones_loaded: false,
                focus,
                pointer: (x as i32, y as i32),
            }
        })
    })
}

impl Selector {
    pub fn switch(&mut self, mode: CaptureMode, kind: u32, cx: &mut Context<Self>) {
        self.mode = mode;
        self.kind = kind;
        self.rect = LogicalRect::default();
        self.window_target = None;
        self.drag = None;
        cx.notify();
    }

    fn dimensions(&self) -> (f64, f64) {
        self.prepared.frame.descriptor.overlay_size()
    }

    fn selected_rect(&self) -> LogicalRect {
        if self.mode == CaptureMode::Display {
            let (w, h) = self.dimensions();
            LogicalRect {
                x: 0.,
                y: 0.,
                width: w,
                height: h,
            }
        } else {
            self.rect
        }
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rect = self.selected_rect();
        if rect.width < 1. || rect.height < 1. {
            return;
        }
        let display = self.prepared.frame.descriptor.clone();
        let target = match (&self.mode, &self.window_target) {
            (CaptureMode::Window, Some(win)) => RecordingTarget::Window {
                window_id: win.id.clone(),
            },
            (CaptureMode::Display, _) | (CaptureMode::Window, None) => RecordingTarget::Display {
                display_id: display.id.clone(),
            },
            _ => RecordingTarget::Region {
                display_id: display.id.clone(),
                rect: CaptureRect {
                    x: rect.x.round() as i32,
                    y: rect.y.round() as i32,
                    width: rect.width.round() as u32,
                    height: rect.height.round() as u32,
                },
            },
        };
        // Move the frozen buffer out, rather than copying a full display on confirm.
        let image = Arc::new(std::mem::take(&mut self.prepared.frame.image));
        let selection = Selection {
            display,
            target,
            image,
            rect,
            kind: self.kind,
            settings: self.settings.clone(),
            pointer: self.pointer,
            window: self.window_target.clone(),
        };
        window.remove_window();
        cx.defer(move |cx| app::selected(selection, cx));
    }

    fn moved(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = (f64::from(event.position.x), f64::from(event.position.y));
        let (width, height) = self.dimensions();
        let display = &self.prepared.frame.descriptor;
        self.pointer = (display.x + x as i32, display.y + y as i32);
        if let Some((anchor, origin)) = self.panel_drag {
            self.panel_origin = point(
                (origin.x + event.position.x - anchor.x)
                    .clamp(px(0.), px((width - 854.).max(0.) as f32)),
                (origin.y + event.position.y - anchor.y)
                    .clamp(px(0.), px((height - 96.).max(0.) as f32)),
            );
            cx.notify();
            return;
        }
        if let Some(drag) = self.drag {
            self.rect = drag_rect(
                drag,
                x,
                y,
                width,
                height,
                if event.modifiers.shift {
                    Some(1.)
                } else {
                    ASPECTS[self.aspect_index].1
                },
            );
            cx.notify();
        } else if self.mode == CaptureMode::Window {
            let target = self
                .prepared
                .windows
                .iter()
                .find(|w| {
                    let r = LogicalRect {
                        x: f64::from(w.x - display.x),
                        y: f64::from(w.y - display.y),
                        width: f64::from(w.width),
                        height: f64::from(w.height),
                    };
                    w.display_id == display.id && contains(r, x, y)
                })
                .cloned();
            self.rect = target.as_ref().map_or(
                LogicalRect {
                    x: 0.,
                    y: 0.,
                    width,
                    height,
                },
                |w| LogicalRect {
                    x: f64::from(w.x - display.x),
                    y: f64::from(w.y - display.y),
                    width: f64::from(w.width),
                    height: f64::from(w.height),
                },
            );
            self.window_target = target;
            cx.notify();
        }
    }

    fn down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == CaptureMode::Region {
            let (x, y) = (f64::from(event.position.x), f64::from(event.position.y));
            self.drag = Some(drag_at(self.rect, x, y));
            if matches!(self.drag, Some(Drag::New(..))) {
                self.rect = LogicalRect {
                    x,
                    y,
                    width: 0.,
                    height: 0.,
                };
            }
            cx.notify();
        } else if self.settings.auto_start_on_selection {
            self.accept(window, cx);
        }
    }

    fn up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let selecting = self.drag.take().is_some();
        self.panel_drag = None;
        if selecting && self.settings.auto_start_on_selection {
            self.accept(window, cx);
        } else {
            cx.notify();
        }
    }

    fn glass_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        cx: &App,
    ) -> Stateful<Div> {
        let p = ui::theme(cx);
        ui::button(id, label, p)
            .h(ui::metric("--h-xl"))
            .border_0()
            .bg(if active {
                p.glass_text().opacity(0.14)
            } else {
                gpui::transparent_black()
            })
            .text_color(if active {
                p.glass_text()
            } else {
                p.glass_text().opacity(0.64)
            })
            .hover(move |s| s.bg(p.glass_text().opacity(0.09)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = ui::theme(cx);
        let mode = self.mode;
        let kind = self.kind;
        let mut top = div()
            .flex()
            .items_center()
            .gap(ui::metric("--s-4"))
            .p(ui::metric("--s-4"));
        top = top.child(
            self.glass_button("close", "", false, cx)
                .px_0()
                .w(ui::metric("--h-md"))
                .child(ui::icon("close"))
                .on_click(|_, window, cx| {
                    window.remove_window();
                    app::cancel_capture(cx);
                }),
        );
        let mut actions = div()
            .flex()
            .rounded(ui::metric("--r-lg"))
            .bg(p.glass_text().opacity(0.04));
        for (i, (label, icon)) in [("Screenshot", "capture"), ("Record", "record-dot")]
            .into_iter()
            .enumerate()
        {
            actions = actions.child(
                self.glass_button(["screenshot", "video"][i], "", (kind != 0) == (i != 0), cx)
                    .child(ui::icon(icon).text_color(if i == 0 {
                        p.glass_text()
                    } else {
                        p.signal
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kind = i as u32;
                        cx.notify();
                    })),
            );
        }
        top = top.child(actions).child(
            div()
                .w(px(1.))
                .h(ui::metric("--h-xs"))
                .bg(p.glass_text().opacity(0.11)),
        );
        let mut targets = div()
            .flex()
            .rounded(ui::metric("--r-lg"))
            .bg(p.glass_text().opacity(0.04));
        for (i, (target, label)) in [
            (CaptureMode::Region, "Region"),
            (CaptureMode::Window, "Window"),
            (CaptureMode::Display, "Full screen"),
        ]
        .into_iter()
        .enumerate()
        {
            targets = targets.child(
                self.glass_button(["region", "window", "display"][i], "", mode == target, cx)
                    .child(ui::icon(["region", "window", "display"][i]))
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.switch(target, this.kind, cx)),
                    ),
            );
        }
        top = top.child(targets).child(div().flex_1());
        if mode == CaptureMode::Region {
            top = top
                .child(
                    div()
                        .text_size(ui::metric("--text-xs"))
                        .text_color(p.glass_text().opacity(0.42))
                        .child("Aspect"),
                )
                .child(self.picker(Picker::Aspect, cx));
        } else if mode == CaptureMode::Display {
            top = top.child(self.picker(Picker::Display, cx));
        }
        top = top.child(
            self.glass_button("confirm", "", true, cx)
                .child(
                    ui::icon(if kind == 0 { "capture" } else { "record-dot" }).text_color(
                        if kind == 0 {
                            ui::Theme { dark: false, ..p }.text()
                        } else {
                            p.signal
                        },
                    ),
                )
                .child(if kind == 0 { "Capture" } else { "Record" })
                .bg(p.accent)
                .text_color(ui::Theme { dark: false, ..p }.text())
                .on_click(cx.listener(|this, _, window, cx| this.accept(window, cx))),
        );
        let hint = if self.settings.auto_start_on_selection {
            "Capture starts after selection"
        } else {
            "Controls will show after selection"
        };
        let panel_height =
            90. + if kind != 0 { 112. } else { 0. } + if self.options { 140. } else { 0. };
        let top_y = self
            .panel_origin
            .y
            .min(px((self.dimensions().1 as f32 - panel_height - 26.).max(0.)));
        let mut panel = div()
            .id("capture-toolbar")
            .absolute()
            .left(self.panel_origin.x)
            .top(top_y)
            .w(px(854.))
            .rounded(ui::metric("--r-2xl"))
            .bg(p.glass())
            .text_color(p.glass_text())
            .border_1()
            .border_color(p.glass_text().opacity(0.11))
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.panel_drag = Some((event.position, this.panel_origin));
                    cx.stop_propagation();
                }),
            )
            .child(top)
            .when(kind != 0, |panel| panel.child(self.recording_controls(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(ui::metric("--s-6"))
                    .pb(ui::metric("--s-4"))
                    .text_size(ui::metric("--text-xs"))
                    .text_color(p.glass_text().opacity(0.64))
                    .child(
                        self.glass_button("options", hint, self.options, cx)
                            .h(ui::metric("--h-xs"))
                            .px_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.options = !this.options;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child("Enter ↵"),
            );
        if self.options {
            let mut choices = div()
                .flex()
                .flex_wrap()
                .gap(ui::metric("--s-4"))
                .p(ui::metric("--s-5"))
                .border_t_1()
                .border_color(p.glass_text().opacity(0.11));
            choices = choices
                .child(
                    self.glass_button(
                        "freeze",
                        if self.settings.freeze_screen {
                            "Freeze screen: On"
                        } else {
                            "Freeze screen: Off"
                        },
                        self.settings.freeze_screen,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.freeze_screen = !this.settings.freeze_screen;
                        cx.notify();
                    })),
                )
                .child(
                    self.glass_button(
                        "cursor",
                        if self.settings.show_cursor_in_screenshots {
                            "Cursor: On"
                        } else {
                            "Cursor: Off"
                        },
                        self.settings.show_cursor_in_screenshots,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.show_cursor_in_screenshots =
                            !this.settings.show_cursor_in_screenshots;
                        cx.notify();
                    })),
                )
                .child(
                    self.glass_button(
                        "countdown",
                        format!(
                            "Countdown: {}s",
                            if kind == 0 {
                                self.settings.screenshot_countdown_seconds
                            } else {
                                self.settings.recording.countdown_seconds
                            }
                        ),
                        false,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let value = if this.kind == 0 {
                            &mut this.settings.screenshot_countdown_seconds
                        } else {
                            &mut this.settings.recording.countdown_seconds
                        };
                        *value = match *value {
                            0 => 3,
                            3 => 5,
                            5 => 10,
                            _ => 0,
                        };
                        cx.notify();
                    })),
                )
                .child(
                    self.glass_button(
                        "auto-start",
                        if self.settings.auto_start_on_selection {
                            "Auto-start: On"
                        } else {
                            "Auto-start: Off"
                        },
                        self.settings.auto_start_on_selection,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.auto_start_on_selection =
                            !this.settings.auto_start_on_selection;
                        cx.notify();
                    })),
                );
            if kind != 0 {
                choices = choices.child(
                    self.glass_button(
                        "system-audio",
                        if self.settings.recording.capture_system_audio {
                            "Desktop audio: On"
                        } else {
                            "Desktop audio: Off"
                        },
                        self.settings.recording.capture_system_audio,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.recording.capture_system_audio =
                            !this.settings.recording.capture_system_audio;
                        cx.notify();
                    })),
                );
            }
            for display in &self.prepared.displays {
                let id = display.id.clone();
                let mode = self.mode;
                let kind = self.kind;
                choices = choices.child(
                    self.glass_button(
                        "monitor",
                        display.name.clone(),
                        id == self.prepared.frame.descriptor.id,
                        cx,
                    )
                    .id(SharedString::from(format!("display-{id}")))
                    .on_click(move |_, window, cx| {
                        window.remove_window();
                        app::select_display(mode, kind, id.clone(), cx);
                    }),
                );
            }
            panel = panel.child(choices);
        }
        panel
    }
}

impl Render for Selector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = ui::theme(cx);
        let (w, h) = self.dimensions();
        let rect = self.selected_rect();
        let mut root = div()
            .id("selection-surface")
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family("DejaVu Sans")
            .text_size(ui::metric("--text-md"))
            .text_color(p.glass_text())
            .track_focus(&self.focus)
            .cursor(CursorStyle::Crosshair)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
            .on_mouse_move(cx.listener(Self::moved))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::up))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        window.remove_window();
                        app::cancel_capture(cx);
                    }
                    "enter" => this.accept(window, cx),
                    _ => {}
                }
            }));
        if self.settings.freeze_screen {
            root = root.child(
                img(self.prepared.texture.clone())
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Fill),
            );
        }
        let shade = p.glass().opacity(0.2);
        let holes = if rect.width > 0. && rect.height > 0. {
            vec![
                (0., 0., w, rect.y),
                (0., rect.y, rect.x, rect.height),
                (
                    rect.x + rect.width,
                    rect.y,
                    (w - rect.x - rect.width).max(0.),
                    rect.height,
                ),
                (
                    0.,
                    rect.y + rect.height,
                    w,
                    (h - rect.y - rect.height).max(0.),
                ),
            ]
        } else {
            vec![(0., 0., w, h)]
        };
        for (x, y, w, h) in holes {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x as f32))
                    .top(px(y as f32))
                    .w(px(w as f32))
                    .h(px(h as f32))
                    .bg(shade),
            );
        }
        if rect.width > 0. && rect.height > 0. {
            let mut outline = div()
                .absolute()
                .left(px(rect.x as f32))
                .top(px(rect.y as f32))
                .w(px(rect.width as f32))
                .h(px(rect.height as f32))
                .border_2()
                .border_color(p.accent);
            if self.mode == CaptureMode::Region {
                for (x, y) in [
                    (0., 0.),
                    (rect.width, 0.),
                    (0., rect.height),
                    (rect.width, rect.height),
                ] {
                    outline = outline.child(
                        div()
                            .absolute()
                            .left(px(x as f32 - 5.))
                            .top(px(y as f32 - 5.))
                            .size(ui::metric("--r-lg"))
                            .rounded_full()
                            .bg(p.accent)
                            .border_2()
                            .border_color(p.glass()),
                    );
                }
            }
            root = root.child(outline).child(
                div()
                    .absolute()
                    .left(px((rect.x + rect.width / 2. - 45.).max(0.) as f32))
                    .top(px((rect.y - 30.).max(4.) as f32))
                    .px(ui::metric("--s-4"))
                    .py(ui::metric("--s-2"))
                    .rounded(ui::metric("--r-sm"))
                    .bg(p.glass())
                    .text_size(ui::metric("--text-xs"))
                    .child(format!("{} × {}", rect.width.round(), rect.height.round())),
            );
        }
        root.child(self.toolbar(cx))
    }
}
