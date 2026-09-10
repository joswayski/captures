//! Native frozen-screen target selector. No webview or browser coordinates.
use crate::ui;
use captures_capture::{
    CaptureMode, DisplayDescriptor, DisplayFrame, LogicalRect, WindowDescriptor, XcapBackend,
};
use captures_recording::{CaptureRect, RecordingTarget};
use gtk::{gdk, glib, prelude::*};
use image::RgbaImage;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct Selection {
    pub display: DisplayDescriptor,
    pub target: RecordingTarget,
    pub image: RgbaImage,
}

pub fn select(mode: CaptureMode, done: Rc<dyn Fn(Selection)>, cancelled: Rc<dyn Fn()>) {
    ui::job(
        move || {
            if !captures_session::capture_session_available() {
                return Err("Capture is unavailable: the desktop is locked, inactive, or its session state cannot be verified.".into());
            }
            let backend = XcapBackend;
            let frame = backend
                .capture_display_at_point(None)
                .map_err(|e| e.to_string())?;
            let windows = if mode == CaptureMode::Window {
                backend.windows().map_err(|e| e.to_string())?
            } else {
                vec![]
            };
            Ok((frame, windows))
        },
        move |result| match result {
            Ok((frame, windows)) => overlay(mode, frame, windows, done.clone(), cancelled.clone()),
            Err(error) => {
                cancelled();
                let window = gtk::Window::new(gtk::WindowType::Toplevel);
                ui::error(&window, &error);
                window.close();
            }
        },
    );
}

