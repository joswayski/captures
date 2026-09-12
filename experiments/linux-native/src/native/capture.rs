//! GTK target selector. Geometry remains in display logical coordinates until capture.
use crate::compat::prelude::*;
use crate::{settings::Settings, ui};
use captures_capture::{
    CaptureMode, DisplayDescriptor, DisplayFrame, LogicalRect, PointerCursor, WindowDescriptor,
    XcapBackend,
};
use captures_recording::{CaptureRect, RecordingTarget};
use gtk::{gdk, glib, prelude::*};
use image::RgbaImage;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

#[derive(Clone)]
pub struct Selection {
    pub display: DisplayDescriptor,
    pub target: RecordingTarget,
    pub image: RgbaImage,
    pub kind: u32,
    pub settings: Settings,
    pub pointer: (i32, i32),
    rect: LogicalRect,
    window: Option<WindowDescriptor>,
}

pub fn pointer() -> (i32, i32) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `point` is valid writable storage for the duration of the call.
        if unsafe { GetCursorPos(&mut point) } != 0 {
            (point.x, point.y)
        } else {
            (0, 0)
        }
    }

    #[cfg(not(target_os = "windows"))]
    gdk::Display::default()
        .and_then(|d| d.default_seat())
        .and_then(|s| s.pointer())
        .map(|p| {
            let (_, x, y) = p.surface_at_position();
            (x as i32, y as i32)
        })
        .unwrap_or((0, 0))
}

pub fn select_with_options(
    mode: CaptureMode,
    kind: u32,
    settings: Settings,
    screenshot_only: bool,
    done: Rc<dyn Fn(Selection)>,
    cancelled: Rc<dyn Fn()>,
) {
    select_display(mode, kind, settings, None, screenshot_only, done, cancelled);
}

fn select_display(
    mode: CaptureMode,
    kind: u32,
    settings: Settings,
    display: Option<String>,
    screenshot_only: bool,
    done: Rc<dyn Fn(Selection)>,
    cancelled: Rc<dyn Fn()>,
) {
    let cursor = pointer();
    ui::job(
        move || {
            if !captures_session::capture_session_available() {
                return Err("Capture is unavailable: desktop locked, inactive, or its session state cannot be verified.".into());
            }
            captures_session::dismiss_transient_shell_ui_before_capture();
            let backend = XcapBackend;
            let displays = backend.displays().map_err(|e| e.to_string())?;
            let frame = match display {
                Some(id) => backend.capture_display(&id),
                None => backend.capture_display_at_point(Some(cursor)),
            }
            .map_err(|e| e.to_string())?;
            let windows = backend.windows().map_err(|e| e.to_string())?;
            Ok((frame, displays, windows))
        },
        move |result| match result {
            Ok((frame, displays, windows)) => overlay(
                mode,
                kind,
                settings,
                frame,
                displays,
                windows,
                cursor,
                screenshot_only,
                done,
                cancelled,
            ),
            Err(error) => {
                cancelled();
                ui::error(&gtk::Window::new(), &error);
            }
        },
    );
}

pub fn refresh(selection: &Selection, show_cursor: bool) -> Result<RgbaImage, String> {
    if !captures_session::capture_session_available() {
        return Err("Capture cancelled: desktop session unavailable".into());
    }
    let cursor = PointerCursor {
        position: selection.pointer,
        image: None,
    };
    if let RecordingTarget::Window { window_id } = &selection.target {
        let mut image = XcapBackend
            .capture_window(window_id)
            .map_err(|e| e.to_string())?;
        if show_cursor && let Some(window) = &selection.window {
            captures_capture::overlay_pointer_cursor_on_window(&mut image, window, &cursor, 1.);
        }
        return Ok(image);
    }
    let mut frame = XcapBackend
        .capture_display(&selection.display.id)
        .map_err(|e| e.to_string())?;
    normalize_display_scale(&mut frame.descriptor, selection.display.scale_factor);
    crop(&frame, selection.rect, show_cursor.then_some(cursor))
}