fn overlay(
    mode: CaptureMode,
    frame: DisplayFrame,
    windows: Vec<WindowDescriptor>,
    done: Rc<dyn Fn(Selection)>,
    cancelled: Rc<dyn Fn()>,
) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — Select target");
    window.set_decorated(false);
    window.set_keep_above(true);
    let (ox, oy, width, height) = frame.descriptor.overlay_geometry();
    window.move_(ox as i32, oy as i32);
    window.set_default_size(width as i32, height as i32);
    window.set_resizable(false);
    let overlay = gtk::Overlay::new();
    let area = gtk::DrawingArea::new();
    area.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK,
    );
    overlay.add(&area);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    controls.style_context().add_class("glass");
    controls.set_border_width(16);
    controls.set_halign(gtk::Align::Center);
    controls.set_valign(gtk::Align::End);
    controls.set_margin_bottom(28);
    let status = ui::label(
        match mode {
            CaptureMode::Region => "Drag a region · Shift for square · Enter to confirm",
            CaptureMode::Window => "Point at a window · Click to select · Enter to confirm",
            CaptureMode::Display => "Full display · Enter to confirm",
        },
        "",
    );
    let accept = ui::button("Confirm");
    accept.style_context().add_class("primary");
    let cancel = ui::button("Cancel · Esc");
    controls.pack_start(&status, false, false, 0);
    controls.pack_start(&accept, false, false, 0);
    controls.pack_start(&cancel, false, false, 0);
    overlay.add_overlay(&controls);
    window.add(&overlay);
    let initial = (mode == CaptureMode::Display).then_some(LogicalRect {
        x: 0.,
        y: 0.,
        width,
        height,
    });
    let rect = Rc::new(RefCell::new(initial));
    let start = Rc::new(Cell::new(None::<(f64, f64)>));
    let selected_window = Rc::new(RefCell::new(None::<WindowDescriptor>));
    let finished = Rc::new(Cell::new(false));
    let background = ui::pixbuf(&frame.image);
    let frame = Rc::new(frame);
    accept.set_sensitive(initial.is_some());
    {
        let rect = rect.clone();
        area.connect_draw(move |_, cr| {
            let _ = cr.save();
            cr.scale(
                width / background.width() as f64,
                height / background.height() as f64,
            );
            cr.set_source_pixbuf(&background, 0., 0.);
            let _ = cr.paint();
            let _ = cr.restore();
            cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
            cr.rectangle(0., 0., width, height);
            if let Some(r) = *rect.borrow() {
                cr.rectangle(r.x, r.y, r.width, r.height);
            }
            cr.set_source_rgba(0., 0., 0., 0.48);
            let _ = cr.fill();
            if let Some(r) = *rect.borrow() {
                let accent = ui::color("theme-accent");
                cr.set_source_rgba(accent.red(), accent.green(), accent.blue(), 1.);
                cr.set_line_width(2.);
                cr.rectangle(r.x, r.y, r.width, r.height);
                let _ = cr.stroke();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let (rect, start, picked, frame, status, accept) = (
            rect.clone(),
            start.clone(),
            selected_window.clone(),
            frame.clone(),
            status.clone(),
            accept.clone(),
        );
        area.connect_motion_notify_event(move |area, event| {
            let (x, y) = event.position();
            if mode == CaptureMode::Region {
                if let Some((sx, sy)) = start.get() {
                    let mut dx = x - sx;
                    let mut dy = y - sy;
                    if event.state().contains(gdk::ModifierType::SHIFT_MASK) {
                        let side = dx.abs().min(dy.abs());
                        dx = side * dx.signum();
                        dy = side * dy.signum();
                    }
                    *rect.borrow_mut() = Some(
                        LogicalRect {
                            x: sx,
                            y: sy,
                            width: dx,
                            height: dy,
                        }
                        .normalized(),
                    );
                }
            } else if mode == CaptureMode::Window && start.get().is_none() {
                *picked.borrow_mut() = windows
                    .iter()
                    .find(|w| {
                        let gx = x + frame.descriptor.x as f64;
                        let gy = y + frame.descriptor.y as f64;
                        gx >= w.x as f64
                            && gy >= w.y as f64
                            && gx < w.x as f64 + w.width as f64
                            && gy < w.y as f64 + w.height as f64
                    })
                    .cloned();
                *rect.borrow_mut() = picked.borrow().as_ref().map(|w| LogicalRect {
                    x: (w.x - frame.descriptor.x) as f64,
                    y: (w.y - frame.descriptor.y) as f64,
                    width: w.width as f64,
                    height: w.height as f64,
                });
            }
            if let Some(r) = *rect.borrow() {
                status.set_text(&format!(
                    "{} × {} · Enter to confirm",
                    r.width.round(),
                    r.height.round()
                ));
                accept.set_sensitive(r.width >= 2. && r.height >= 2.);
            }
            area.queue_draw();
            glib::Propagation::Proceed
        });
    }
    {
        let press_start = start.clone();
        area.connect_button_press_event(move |_, e| {
            if e.button() == 1 {
                press_start.set(Some(e.position()));
            }
            glib::Propagation::Stop
        });
        let start = start.clone();
        area.connect_button_release_event(move |_, _| {
            if mode == CaptureMode::Region {
                start.set(None);
            }
            glib::Propagation::Stop
        });
    }
    {
        let (window, finished) = (window.clone(), finished.clone());
        let cancelled = cancelled.clone();
        accept.connect_clicked(move |_| {
            let Some(r) = *rect.borrow() else {
                return;
            };
            if r.width < 2. || r.height < 2. || finished.replace(true) {
                return;
            }
            let frame = frame.clone();
            let target = match mode {
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
                    let picked = selected_window.borrow();
                    let Some(w) = picked.as_ref() else {
                        finished.set(false);
                        return;
                    };
                    RecordingTarget::Window {
                        window_id: w.id.clone(),
                    }
                }
            };
            window.hide();
            let window = window.clone();
            let done = done.clone();
            let cancelled = cancelled.clone();
            let display = frame.descriptor.clone();
            let crop = r.to_physical(
                display.overlay_to_buffer_scale(frame.image.width(), frame.image.height()),
                frame.image.width(),
                frame.image.height(),
            );
            let frozen = frame.crop(crop);
            ui::job(
                move || {
                    if !captures_session::capture_session_available() {
                        return Err(
                            "Capture cancelled because the desktop session is unavailable.".into(),
                        );
                    }
                    let image = match &target {
                        RecordingTarget::Window { window_id } => XcapBackend
                            .capture_window(window_id)
                            .map_err(|e| e.to_string())?,
                        _ => frozen.ok_or("The selected region is empty")?,
                    };
                    Ok(Selection {
                        display,
                        target,
                        image,
                    })
                },
                move |result| {
                    match result {
                        Ok(selection) => done(selection),
                        Err(error) => {
                            ui::error(&window, &error);
                            cancelled();
                        }
                    }
                    window.close();
                },
            );
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let accept = accept.clone();
        window.connect_key_press_event(move |window, event| {
            match event.keyval() {
                gdk::keys::constants::Escape => window.close(),
                gdk::keys::constants::Return => {
                    if accept.is_sensitive() {
                        accept.clicked();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.connect_delete_event(move |_, _| {
        if !finished.replace(true) {
            cancelled();
        }
        glib::Propagation::Proceed
    });
    window.show_all();
    window.present();
}