fn normalize_display_scale(display: &mut DisplayDescriptor, gtk_scale: f64) {
    #[cfg(target_os = "windows")]
    {
        display.scale_factor = gtk_scale.max(1.0);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (display, gtk_scale);
}

fn overlay_point_to_physical(display: &DisplayDescriptor, x: f64, y: f64) -> (f64, f64) {
    overlay_point_to_global(
        display,
        x,
        y,
        DisplayDescriptor::reports_physical_geometry(),
    )
}

fn overlay_point_to_global(
    display: &DisplayDescriptor,
    x: f64,
    y: f64,
    physical_geometry: bool,
) -> (f64, f64) {
    if physical_geometry {
        (
            f64::from(display.x) + x * display.scale_factor,
            f64::from(display.y) + y * display.scale_factor,
        )
    } else {
        (f64::from(display.x) + x, f64::from(display.y) + y)
    }
}

fn physical_window_to_overlay(
    display: &DisplayDescriptor,
    window: &WindowDescriptor,
) -> LogicalRect {
    window_to_overlay(
        display,
        window,
        DisplayDescriptor::reports_physical_geometry(),
    )
}

fn window_to_overlay(
    display: &DisplayDescriptor,
    window: &WindowDescriptor,
    physical_geometry: bool,
) -> LogicalRect {
    let scale = if physical_geometry {
        display.scale_factor.max(1.0)
    } else {
        1.0
    };
    LogicalRect {
        x: f64::from(window.x - display.x) / scale,
        y: f64::from(window.y - display.y) / scale,
        width: f64::from(window.width) / scale,
        height: f64::from(window.height) / scale,
    }
}

fn crop(
    frame: &DisplayFrame,
    rect: LogicalRect,
    cursor: Option<PointerCursor>,
) -> Result<RgbaImage, String> {
    let crop = rect.to_physical(
        frame
            .descriptor
            .overlay_to_buffer_scale(frame.image.width(), frame.image.height()),
        frame.image.width(),
        frame.image.height(),
    );
    let mut image = frame.crop(crop).ok_or("Selected region is empty")?;
    if let Some(cursor) = cursor {
        captures_capture::overlay_pointer_cursor_in_crop(
            &mut image,
            &frame.descriptor,
            crop.x,
            crop.y,
            frame.image.width(),
            frame.image.height(),
            &cursor,
            1.,
        );
    }
    Ok(image)
}

#[derive(Clone, Copy)]
enum Drag {
    New(f64, f64),
    Move(LogicalRect, f64, f64),
    Resize(LogicalRect, usize),
}

fn drag_rect(
    drag: Drag,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    aspect: Option<f64>,
) -> LogicalRect {
    let (x, y) = (x.clamp(0., width), y.clamp(0., height));
    let (sx, sy) = match drag {
        Drag::Move(mut r, sx, sy) => {
            r.x = (r.x + x - sx).clamp(0., (width - r.width).max(0.));
            r.y = (r.y + y - sy).clamp(0., (height - r.height).max(0.));
            return r;
        }
        Drag::New(sx, sy) => (sx, sy),
        Drag::Resize(r, corner) => match corner {
            0 => (r.x + r.width, r.y + r.height),
            1 => (r.x, r.y + r.height),
            2 => (r.x + r.width, r.y),
            _ => (r.x, r.y),
        },
    };
    let (mut dx, mut dy) = (x - sx, y - sy);
    if let Some(ratio) = aspect {
        let sign_x = if dx < 0. { -1. } else { 1. };
        let sign_y = if dy < 0. { -1. } else { 1. };
        let max_w = if sign_x > 0. { width - sx } else { sx };
        let max_h = if sign_y > 0. { height - sy } else { sy };
        let minimum = if matches!(drag, Drag::Resize(..)) {
            16. * ratio.max(1.)
        } else {
            0.
        };
        let w = dx
            .abs()
            .max(dy.abs() * ratio)
            .max(minimum)
            .min(max_w)
            .min(max_h * ratio);
        dx = w * sign_x;
        dy = w / ratio * sign_y;
    } else if let Drag::Resize(_, corner) = drag {
        dx = if corner.is_multiple_of(2) {
            dx.min(-16.)
        } else {
            dx.max(16.)
        };
        dy = if corner < 2 {
            dy.min(-16.)
        } else {
            dy.max(16.)
        };
    }
    LogicalRect {
        x: sx,
        y: sy,
        width: dx,
        height: dy,
    }
    .normalized()
}

#[allow(clippy::too_many_arguments)]
fn overlay(
    initial_mode: CaptureMode,
    initial_kind: u32,
    settings: Settings,
    mut frame: DisplayFrame,
    displays: Vec<DisplayDescriptor>,
    windows: Vec<WindowDescriptor>,
    cursor: (i32, i32),
    screenshot_only: bool,
    done: Rc<dyn Fn(Selection)>,
    cancelled: Rc<dyn Fn()>,
) {
    let window = gtk::Window::new();
    window.set_title(Some("Captures — Select target"));
    window.set_decorated(false);
    window.set_keep_above(true);
    window.style_context().add_class("floating");
    normalize_display_scale(&mut frame.descriptor, f64::from(window.scale_factor()));
    #[cfg(target_os = "windows")]
    if let Err(error) = crate::windows::exclude_from_capture(&window, true) {
        window.close();
        cancelled();
        ui::error(&gtk::Window::new(), &error);
        return;
    }
    let (ox, oy, width, height) = frame.descriptor.overlay_geometry();
    window.move_(ox as i32, oy as i32);
    window.set_default_size(width as i32, height as i32);
    window.set_resizable(false);
    let mode = Rc::new(Cell::new(initial_mode));
    let kind = Rc::new(Cell::new(initial_kind));
    let settings = Rc::new(RefCell::new(settings));
    let overlay = gtk::Overlay::new();
    let area = gtk::DrawingArea::new();
    overlay.add(&area);
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 0);
    controls.style_context().add_class("glass");
    controls.style_context().add_class("capture-selector");
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add(&controls);
    panel.set_halign(gtk::Align::Center);
    panel.set_valign(gtk::Align::End);
    panel.set_margin_bottom(26);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.style_context().add_class("capture-panel-top");
    let targets = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    targets.style_context().add_class("capture-segmented");
    let status = ui::label("", "capture-selector-note");
    status.set_halign(gtk::Align::Center);
    let accept = ui::button(if initial_kind == 0 {
        "Capture"
    } else {
        "Record"
    });
    accept.style_context().add_class("primary");
    accept.style_context().add_class("capture-primary");
    ui::named(
        &accept,
        if initial_kind == 0 {
            "Capture"
        } else {
            "Start recording"
        },
    );
    let action_icon = ui::icon(
        if initial_kind == 0 {
            "capture"
        } else {
            "record-dot"
        },
        16,
    );
    if initial_kind != 0 {
        action_icon.style_context().add_class("capture-record-dot");
    }
    accept.set_image(Some(&action_icon));
    accept.set_always_show_image(true);
    accept.set_no_show_all(settings.borrow().auto_start_on_selection);
    let cancel = ui::icon_button("Cancel · Esc", "window-close-symbolic");
    cancel.style_context().add_class("capture-close");
    toolbar.pack_start(&cancel, false, false, 0);
    let modes = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    modes.style_context().add_class("capture-segmented");
    ui::named(&modes, "Capture type");
    let screenshot = gtk::ToggleButton::with_label("Screenshot");
    screenshot.set_image(Some(&ui::icon("capture", 16)));
    screenshot.set_always_show_image(true);
    let record = gtk::ToggleButton::with_label("Record");
    let dot = gtk::DrawingArea::new();
    dot.set_size_request(9, 9);
    dot.set_valign(gtk::Align::Center);
    dot.connect_draw(|_, cr| {
        let c = ui::color("signal");
        cr.set_source_rgba(
            f64::from(c.red()),
            f64::from(c.green()),
            f64::from(c.blue()),
            1.,
        );
        cr.arc(4.5, 4.5, 4.5, 0., std::f64::consts::TAU);
        let _ = cr.fill();
        glib::Propagation::Stop
    });
    record.set_image(Some(&dot));
    record.set_always_show_image(true);
    record.set_sensitive(!screenshot_only);
    screenshot.set_active(initial_kind == 0);
    record.set_active(initial_kind != 0);
    modes.add(&screenshot);
    modes.add(&record);
    toolbar.pack_start(&modes, false, false, 0);
    let divider = gtk::Separator::new(gtk::Orientation::Vertical);
    divider.set_margin_top(8);
    divider.set_margin_bottom(8);
    toolbar.pack_start(&divider, false, false, 0);
    toolbar.pack_start(&targets, false, false, 0);
    let monitor = gtk::ComboBoxText::new();
    for display in &displays {
        monitor.append(Some(&display.id), &display.name);
    }
    monitor.set_active_id(Some(&frame.descriptor.id));
    monitor.set_sensitive(displays.len() > 1);
    monitor.set_size_request(190, -1);
    ui::named(&monitor, "Display");
    toolbar.pack_start(&monitor, false, false, 0);
    let ratio = gtk::ComboBoxText::new();
    for name in ["Free", "1 : 1", "4 : 3", "3 : 2", "16 : 9", "9 : 16"] {
        ratio.append_text(name);
    }
    ratio.set_active(Some(0));
    ui::named(&ratio, "Selection aspect ratio");
    let aspect = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    aspect.set_size_request(150, -1);
    aspect.pack_start(&ui::label("Aspect", "muted"), false, false, 0);
    aspect.pack_start(&ratio, false, false, 0);
    toolbar.pack_start(&aspect, false, false, 0);
    toolbar.pack_end(&accept, false, false, 0);
    controls.add(&toolbar);
    let options = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    options.style_context().add_class("capture-options");
    let fps = gtk::ComboBoxText::new();
    for value in [60, 30, 15] {
        fps.append(Some(&value.to_string()), &value.to_string());
    }
    fps.set_active_id(Some(
        &if initial_kind == 2 {
            settings.borrow().recording.gif_fps
        } else {
            settings.borrow().recording.video_fps
        }
        .to_string(),
    ));
    ui::named(&fps, "Frames per second");
    options.pack_start(&option_field("FPS", 76, &fps), false, false, 0);
    let resolution = gtk::ComboBoxText::new();
    for (id, text) in [
        ("original", "Original"),
        ("p1080", "1080p"),
        ("p720", "720p"),
    ] {
        resolution.append(Some(id), text);
    }
    resolution.set_active_id(Some(
        match settings.borrow().recording.video_max_resolution {
            captures_recording::MaxResolution::Original => "original",
            captures_recording::MaxResolution::P1080 => "p1080",
            captures_recording::MaxResolution::P720 => "p720",
        },
    ));
    ui::named(&resolution, "Maximum resolution");
    options.pack_start(
        &option_field("MAX RESOLUTION", 132, &resolution),
        false,
        false,
        0,
    );
    let (cursor_switch, cursor_row) =
        option_switch("Show cursor", settings.borrow().recording.show_cursor);
    let (clicks, clicks_row) =
        option_switch("Show clicks", settings.borrow().recording.highlight_clicks);
    let pointer_available = captures_recording_xcap::pointer_features_available();
    cursor_switch.set_sensitive(pointer_available);
    clicks.set_sensitive(pointer_available);
    options.pack_start(
        &option_field("SHOW CURSOR", 92, &cursor_row),
        false,
        false,
        0,
    );
    options.pack_start(
        &option_field("SHOW CLICKS", 92, &clicks_row),
        false,
        false,
        0,
    );
    let (audio, audio_row) = option_switch(
        "Desktop audio",
        settings.borrow().recording.capture_system_audio,
    );
    options.pack_start(
        &option_field("DESKTOP AUDIO", 118, &audio_row),
        false,
        false,
        0,
    );
    let microphone = gtk::ComboBoxText::new();
    microphone.append(Some("off"), "Off");
    if let Some(id) = settings.borrow().recording.microphone_device_id.as_deref() {
        microphone.append(Some(id), "Selected microphone");
        microphone.set_active_id(Some(id));
    } else {
        microphone.set_active(Some(0));
    }
    ui::named(&microphone, "Microphone");
    options.pack_start(&option_field("MICROPHONE", 180, &microphone), true, true, 0);
    controls.add(&options);
    controls.add(&status);
    overlay.add_overlay(&panel);
    let panel_drag = Rc::new(Cell::new(None::<(f64, f64, i32, i32)>));
    {
        let drag = panel_drag.clone();
        panel.connect_button_press_event(move |panel, event| {
            if event.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (x, y) = event.root();
            let allocation = panel.allocation();
            drag.set(Some((x, y, allocation.x(), allocation.y())));
            glib::Propagation::Stop
        });
    }
    {
        let drag = panel_drag.clone();
        panel.connect_motion_notify_event(move |panel, event| {
            let Some((sx, sy, x, y)) = drag.get() else {
                return glib::Propagation::Proceed;
            };
            let (px, py) = event.root();
            panel.set_halign(gtk::Align::Start);
            panel.set_valign(gtk::Align::Start);
            panel.set_margin_bottom(0);
            panel.set_margin_start(
                (x + (px - sx) as i32)
                    .clamp(8, (width as i32 - panel.allocated_width() - 8).max(8)),
            );
            panel.set_margin_top(
                (y + (py - sy) as i32)
                    .clamp(8, (height as i32 - panel.allocated_height() - 8).max(8)),
            );
            glib::Propagation::Stop
        });
    }
    panel.connect_button_release_event(move |_, _| {
        panel_drag.set(None);
        glib::Propagation::Stop
    });
    let guidance = gtk::Box::new(gtk::Orientation::Vertical, 4);
    guidance.style_context().add_class("capture-guidance");
    guidance.set_halign(gtk::Align::Center);
    guidance.set_valign(gtk::Align::Start);
    guidance.set_margin_top((height * 0.16) as i32);
    let guidance_title = ui::label("Drag to select a region", "section-title");
    guidance_title.set_halign(gtk::Align::Center);
    let guidance_hint = ui::label("Shift for square · Esc to cancel", "muted");
    guidance_hint.set_halign(gtk::Align::Center);
    guidance.add(&guidance_title);
    guidance.add(&guidance_hint);
    overlay.add_overlay(&guidance);
    overlay.set_overlay_pass_through(&guidance, true);
    let identity = gtk::Box::new(gtk::Orientation::Vertical, 8);
    identity.style_context().add_class("capture-display");
    identity.set_halign(gtk::Align::Center);
    identity.set_valign(gtk::Align::Center);
    identity.set_margin_bottom(24);
    let display_icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    display_icon
        .style_context()
        .add_class("capture-display-icon");
    display_icon.set_halign(gtk::Align::Center);
    display_icon.add(&ui::icon("display", 34));
    identity.add(&display_icon);
    let display_name = ui::label(&frame.descriptor.name, "title");
    display_name.set_halign(gtk::Align::Center);
    identity.add(&display_name);
    let display_size = ui::label(
        &format!("{} × {}", frame.descriptor.width, frame.descriptor.height),
        "muted",
    );
    display_size.set_halign(gtk::Align::Center);
    identity.add(&display_size);
    {
        let dimensions = format!("{} × {}", frame.descriptor.width, frame.descriptor.height);
        let (record_state, fps_value) = (record.clone(), fps.clone());
        let update = Rc::new(move || {
            let suffix = if record_state.is_active() {
                format!(" · {} FPS", fps_value.active_id().unwrap_or_default())
            } else {
                String::new()
            };
            display_size.set_text(&format!("{dimensions}{suffix}"));
        });
        update();
        let on_mode = update.clone();
        record.connect_toggled(move |_| on_mode());
        fps.connect_changed(move |_| update());
    }
    overlay.add_overlay(&identity);
    overlay.set_overlay_pass_through(&identity, true);
    window.add(&overlay);
    let full = LogicalRect {
        x: 0.,
        y: 0.,
        width,
        height,
    };
    let rect = Rc::new(RefCell::new(
        (initial_mode == CaptureMode::Display).then_some(full),
    ));
    {
        let (rect, area) = (rect.clone(), area.clone());
        ratio.connect_changed(move |combo| {
            if let Some(aspect) = selection_aspect(combo.active()) {
                let mut rect = rect.borrow_mut();
                if let Some(r) = *rect {
                    *rect = Some(fit_selection_aspect(r, aspect, width, height));
                    area.queue_draw();
                }
            }
        });
    }
    let drag = Rc::new(Cell::new(None::<Drag>));
    let drag_pointer = Rc::new(Cell::new(None::<(f64, f64)>));
    let selected = Rc::new(RefCell::new(None::<WindowDescriptor>));
    let locked = Rc::new(Cell::new(false));
    let finished = Rc::new(Cell::new(false));
    let buttons: Rc<RefCell<Vec<(CaptureMode, gtk::ToggleButton)>>> = Rc::new(RefCell::new(vec![]));
    for (value, name) in [
        (CaptureMode::Region, "Region"),
        (CaptureMode::Window, "Window"),
        (CaptureMode::Display, "Full screen"),
    ] {
        let button = gtk::ToggleButton::with_label(name);
        button.set_image(Some(&ui::icon(
            match value {
                CaptureMode::Region => "region",
                CaptureMode::Window => "window",
                CaptureMode::Display => "display",
            },
            16,
        )));
        button.set_always_show_image(true);
        button.set_active(value == initial_mode);
        {
            let (
                mode,
                rect,
                selected,
                locked,
                area,
                accept,
                buttons,
                aspect,
                monitor,
                guidance_title,
                guidance_hint,
                guidance,
                identity,
            ) = (
                mode.clone(),
                rect.clone(),
                selected.clone(),
                locked.clone(),
                area.clone(),
                accept.clone(),
                buttons.clone(),
                aspect.clone(),
                monitor.clone(),
                guidance_title.clone(),
                guidance_hint.clone(),
                guidance.clone(),
                identity.clone(),
            );
            button.connect_toggled(move |clicked| {
                if !clicked.is_active() {
                    if mode.get() == value {
                        clicked.set_active(true);
                    }
                    return;
                }
                if mode.get() == value {
                    return;
                }
                mode.set(value);
                *rect.borrow_mut() = (value == CaptureMode::Display).then_some(full);
                *selected.borrow_mut() = None;
                locked.set(false);
                accept.set_sensitive(value == CaptureMode::Display);
                for (m, b) in buttons.borrow().iter() {
                    b.set_active(*m == value);
                }
                aspect.set_visible(value == CaptureMode::Region);
                monitor.set_visible(value == CaptureMode::Display);
                guidance.set_visible(value != CaptureMode::Display);
                identity.set_visible(value == CaptureMode::Display);
                guidance_title.set_text(match value {
                    CaptureMode::Region => "Drag to select a region",
                    CaptureMode::Window => "Select a window to continue",
                    CaptureMode::Display => "Click to capture this display",
                });
                guidance_hint.set_text(if value == CaptureMode::Region {
                    "Shift for square · Esc to cancel"
                } else {
                    "Esc to cancel"
                });
                area.queue_draw();
            });
        }
        targets.add(&button);
        buttons.borrow_mut().push((value, button));
    }
    accept.set_sensitive(initial_mode == CaptureMode::Display);
    for (button, other, is_recording) in
        [(&screenshot, &record, false), (&record, &screenshot, true)]
    {
        let (kind, accept, options, status, settings, other) = (
            kind.clone(),
            accept.clone(),
            options.clone(),
            status.clone(),
            settings.clone(),
            other.clone(),
        );
        button.connect_toggled(move |button| {
            if !button.is_active() {
                if (kind.get() != 0) == is_recording {
                    button.set_active(true);
                }
                return;
            }
            if (kind.get() != 0) == is_recording {
                return;
            }
            kind.set(if is_recording { initial_kind.max(1) } else { 0 });
            other.set_active(false);
            accept.set_label(if is_recording { "Record" } else { "Capture" });
            ui::named(
                &accept,
                if is_recording {
                    "Start recording"
                } else {
                    "Capture"
                },
            );
            let action_icon = ui::icon(
                if is_recording {
                    "record-dot"
                } else {
                    "capture"
                },
                16,
            );
            if is_recording {
                action_icon.style_context().add_class("capture-record-dot");
            }
            action_icon.show();
            accept.set_image(Some(&action_icon));
            options.set_visible(is_recording);
            status.set_text(&selector_note(
                is_recording,
                settings.borrow().auto_start_on_selection,
            ));
        });
    }
    {
        let settings = settings.clone();
        audio.connect_active_notify(move |c| {
            settings.borrow_mut().recording.capture_system_audio = c.is_active()
        });
    }
    {
        let (settings, clicks) = (settings.clone(), clicks.clone());
        cursor_switch.connect_active_notify(move |c| {
            settings.borrow_mut().recording.show_cursor = c.is_active();
            if !c.is_active() {
                clicks.set_active(false);
            }
        });
    }
    {
        let (settings, cursor_switch) = (settings.clone(), cursor_switch.clone());
        clicks.connect_active_notify(move |c| {
            settings.borrow_mut().recording.highlight_clicks = c.is_active();
            if c.is_active() {
                cursor_switch.set_active(true);
            }
        });
    }
    {
        let (settings, kind) = (settings.clone(), kind.clone());
        fps.connect_changed(move |c| {
            if let Some(value) = c.active_id().and_then(|v| v.parse().ok()) {
                if kind.get() == 2 {
                    settings.borrow_mut().recording.gif_fps = value;
                } else {
                    settings.borrow_mut().recording.video_fps = value;
                }
            }
        });
    }
    {
        let settings = settings.clone();
        resolution.connect_changed(move |c| {
            settings.borrow_mut().recording.video_max_resolution = match c.active_id().as_deref() {
                Some("p1080") => captures_recording::MaxResolution::P1080,
                Some("p720") => captures_recording::MaxResolution::P720,
                _ => captures_recording::MaxResolution::Original,
            };
        });
    }
    {
        let settings = settings.clone();
        microphone.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                settings.borrow_mut().recording.microphone_device_id =
                    (id != "off").then(|| id.to_string());
            }
        });
    }
    {
        let loaded = Cell::new(false);
        microphone.connect_popup(move |microphone| {
            if loaded.replace(true) {
                return;
            }
            let microphone = microphone.clone();
            ui::job(
                || Ok(captures_recording_xcap::microphone_devices()),
                move |result| {
                    if let Ok(devices) = result {
                        let current = microphone.active_id();
                        microphone.remove_all();
                        microphone.append(Some("off"), "Off");
                        for device in &devices {
                            microphone.append(Some(&device.id), &device.name);
                        }
                        if let Some(id) = current
                            .as_deref()
                            .filter(|id| *id != "off" && !devices.iter().any(|d| d.id == *id))
                        {
                            microphone.append(Some(id), "Selected microphone");
                        }
                        microphone.set_active_id(current.as_deref().or(Some("off")));
                    }
                },
            );
        });
    }
    let background = ui::pixbuf(&frame.image);
    let frame = Rc::new(frame);
    {
        let (rect, settings, mode, selected) = (
            rect.clone(),
            settings.clone(),
            mode.clone(),
            selected.clone(),
        );
        area.connect_draw(move |_, cr| {
            cr.set_operator(gtk::cairo::Operator::Source);
            cr.set_source_rgba(0., 0., 0., 0.);
            let _ = cr.paint();
            cr.set_operator(gtk::cairo::Operator::Over);
            if settings.borrow().freeze_screen {
                let _ = cr.save();
                cr.scale(
                    width / background.width() as f64,
                    height / background.height() as f64,
                );
                cr.set_source_pixbuf(&background, 0., 0.);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
            cr.rectangle(0., 0., width, height);
            if mode.get() != CaptureMode::Display
                && let Some(r) = *rect.borrow()
            {
                cr.rectangle(r.x, r.y, r.width, r.height);
            }
            cr.set_source_rgba(
                4. / 255.,
                5. / 255.,
                8. / 255.,
                match mode.get() {
                    CaptureMode::Region => 0.38,
                    CaptureMode::Window => 0.30,
                    CaptureMode::Display => 0.50,
                },
            );
            let _ = cr.fill();
            if let Some(r) = *rect.borrow() {
                let color = ui::color("accent");
                if mode.get() == CaptureMode::Window {
                    cr.set_source_rgba(
                        f64::from(color.red()),
                        f64::from(color.green()),
                        f64::from(color.blue()),
                        0.14,
                    );
                    cr.rectangle(r.x, r.y, r.width, r.height);
                    let _ = cr.fill();
                }
                cr.set_source_rgba(
                    f64::from(color.red()),
                    f64::from(color.green()),
                    f64::from(color.blue()),
                    1.,
                );
                cr.set_line_width(1.5);
                cr.rectangle(r.x, r.y, r.width, r.height);
                let _ = cr.stroke();
                if mode.get() == CaptureMode::Region {
                    for (x, y) in [
                        (r.x, r.y),
                        (r.x + r.width, r.y),
                        (r.x, r.y + r.height),
                        (r.x + r.width, r.y + r.height),
                    ] {
                        cr.set_source_rgba(
                            f64::from(color.red()),
                            f64::from(color.green()),
                            f64::from(color.blue()),
                            1.,
                        );
                        cr.arc(x, y, 4., 0., std::f64::consts::TAU);
                        let _ = cr.fill_preserve();
                        let ink = ui::color("theme-accent-ink");
                        cr.set_source_rgba(
                            f64::from(ink.red()),
                            f64::from(ink.green()),
                            f64::from(ink.blue()),
                            1.,
                        );
                        cr.set_line_width(2.);
                        let _ = cr.stroke();
                    }
                    let text = format!("{} × {}", r.width.round(), r.height.round());
                    badge(
                        cr,
                        &text,
                        r.x + r.width / 2.,
                        if r.y > 32. { r.y - 30. } else { r.y + 6. },
                        "accent",
                        "theme-accent-ink",
                    );
                } else if mode.get() == CaptureMode::Window
                    && let Some(w) = selected.borrow().as_ref()
                {
                    badge(
                        cr,
                        &w.title,
                        r.x + r.width / 2.,
                        r.y + 4.,
                        "glass-strong",
                        "glass-text",
                    );
                }
            }
            glib::Propagation::Proceed
        });
    }
    {
        let (mode, rect, drag, selected, locked, frame, guidance, accept, ratio) = (
            mode.clone(),
            rect.clone(),
            drag.clone(),
            selected.clone(),
            locked.clone(),
            frame.clone(),
            guidance.clone(),
            accept.clone(),
            ratio.clone(),
        );
        let drag_pointer = drag_pointer.clone();
        area.connect_motion_notify_event(move |area, event| {
            let (x, y) = event.position();
            drag_pointer.set(Some((x, y)));
            if mode.get() == CaptureMode::Region {
                if let Some(drag) = drag.get() {
                    let aspect = if event.state().contains(gdk::ModifierType::SHIFT_MASK) {
                        Some(1.)
                    } else {
                        selection_aspect(ratio.active())
                    };
                    *rect.borrow_mut() = Some(drag_rect(drag, x, y, width, height, aspect));
                }
            } else if mode.get() == CaptureMode::Window && !locked.get() {
                let (gx, gy) = overlay_point_to_physical(&frame.descriptor, x, y);
                *selected.borrow_mut() = windows
                    .iter()
                    .find(|w| {
                        gx >= w.x as f64
                            && gy >= w.y as f64
                            && gx < (w.x as f64 + w.width as f64)
                            && gy < (w.y as f64 + w.height as f64)
                    })
                    .cloned();
                *rect.borrow_mut() = selected
                    .borrow()
                    .as_ref()
                    .map(|w| physical_window_to_overlay(&frame.descriptor, w));
            }
            if let Some(r) = *rect.borrow() {
                accept.set_sensitive(r.width >= 2. && r.height >= 2.);
            } else {
                accept.set_sensitive(false);
            }
            let bounds = guidance.allocation();
            let covered = x >= bounds.x() as f64
                && x <= (bounds.x() + bounds.width()) as f64
                && y >= bounds.y() as f64
                && y <= (bounds.y() + bounds.height()) as f64;
            guidance.set_opacity(if covered || drag.get().is_some() {
                0.
            } else {
                1.
            });
            area.queue_draw();
            glib::Propagation::Proceed
        });
    }
    {
        let (mode, rect, drag, locked) = (mode.clone(), rect.clone(), drag.clone(), locked.clone());
        area.connect_button_press_event(move |_, event| {
            if event.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (x, y) = event.position();
            if mode.get() == CaptureMode::Window {
                locked.set(!locked.get());
            } else if mode.get() == CaptureMode::Region {
                let mut action = Drag::New(x, y);
                if let Some(r) = *rect.borrow() {
                    if let Some((corner, _)) = [
                        (r.x, r.y),
                        (r.x + r.width, r.y),
                        (r.x, r.y + r.height),
                        (r.x + r.width, r.y + r.height),
                    ]
                    .iter()
                    .enumerate()
                    .find(|(_, p)| (p.0 - x).abs() <= 9. && (p.1 - y).abs() <= 9.)
                    {
                        action = Drag::Resize(r, corner);
                    } else if x > r.x && x < r.x + r.width && y > r.y && y < r.y + r.height {
                        action = Drag::Move(r, x, y);
                    }
                }
                drag.set(Some(action));
            }
            glib::Propagation::Stop
        });
    }
    {
        let (drag, settings, accept, guidance) = (
            drag.clone(),
            settings.clone(),
            accept.clone(),
            guidance.clone(),
        );
        area.connect_button_release_event(move |_, _| {
            drag.set(None);
            guidance.set_opacity(1.);
            if settings.borrow().auto_start_on_selection && accept.is_sensitive() {
                accept.clicked();
            }
            glib::Propagation::Stop
        });
    }
    {
        let (window, finished, mode, kind, settings, rect, selected, frame) = (
            window.clone(),
            finished.clone(),
            mode.clone(),
            kind.clone(),
            settings.clone(),
            rect.clone(),
            selected.clone(),
            frame.clone(),
        );
        let cancelled = cancelled.clone();
        let done = done.clone();
        accept.connect_clicked(move |_| {
            let Some(r) = *rect.borrow() else {
                return;
            };
            if r.width < 2. || r.height < 2. || finished.replace(true) {
                return;
            }
            let target = match mode.get() {
                CaptureMode::Display => RecordingTarget::Display {
                    display_id: frame.descriptor.id.clone(),
                },
                CaptureMode::Region => RecordingTarget::Region {
                    display_id: frame.descriptor.id.clone(),
                    rect: CaptureRect {
                        x: r.x.round() as i32,
                        y: r.y.round() as i32,
                        width: r.width.round() as u32,
                        height: r.height.round() as u32,
                    },
                },
                CaptureMode::Window => {
                    let Some(w) = selected.borrow().clone() else {
                        finished.set(false);
                        return;
                    };
                    RecordingTarget::Window { window_id: w.id }
                }
            };
            let config = settings.borrow().clone();
            let kind = kind.get();
            let current_pointer = if config.freeze_screen {
                cursor
            } else {
                pointer()
            };
            let image = match crop(
                &frame,
                r,
                (config.show_cursor_in_screenshots && kind == 0).then_some(PointerCursor {
                    position: current_pointer,
                    image: None,
                }),
            ) {
                Ok(image) => image,
                Err(error) => {
                    finished.set(false);
                    ui::error(&window, &error);
                    return;
                }
            };
            let selection = Selection {
                display: frame.descriptor.clone(),
                target,
                image,
                kind,
                settings: config.clone(),
                pointer: current_pointer,
                rect: r,
                window: selected.borrow().clone(),
            };
            window.hide();
            let window = window.clone();
            let done = done.clone();
            let cancelled = cancelled.clone();
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                ui::job(
                    move || {
                        if !captures_session::capture_session_available() {
                            return Err("Capture cancelled: desktop session unavailable".into());
                        }
                        let mut selection = selection;
                        if !config.freeze_screen && kind == 0 {
                            selection.image =
                                refresh(&selection, config.show_cursor_in_screenshots)?;
                        }
                        Ok(selection)
                    },
                    move |result| {
                        window.close();
                        match result {
                            Ok(selection) => done(selection),
                            Err(error) => {
                                cancelled();
                                ui::error(&window, &error);
                            }
                        }
                    },
                )
            });
        });
    }
    {
        let (window, finished, mode, kind, settings) = (
            window.clone(),
            finished.clone(),
            mode.clone(),
            kind.clone(),
            settings.clone(),
        );
        let cancelled = cancelled.clone();
        monitor.connect_changed(move |c| {
            let Some(id) = c.active_id() else {
                return;
            };
            if finished.replace(true) {
                return;
            }
            window.hide();
            let (mode, kind, settings) = (mode.get(), kind.get(), settings.borrow().clone());
            let (done, cancelled, window) = (done.clone(), cancelled.clone(), window.clone());
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                window.close();
                select_display(
                    mode,
                    kind,
                    settings,
                    Some(id.to_string()),
                    screenshot_only,
                    done,
                    cancelled,
                );
            });
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    // React reapplies the current gesture on Shift itself, without requiring
    // another pointer move. Retain the unconstrained pointer for key release.
    let shift_drag = Rc::new(move |square: bool| {
        if let (Some(drag), Some((x, y))) = (drag.get(), drag_pointer.get()) {
            let aspect = if square {
                Some(1.)
            } else {
                selection_aspect(ratio.active())
            };
            *rect.borrow_mut() = Some(drag_rect(drag, x, y, width, height, aspect));
            area.queue_draw();
        }
    });
    {
        let (accept, buttons) = (accept.clone(), buttons.clone());
        let shift_drag = shift_drag.clone();
        window.connect_key_press_event(move |window, event| {
            match event.keyval() {
                gdk::Key::Shift_L | gdk::Key::Shift_R => shift_drag(true),
                gdk::Key::Escape => window.close(),
                gdk::Key::Return => {
                    if accept.is_sensitive() {
                        accept.clicked();
                    }
                }
                gdk::Key::r => buttons.borrow()[0].1.clicked(),
                gdk::Key::w => buttons.borrow()[1].1.clicked(),
                gdk::Key::f => buttons.borrow()[2].1.clicked(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.connect_key_release_event(move |_, event| {
        if matches!(event.keyval(), gdk::Key::Shift_L | gdk::Key::Shift_R) {
            shift_drag(false);
        }
        glib::Propagation::Proceed
    });
    // The selector owns cancellation until it hands off a confirmed selection.
    let cancel_on_close = cancelled;
    window.connect_close_request(move |_| {
        if !finished.replace(true) {
            cancel_on_close();
        }
        glib::Propagation::Proceed
    });
    window.show_all();
    #[cfg(target_os = "windows")]
    if let Err(error) = crate::windows::place_window(
        &window,
        frame.descriptor.x,
        frame.descriptor.y,
        frame.descriptor.width as i32,
        frame.descriptor.height as i32,
    ) {
        window.close();
        ui::error(&gtk::Window::new(), &error);
        return;
    }
    aspect.set_visible(initial_mode == CaptureMode::Region);
    monitor.set_visible(initial_mode == CaptureMode::Display);
    options.set_visible(initial_kind != 0);
    guidance.set_visible(initial_mode != CaptureMode::Display);
    identity.set_visible(initial_mode == CaptureMode::Display);
    status.set_text(&selector_note(
        initial_kind != 0,
        settings.borrow().auto_start_on_selection,
    ));
    guidance_title.set_text(match initial_mode {
        CaptureMode::Region => "Drag to select a region",
        CaptureMode::Window => "Select a window to continue",
        CaptureMode::Display => "Click to capture this display",
    });
    guidance_hint.set_text(if initial_mode == CaptureMode::Region {
        "Shift for square · Esc to cancel"
    } else {
        "Esc to cancel"
    });
    window.present();
}

fn selection_aspect(index: Option<u32>) -> Option<f64> {
    match index {
        Some(1) => Some(1.),
        Some(2) => Some(4. / 3.),
        Some(3) => Some(3. / 2.),
        Some(4) => Some(16. / 9.),
        Some(5) => Some(9. / 16.),
        _ => None,
    }
}

// Same centered, inward fit as selection.ts constrainSelectionToAspect.
fn fit_selection_aspect(r: LogicalRect, ratio: f64, width: f64, height: f64) -> LogicalRect {
    let mut w = r.width.min(r.height * ratio);
    if w < 16. || w / ratio < 16. {
        w = w.max(if ratio >= 1. { 16. } else { 16. * ratio });
    }
    let (x, y) = (r.x + (r.width - w) / 2., r.y + (r.height - w / ratio) / 2.);
    w = w.min(width).min(height * ratio);
    let h = w / ratio;
    LogicalRect {
        x: x.clamp(0., width - w),
        y: y.clamp(0., height - h),
        width: w,
        height: h,
    }
}

fn option_field(name: &str, width: i32, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let field = gtk::Box::new(gtk::Orientation::Vertical, 6);
    field.set_size_request(width, -1);
    field.pack_start(&ui::label(name, "capture-field-label"), false, false, 0);
    field.pack_start(control, false, false, 0);
    field
}

fn option_switch(name: &str, active: bool) -> (gtk::Switch, gtk::Box) {
    let control = gtk::Switch::new();
    control.set_active(active);
    control.set_valign(gtk::Align::Center);
    ui::named(&control, name);
    let label = ui::label(if active { "On" } else { "Off" }, "muted");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_size_request(-1, 36);
    row.pack_start(&control, false, false, 0);
    row.pack_start(&label, false, false, 0);
    control
        .connect_active_notify(move |c| label.set_text(if c.is_active() { "On" } else { "Off" }));
    (control, row)
}

fn selector_note(recording: bool, auto: bool) -> String {
    format!(
        "{} · {}",
        if recording {
            "Controls may appear in recordings"
        } else {
            "Controls are hidden from screenshots"
        },
        if auto {
            "Auto-capture is on. Selecting a target starts immediately."
        } else {
            "Press Enter to confirm"
        }
    )
}

fn badge(
    cr: &gtk::cairo::Context,
    text: &str,
    center: f64,
    y: f64,
    background: &str,
    foreground: &str,
) {
    cr.select_font_face(
        "Arial",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Bold,
    );
    cr.set_font_size(11.);
    let Ok(extents) = cr.text_extents(text) else {
        return;
    };
    let width = extents.width() + 16.;
    let x = (center - width / 2.).max(4.);
    let c = ui::color(background);
    cr.set_source_rgba(
        f64::from(c.red()),
        f64::from(c.green()),
        f64::from(c.blue()),
        f64::from(c.alpha()),
    );
    cr.new_sub_path();
    for (cx, cy, start) in [
        (x + width - 6., y + 6., -std::f64::consts::FRAC_PI_2),
        (x + width - 6., y + 16., 0.),
        (x + 6., y + 16., std::f64::consts::FRAC_PI_2),
        (x + 6., y + 6., std::f64::consts::PI),
    ] {
        cr.arc(cx, cy, 6., start, start + std::f64::consts::FRAC_PI_2);
    }
    cr.close_path();
    let _ = cr.fill();
    let c = ui::color(foreground);
    cr.set_source_rgba(
        f64::from(c.red()),
        f64::from(c.green()),
        f64::from(c.blue()),
        f64::from(c.alpha()),
    );
    cr.move_to(x + 8., y + 15.);
    let _ = cr.show_text(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: i32, y: i32, width: u32, height: u32, scale_factor: f64) -> DisplayDescriptor {
        DisplayDescriptor {
            id: "display".into(),
            name: "Display".into(),
            x,
            y,
            width,
            height,
            scale_factor,
            is_primary: false,
        }
    }

    #[test]
    fn move_clamps_to_display_without_resizing() {
        let r = LogicalRect {
            x: 60.,
            y: 20.,
            width: 80.,
            height: 30.,
        };
        let out = drag_rect(Drag::Move(r, 70., 25.), 199., 99., 200., 100., None);
        assert_eq!((out.x, out.y, out.width, out.height), (120., 70., 80., 30.));
    }
    #[test]
    fn reversed_aspect_drag_and_corner_resize_keep_opposite_anchor() {
        let out = drag_rect(Drag::New(140., 90.), 20., 10., 200., 100., Some(2.));
        // Tauri follows the longer delta, then clamps 160x80 to the left edge.
        assert_eq!((out.x, out.y, out.width, out.height), (0., 20., 140., 70.));
        let r = LogicalRect {
            x: 20.,
            y: 10.,
            width: 80.,
            height: 40.,
        };
        let out = drag_rect(Drag::Resize(r, 0), 5., 3., 200., 100., None);
        assert_eq!((out.x, out.y, out.width, out.height), (5., 3., 95., 47.));
    }

    #[test]
    fn aspect_preset_refits_inward_while_drawing_follows_longer_delta() {
        let r = LogicalRect {
            x: 100.,
            y: 50.,
            width: 320.,
            height: 180.,
        };
        let square = fit_selection_aspect(r, 1., 1440., 900.);
        assert_eq!(
            (square.x, square.y, square.width, square.height),
            (170., 50., 180., 180.)
        );
        let wide = fit_selection_aspect(
            LogicalRect {
                x: 200.,
                y: 100.,
                width: 400.,
                height: 400.,
            },
            16. / 9.,
            1440.,
            900.,
        );
        assert_eq!(
            (wide.x, wide.y, wide.width, wide.height),
            (200., 187.5, 400., 225.)
        );
        let drawn = drag_rect(Drag::New(100., 50.), 420., 230., 1440., 900., Some(1.));
        assert_eq!((drawn.width, drawn.height), (320., 320.));
        assert_eq!(selection_aspect(Some(3)), Some(1.5));
    }

    #[test]
    fn windows_negative_origin_and_gtk_scale_map_to_physical_desktop() {
        let display = display(-2560, 180, 2560, 1440, 2.0);
        assert_eq!(
            overlay_point_to_global(&display, 125.0, 70.0, true),
            (-2310.0, 320.0)
        );
    }

    #[test]
    fn windows_asymmetric_window_bounds_map_back_to_overlay_space() {
        let display = display(-2560, 180, 2560, 1440, 2.0);
        let window = WindowDescriptor {
            id: "window".into(),
            title: "Window".into(),
            app_name: None,
            z_order: 0,
            x: -2410,
            y: 260,
            width: 901,
            height: 603,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        let rect = window_to_overlay(&display, &window, true);
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (75.0, 40.0, 450.5, 301.5)
        );
    }

    #[test]
    fn refreshed_crop_uses_preserved_gtk_scale() {
        let display = display(-2560, 180, 2560, 1440, 2.0);
        let scale = DisplayDescriptor::overlay_to_buffer_scale_for(
            display.width,
            display.height,
            display.scale_factor,
            2560,
            1440,
            true,
        );
        let crop = LogicalRect {
            x: 75.0,
            y: 40.0,
            width: 450.5,
            height: 301.5,
        }
        .to_physical(scale, 2560, 1440);
        assert_eq!(
            (crop.x, crop.y, crop.width, crop.height),
            (150, 80, 901, 603)
        );
    }
}
