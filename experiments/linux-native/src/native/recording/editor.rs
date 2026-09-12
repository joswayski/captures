//! Native recording editor surface.
use captures_media::{
    AudioEdit, CancelToken, CropRect, EditSpec, ExportFormat, ExportSpec, MediaToolchain,
    QualityPreset, TimelineSpriteSpec,
};
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::Duration,
};

use super::{create_private_work_directory, named_output};
use crate::compat::prelude::*;
use crate::ui;

#[path = "playback.rs"]
mod playback;
use playback::{AudioPlayback, Playback};

const CROP_MIN_SIZE: f64 = 2.0;
const CROP_HANDLE_RADIUS: f64 = 7.0;
const CROP_HIT_RADIUS: f64 = 12.0;
const TRIM_HIT_RADIUS: f64 = 14.0;

#[derive(Clone, Copy, Default)]
struct SourceAudio {
    system: bool,
    microphone: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Size {
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Crop {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MediaFit {
    offset: Point,
    scale: f64,
    source: Size,
}

impl MediaFit {
    fn new(viewport: Size, source: Size) -> Option<Self> {
        if viewport.width <= 0.0
            || viewport.height <= 0.0
            || source.width <= 0.0
            || source.height <= 0.0
        {
            return None;
        }
        let scale = (viewport.width / source.width).min(viewport.height / source.height);
        Some(Self {
            offset: Point {
                x: (viewport.width - source.width * scale) / 2.0,
                y: (viewport.height - source.height * scale) / 2.0,
            },
            scale,
            source,
        })
    }

    fn source_to_view(self, point: Point) -> Point {
        Point {
            x: self.offset.x + point.x * self.scale,
            y: self.offset.y + point.y * self.scale,
        }
    }

    fn view_to_source(self, point: Point) -> Point {
        Point {
            x: ((point.x - self.offset.x) / self.scale).clamp(0.0, self.source.width),
            y: ((point.y - self.offset.y) / self.scale).clamp(0.0, self.source.height),
        }
    }

    fn contains_view(self, point: Point) -> bool {
        point.x >= self.offset.x
            && point.y >= self.offset.y
            && point.x <= self.offset.x + self.source.width * self.scale
            && point.y <= self.offset.y + self.source.height * self.scale
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CropHandle {
    New,
    Move,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

#[derive(Clone, Copy, Debug)]
struct CropDrag {
    handle: CropHandle,
    start: Point,
    initial: Crop,
}

fn bounded_crop(crop: Crop, source: Size) -> Crop {
    let width = crop
        .width
        .round()
        .clamp(CROP_MIN_SIZE, source.width.max(CROP_MIN_SIZE));
    let height = crop
        .height
        .round()
        .clamp(CROP_MIN_SIZE, source.height.max(CROP_MIN_SIZE));
    Crop {
        x: crop.x.round().clamp(0.0, (source.width - width).max(0.0)),
        y: crop.y.round().clamp(0.0, (source.height - height).max(0.0)),
        width,
        height,
    }
}

fn crop_after_numeric_change(crop: Crop, source: Size, field: usize) -> Crop {
    let mut crop = crop;
    match field {
        0 => {
            crop.x = crop
                .x
                .round()
                .clamp(0.0, (source.width - crop.width).max(0.0))
        }
        1 => {
            crop.y = crop
                .y
                .round()
                .clamp(0.0, (source.height - crop.height).max(0.0))
        }
        2 => {
            crop.width = crop
                .width
                .round()
                .clamp(CROP_MIN_SIZE, (source.width - crop.x).max(CROP_MIN_SIZE));
        }
        3 => {
            crop.height = crop
                .height
                .round()
                .clamp(CROP_MIN_SIZE, (source.height - crop.y).max(CROP_MIN_SIZE));
        }
        _ => unreachable!(),
    }
    crop
}

fn crop_after_drag(drag: CropDrag, current: Point, source: Size) -> Crop {
    if drag.handle == CropHandle::New {
        let left = drag.start.x.min(current.x);
        let right = drag.start.x.max(current.x);
        let top = drag.start.y.min(current.y);
        let bottom = drag.start.y.max(current.y);
        return bounded_crop(
            Crop {
                x: left,
                y: top,
                width: (right - left).max(CROP_MIN_SIZE),
                height: (bottom - top).max(CROP_MIN_SIZE),
            },
            source,
        );
    }

    let delta = Point {
        x: current.x - drag.start.x,
        y: current.y - drag.start.y,
    };
    if drag.handle == CropHandle::Move {
        return bounded_crop(
            Crop {
                x: drag.initial.x + delta.x,
                y: drag.initial.y + delta.y,
                ..drag.initial
            },
            source,
        );
    }

    let west = matches!(
        drag.handle,
        CropHandle::West | CropHandle::NorthWest | CropHandle::SouthWest
    );
    let east = matches!(
        drag.handle,
        CropHandle::East | CropHandle::NorthEast | CropHandle::SouthEast
    );
    let north = matches!(
        drag.handle,
        CropHandle::North | CropHandle::NorthEast | CropHandle::NorthWest
    );
    let south = matches!(
        drag.handle,
        CropHandle::South | CropHandle::SouthEast | CropHandle::SouthWest
    );
    let mut left = drag.initial.x + if west { delta.x } else { 0.0 };
    let mut right = drag.initial.x + drag.initial.width + if east { delta.x } else { 0.0 };
    let mut top = drag.initial.y + if north { delta.y } else { 0.0 };
    let mut bottom = drag.initial.y + drag.initial.height + if south { delta.y } else { 0.0 };
    if west {
        left = left.clamp(0.0, right - CROP_MIN_SIZE);
    }
    if east {
        right = right.clamp(left + CROP_MIN_SIZE, source.width);
    }
    if north {
        top = top.clamp(0.0, bottom - CROP_MIN_SIZE);
    }
    if south {
        bottom = bottom.clamp(top + CROP_MIN_SIZE, source.height);
    }
    bounded_crop(
        Crop {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        },
        source,
    )
}

fn crop_after_locked_drag(drag: CropDrag, current: Point, source: Size) -> Crop {
    let crop = crop_after_drag(drag, current, source);
    if matches!(drag.handle, CropHandle::Move | CropHandle::New) {
        return crop;
    }
    let ratio = drag.initial.width / drag.initial.height.max(1.0);
    let vertical_only = matches!(drag.handle, CropHandle::North | CropHandle::South);
    let (mut width, mut height) = if vertical_only {
        (crop.height * ratio, crop.height)
    } else {
        (crop.width, crop.width / ratio)
    };
    let maximum_width = if matches!(
        drag.handle,
        CropHandle::West | CropHandle::NorthWest | CropHandle::SouthWest
    ) {
        drag.initial.x + drag.initial.width
    } else {
        source.width - drag.initial.x
    };
    let maximum_height = if matches!(
        drag.handle,
        CropHandle::North | CropHandle::NorthEast | CropHandle::NorthWest
    ) {
        drag.initial.y + drag.initial.height
    } else {
        source.height - drag.initial.y
    };
    if width > maximum_width {
        width = maximum_width;
        height = width / ratio;
    }
    if height > maximum_height {
        height = maximum_height;
        width = height * ratio;
    }
    let x = if matches!(
        drag.handle,
        CropHandle::West | CropHandle::NorthWest | CropHandle::SouthWest
    ) {
        drag.initial.x + drag.initial.width - width
    } else {
        drag.initial.x
    };
    let y = if matches!(
        drag.handle,
        CropHandle::North | CropHandle::NorthEast | CropHandle::NorthWest
    ) {
        drag.initial.y + drag.initial.height - height
    } else {
        drag.initial.y
    };
    bounded_crop(
        Crop {
            x,
            y,
            width,
            height,
        },
        source,
    )
}

fn crop_handle_at(crop: Crop, fit: MediaFit, pointer: Point) -> CropHandle {
    let left = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y,
        })
        .x;
    let right = fit
        .source_to_view(Point {
            x: crop.x + crop.width,
            y: crop.y,
        })
        .x;
    let top = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y,
        })
        .y;
    let bottom = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y + crop.height,
        })
        .y;
    let center_x = (left + right) / 2.0;
    let center_y = (top + bottom) / 2.0;
    let handles = [
        (CropHandle::NorthWest, Point { x: left, y: top }),
        (CropHandle::NorthEast, Point { x: right, y: top }),
        (
            CropHandle::SouthEast,
            Point {
                x: right,
                y: bottom,
            },
        ),
        (CropHandle::SouthWest, Point { x: left, y: bottom }),
        (
            CropHandle::North,
            Point {
                x: center_x,
                y: top,
            },
        ),
        (
            CropHandle::East,
            Point {
                x: right,
                y: center_y,
            },
        ),
        (
            CropHandle::South,
            Point {
                x: center_x,
                y: bottom,
            },
        ),
        (
            CropHandle::West,
            Point {
                x: left,
                y: center_y,
            },
        ),
    ];
    if let Some((handle, _)) = handles.into_iter().find(|(_, point)| {
        (pointer.x - point.x).abs() <= CROP_HIT_RADIUS
            && (pointer.y - point.y).abs() <= CROP_HIT_RADIUS
    }) {
        return handle;
    }
    if pointer.x >= left && pointer.x <= right && pointer.y >= top && pointer.y <= bottom {
        CropHandle::Move
    } else {
        CropHandle::New
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TimelineDrag {
    Scrub,
    Start,
    End,
}

fn set_timeline_accessible_name(area: &gtk::DrawingArea, target: TimelineDrag) {
    accessible_name(
        area,
        match target {
            TimelineDrag::Start => "Trim start handle",
            TimelineDrag::End => "Trim end handle",
            TimelineDrag::Scrub => "Playback position",
        },
    );
}

fn timeline_time_at(pointer_x: f64, width: f64, duration_ms: u64) -> u64 {
    if width <= 0.0 {
        return 0;
    }
    ((pointer_x / width).clamp(0.0, 1.0) * duration_ms as f64).round() as u64
}

fn trim_after_drag(
    drag: TimelineDrag,
    pointer_x: f64,
    width: f64,
    duration_ms: u64,
    start_ms: u64,
    end_ms: u64,
) -> (u64, u64, u64) {
    let time = timeline_time_at(pointer_x, width, duration_ms);
    match drag {
        TimelineDrag::Start => {
            let next = time.min(end_ms.saturating_sub(1));
            (next, end_ms, next)
        }
        TimelineDrag::End => {
            let next = time.clamp(start_ms.saturating_add(1), duration_ms.max(1));
            (start_ms, next, next)
        }
        TimelineDrag::Scrub => (start_ms, end_ms, time),
    }
}

#[allow(clippy::too_many_lines)]
pub fn open(path: PathBuf, directory: PathBuf, on_saved: Rc<dyn Fn(PathBuf)>) {
    let window = gtk::Window::new();
    window.set_title(Some("Edit recording — Captures"));
    window.set_default_size(1240, 760);
    window.set_size_request(920, 620);
    window.set_position(());

    install_editor_styles(&window);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.style_context().add_class("recording-editor-root");
    let page_scroll = gtk::ScrolledWindow::new();
    page_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    page_scroll.set_overlay_scrolling(false);
    page_scroll
        .style_context()
        .add_class("recording-editor-scroll");
    accessible_name(&page_scroll, "Recording editor page");
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_margin_top(24);
    page.set_margin_bottom(112);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_halign(gtk::Align::Fill);
    page.set_hexpand(true);
    page.set_size_request(860, -1);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let heading = ui::label("Edit recording", "title");
    heading
        .style_context()
        .add_class("recording-editor-heading");
    accessible_name(&heading, "Edit recording");
    let source_summary = ui::label("Reading recording…", "muted");
    header.pack_start(&heading, false, false, 0);
    source_summary.set_no_show_all(true);
    page.pack_start(&header, false, false, 0);

    let preview_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_card.set_margin_top(4);
    preview_card.style_context().add_class("editor-card");
    preview_card
        .style_context()
        .add_class("recording-preview-card");
    let preview_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    preview_toolbar
        .style_context()
        .add_class("recording-preview-toolbar");
    let preview_title = ui::label("Preview", "section-title");
    preview_toolbar.pack_start(&preview_title, false, false, 0);
    let play = ui::icon_button("Play preview", "play");
    play.set_tooltip_text(Some("Play or pause the in-window preview (Space)"));
    accessible_name(&play, "Play preview");
    play.style_context().add_class("preview-play-button");
    let loop_toggle = gtk::CheckButton::with_label("Loop preview");
    accessible_name(&loop_toggle, "Loop preview");
    loop_toggle.set_tooltip_text(Some("Loop playback within the selected trim range"));
    loop_toggle
        .style_context()
        .add_class("recording-loop-toggle");
    let fit = gtk::CheckButton::with_label("Fit");
    let actual = gtk::CheckButton::with_label("100%");
    actual.set_group(Some(&fit));
    fit.set_active(true);
    fit.style_context().add_class("preview-size-button");
    fit.style_context().add_class("preview-size-first");
    actual.style_context().add_class("preview-size-button");
    actual.style_context().add_class("preview-size-last");
    accessible_name(&fit, "Fit preview");
    accessible_name(&actual, "Show preview at 100%");
    let preview_actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    preview_actions.pack_start(&loop_toggle, false, false, 10);
    preview_actions.pack_start(&fit, false, false, 0);
    preview_actions.pack_start(&actual, false, false, 0);
    preview_toolbar.pack_end(&preview_actions, false, false, 0);
    preview_card.pack_start(&preview_toolbar, false, false, 0);
    let preview_frame = gtk::Frame::new(None);
    preview_frame.style_context().add_class("recording-preview");
    preview_frame
        .style_context()
        .add_class("recording-preview-frame");
    let preview = gtk::Image::new();
    preview.set_size_request(680, 400);
    accessible_name(&preview, "Recording frame preview");
    let preview_overlay = gtk::Overlay::new();
    preview_overlay.set_halign(gtk::Align::Center);
    preview_overlay.set_valign(gtk::Align::Center);
    preview_overlay.add(&preview);
    let preview_events = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let comparison_images = Rc::new(RefCell::new(
        None::<(gtk::gdk_pixbuf::Pixbuf, gtk::gdk_pixbuf::Pixbuf)>,
    ));
    let comparison_split = Rc::new(Cell::new(0.5_f64));
    let comparison_dragging = Rc::new(Cell::new(false));
    let comparison_overlay = gtk::DrawingArea::new();
    // A separate GDK child window would cover the non-windowed Play button.
    comparison_overlay.set_has_window(false);
    comparison_overlay.set_no_show_all(true);
    accessible_name(&comparison_overlay, "Embedded compression comparison");
    let comparison_handle = gtk::Button::new();
    comparison_handle
        .style_context()
        .add_class("recording-comparison-handle-hit");
    comparison_handle.set_can_focus(true);
    comparison_handle.set_size_request(44, 44);
    comparison_handle.set_halign(gtk::Align::Start);
    comparison_handle.set_valign(gtk::Align::Center);
    comparison_handle.set_no_show_all(true);
    accessible_name(
        &comparison_handle,
        "Compression comparison slider, Before on the left and After on the right",
    );
    comparison_handle.set_tooltip_text(Some(
        "Drag the divider or use Left and Right arrow keys to compare compression",
    ));
    preview_overlay.add_overlay(&comparison_overlay);
    preview_overlay.set_overlay_pass_through(&comparison_overlay, true);
    comparison_overlay.connect_draw({
        let images = comparison_images.clone();
        let split = comparison_split.clone();
        let handle = comparison_handle.clone();
        move |area, context| {
            let images = images.borrow();
            let Some((before, after)) = images.as_ref() else {
                return glib::Propagation::Proceed;
            };
            let width = f64::from(area.allocated_width()).max(1.0);
            let height = f64::from(area.allocated_height()).max(1.0);
            let divider_x = width * split.get();
            let paint = |pixbuf: &gtk::gdk_pixbuf::Pixbuf| {
                let _ = context.save();
                context.scale(
                    width / f64::from(pixbuf.width()).max(1.0),
                    height / f64::from(pixbuf.height()).max(1.0),
                );
                context.set_source_pixbuf(pixbuf, 0.0, 0.0);
                let _ = context.paint();
                let _ = context.restore();
            };
            paint(before);
            let _ = context.save();
            context.rectangle(divider_x, 0.0, (width - divider_x).max(0.0), height);
            context.clip();
            paint(after);
            let _ = context.restore();

            context.set_source_rgba(1.0, 1.0, 1.0, 0.98);
            context.set_line_width(2.0);
            context.move_to(divider_x, 0.0);
            context.line_to(divider_x, height);
            let _ = context.stroke();
            context.arc(divider_x, height / 2.0, 18.0, 0.0, std::f64::consts::TAU);
            context.set_source_rgba(0.08, 0.09, 0.11, 0.88);
            let _ = context.fill_preserve();
            context.set_source_rgba(1.0, 1.0, 1.0, 0.9);
            context.set_line_width(if handle.has_focus() { 3.0 } else { 1.0 });
            let _ = context.stroke();
            context.select_font_face(
                "sans-serif",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Bold,
            );
            context.set_font_size(13.0);
            context.move_to(divider_x - 9.0, height / 2.0 + 4.5);
            let _ = context.show_text("‹ ›");

            for (label, x) in [("Before", 14.0), ("After", width - 64.0)] {
                context.rectangle(x, height - 38.0, 54.0, 24.0);
                context.set_source_rgba(0.08, 0.09, 0.11, 0.84);
                let _ = context.fill();
                context.set_source_rgba(1.0, 1.0, 1.0, 0.96);
                context.set_font_size(11.0);
                context.move_to(x + 9.0, height - 21.5);
                let _ = context.show_text(label);
            }
            glib::Propagation::Proceed
        }
    });
    let set_comparison_split = {
        let split = comparison_split.clone();
        let overlay = comparison_overlay.clone();
        let handle = comparison_handle.clone();
        Rc::new(move |x: f64, width: f64| {
            split.set((x / width.max(1.0)).clamp(0.06, 0.94));
            handle.set_margin_start((width * split.get() - 22.0).max(0.0).round() as i32);
            overlay.queue_draw();
        })
    };
    comparison_overlay.connect_size_allocate({
        let split = comparison_split.clone();
        let handle = comparison_handle.clone();
        move |_, allocation| {
            handle.set_margin_start(
                (f64::from(allocation.width()) * split.get() - 22.0)
                    .max(0.0)
                    .round() as i32,
            );
        }
    });
    preview_events.connect_button_press_event({
        let dragging = comparison_dragging.clone();
        let set_split = set_comparison_split.clone();
        let overlay = comparison_overlay.clone();
        let handle = comparison_handle.clone();
        let split = comparison_split.clone();
        move |_, event| {
            if event.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (x, y) = event.position();
            let divider_x = f64::from(overlay.allocated_width()) * split.get();
            let divider_y = f64::from(overlay.allocated_height()) / 2.0;
            if (x - divider_x).abs() > 24.0 || (y - divider_y).abs() > 24.0 {
                return glib::Propagation::Proceed;
            }
            handle.grab_focus();
            dragging.set(true);
            set_split(x, f64::from(overlay.allocated_width()));
            glib::Propagation::Stop
        }
    });
    comparison_handle.connect_button_press_event({
        let dragging = comparison_dragging.clone();
        move |handle, event| {
            if event.button() != 1 {
                return glib::Propagation::Proceed;
            }
            handle.grab_focus();
            dragging.set(true);
            glib::Propagation::Stop
        }
    });
    preview_events.connect_motion_notify_event({
        let dragging = comparison_dragging.clone();
        let set_split = set_comparison_split.clone();
        let overlay = comparison_overlay.clone();
        move |_, event| {
            if !dragging.get() {
                return glib::Propagation::Proceed;
            }
            set_split(event.position().0, f64::from(overlay.allocated_width()));
            glib::Propagation::Stop
        }
    });
    comparison_handle.connect_motion_notify_event({
        let dragging = comparison_dragging.clone();
        let set_split = set_comparison_split.clone();
        let overlay = comparison_overlay.clone();
        move |handle, event| {
            if !dragging.get() {
                return glib::Propagation::Proceed;
            }
            set_split(
                f64::from(handle.margin_start()) + event.position().0,
                f64::from(overlay.allocated_width()),
            );
            glib::Propagation::Stop
        }
    });
    preview_events.connect_button_release_event({
        let dragging = comparison_dragging.clone();
        move |_, event| {
            if event.button() == 1 && dragging.replace(false) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    comparison_handle.connect_button_release_event({
        let dragging = comparison_dragging.clone();
        move |_, event| {
            if event.button() == 1 && dragging.replace(false) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    let comparison_gesture = gtk::GestureDrag::new();
    comparison_handle.add_controller(comparison_gesture.clone());
    let comparison_gesture_start = Rc::new(Cell::new(0.5_f64));
    comparison_gesture.connect_drag_begin({
        let split = comparison_split.clone();
        let start = comparison_gesture_start.clone();
        let handle = comparison_handle.clone();
        move |_, _, _| {
            start.set(split.get());
            handle.grab_focus();
        }
    });
    comparison_gesture.connect_drag_update({
        let start = comparison_gesture_start.clone();
        let overlay = comparison_overlay.clone();
        let set_split = set_comparison_split.clone();
        move |_, offset_x, _| {
            let width = f64::from(overlay.allocated_width()).max(1.0);
            set_split(start.get() * width + offset_x, width);
        }
    });
    comparison_handle.connect_key_press_event({
        let split = comparison_split.clone();
        let overlay = comparison_overlay.clone();
        let set_split = set_comparison_split.clone();
        move |_, event| {
            let next = match event.keyval() {
                gtk::gdk::Key::Left => Some(split.get() - 0.02),
                gtk::gdk::Key::Right => Some(split.get() + 0.02),
                gtk::gdk::Key::Home => Some(0.06),
                gtk::gdk::Key::End => Some(0.94),
                _ => None,
            };
            if let Some(next) = next {
                set_split(
                    next.clamp(0.06, 0.94) * f64::from(overlay.allocated_width()),
                    f64::from(overlay.allocated_width()),
                );
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    comparison_handle.connect_focus_in_event({
        let overlay = comparison_overlay.clone();
        move |_, _| {
            overlay.queue_draw();
            glib::Propagation::Proceed
        }
    });
    comparison_handle.connect_focus_out_event({
        let overlay = comparison_overlay.clone();
        move |_, _| {
            overlay.queue_draw();
            glib::Propagation::Proceed
        }
    });
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    // Keep the button's rendering and input above the crop DrawingArea's GDK
    // window, rather than merely above it in GTK's non-windowed paint order.
    let play_overlay = gtk::Box::new(gtk::Orientation::Vertical, 0);
    play_overlay.set_halign(gtk::Align::Center);
    play_overlay.set_valign(gtk::Align::Center);
    play_overlay.add(&play);
    preview_overlay.add_overlay(&play_overlay);
    preview_overlay.add_overlay(&comparison_handle);
    preview_overlay.set_overlay_pass_through(&comparison_handle, false);
    let crop_overlay = gtk::DrawingArea::new();
    accessible_name(&crop_overlay, "Interactive recording crop");
    preview_overlay.add_overlay(&crop_overlay);
    preview_overlay.set_overlay_pass_through(&crop_overlay, true);
    preview_overlay.reorder_overlay(&play_overlay, -1);
    preview_overlay.reorder_overlay(&comparison_handle, -1);
    let preview_scroll = gtk::ScrolledWindow::new();
    preview_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_scroll.set_overlay_scrolling(true);
    preview_scroll.set_size_request(-1, 396);
    preview_events.add(&preview_overlay);
    preview_scroll.add(&preview_events);
    preview_frame.add(&preview_scroll);
    preview_card.pack_start(&preview_frame, true, true, 0);
    page.pack_start(&preview_card, false, false, 0);

    let timeline_card = card("");
    timeline_card
        .style_context()
        .add_class("recording-timeline-card");
    let timeline_summary = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let timeline_range = ui::label("0:00.0 – 0:00.0", "section-title");
    let timeline_selected = ui::label("0:00.0 selected", "muted");
    timeline_summary.pack_start(&timeline_range, false, false, 0);
    timeline_summary.pack_end(&timeline_selected, false, false, 0);
    timeline_card.pack_start(&timeline_summary, false, false, 0);
    let timeline = gtk::Image::new();
    timeline.set_size_request(1080, 68);
    accessible_name(&timeline, "Recording timeline filmstrip");
    let timeline_overlay = gtk::Overlay::new();
    timeline_overlay.add(&timeline);
    let timeline_interaction = gtk::DrawingArea::new();
    timeline_interaction.set_can_focus(true);
    accessible_name(&timeline_interaction, "Interactive trim timeline");
    timeline_interaction.set_tooltip_text(Some(
        "Tab cycles trim start, trim end, and playhead; arrow keys adjust the focused control",
    ));
    timeline_overlay.add_overlay(&timeline_interaction);
    let position = scale("Playback position");
    let trim_start = scale("Trim start");
    let trim_end = scale("Trim end");
    for widget in [&position, &trim_start, &trim_end] {
        widget.set_draw_value(false);
        widget.set_hexpand(true);
        widget.set_no_show_all(true);
    }
    timeline_card.pack_start(&timeline_overlay, false, false, 0);
    timeline_card.pack_start(&position, false, false, 0);
    timeline_card.pack_start(&trim_start, false, false, 0);
    timeline_card.pack_start(&trim_end, false, false, 0);
    page.pack_start(&timeline_card, false, false, 0);

    let options = gtk::Grid::new();
    options.set_row_spacing(20);
    options.set_column_spacing(20);
    options.set_column_homogeneous(true);
    options.set_hexpand(true);
    accessible_name(&options, "Recording export options");
    let picture_card = card("Crop & size");
    let crop_enabled = gtk::CheckButton::with_label("Crop recording");
    crop_enabled.set_tooltip_text(Some("Show and apply the crop rectangle"));
    picture_card.pack_start(&crop_enabled, false, false, 0);
    let crop_grid = gtk::Grid::new();
    crop_grid.set_row_spacing(7);
    crop_grid.set_column_spacing(7);
    let crop_x = spin(0.0, 20_000.0, 2.0, "Crop X");
    let crop_y = spin(0.0, 20_000.0, 2.0, "Crop Y");
    let crop_w = spin(2.0, 20_000.0, 2.0, "Crop width");
    let crop_h = spin(2.0, 20_000.0, 2.0, "Crop height");
    let source_size = Rc::new(Cell::new(Size::default()));
    let crop_drag = Rc::new(RefCell::new(None::<CropDrag>));
    let set_preview_size = {
        let preview = preview.clone();
        let preview_overlay = preview_overlay.clone();
        let source_size = source_size.clone();
        Rc::new(move |actual_size: bool| {
            let source = source_size.get();
            if source.width <= 0.0 || source.height <= 0.0 {
                return;
            }
            let (width, height) = if actual_size {
                (source.width.round() as i32, source.height.round() as i32)
            } else {
                let scale = (900.0 / source.width).min(396.0 / source.height).min(1.0);
                (
                    (source.width * scale).round() as i32,
                    (source.height * scale).round() as i32,
                )
            };
            preview.set_size_request(width.max(2), height.max(2));
            preview_overlay.set_size_request(width.max(2), height.max(2));
        })
    };
    crop_overlay.connect_draw({
        let crop_enabled = crop_enabled.clone();
        let crop_x = crop_x.clone();
        let crop_y = crop_y.clone();
        let crop_w = crop_w.clone();
        let crop_h = crop_h.clone();
        let source_size = source_size.clone();
        move |area, context| {
            if !crop_enabled.is_active() {
                return glib::Propagation::Proceed;
            }
            let source = source_size.get();
            let Some(fit) = MediaFit::new(
                Size {
                    width: f64::from(area.allocated_width()),
                    height: f64::from(area.allocated_height()),
                },
                source,
            ) else {
                return glib::Propagation::Proceed;
            };
            let crop = bounded_crop(
                crop_from_widgets(&crop_x, &crop_y, &crop_w, &crop_h),
                source,
            );
            let top_left = fit.source_to_view(Point {
                x: crop.x,
                y: crop.y,
            });
            let bottom_right = fit.source_to_view(Point {
                x: crop.x + crop.width,
                y: crop.y + crop.height,
            });

            context.set_source_rgba(0.0, 0.0, 0.0, 0.56);
            context.rectangle(
                fit.offset.x,
                fit.offset.y,
                source.width * fit.scale,
                source.height * fit.scale,
            );
            context.rectangle(
                top_left.x,
                top_left.y,
                bottom_right.x - top_left.x,
                bottom_right.y - top_left.y,
            );
            context.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
            let _ = context.fill();
            let accent = area
                .style_context()
                .lookup_color("captures_accent")
                .unwrap_or_else(|| gtk::gdk::RGBA::new(0.10, 0.58, 0.96, 1.0));
            context.set_source_rgba(
                f64::from(accent.red()),
                f64::from(accent.green()),
                f64::from(accent.blue()),
                0.98,
            );
            context.set_line_width(3.0);
            context.rectangle(
                top_left.x,
                top_left.y,
                bottom_right.x - top_left.x,
                bottom_right.y - top_left.y,
            );
            let _ = context.stroke();
            for handle in crop_handle_points(crop, fit) {
                context.arc(
                    handle.x,
                    handle.y,
                    CROP_HANDLE_RADIUS,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = context.fill();
                context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                context.arc(
                    handle.x,
                    handle.y,
                    CROP_HANDLE_RADIUS - 3.0,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = context.fill();
                context.set_source_rgba(
                    f64::from(accent.red()),
                    f64::from(accent.green()),
                    f64::from(accent.blue()),
                    0.98,
                );
            }

            let label = format!("{} × {}", crop.width as u32, crop.height as u32);
            context.select_font_face(
                "Sans",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Bold,
            );
            context.set_font_size(12.0);
            let extents = context.text_extents(&label).ok();
            let label_width = extents.map_or(64.0, |value| value.width() + 12.0);
            let label_y = (top_left.y + 8.0)
                .min(bottom_right.y - 24.0)
                .max(top_left.y);
            context.set_source_rgba(0.0, 0.0, 0.0, 0.72);
            context.rectangle(top_left.x + 8.0, label_y + 4.0, label_width, 22.0);
            let _ = context.fill();
            context.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            context.move_to(top_left.x + 14.0, label_y + 20.0);
            let _ = context.show_text(&label);
            glib::Propagation::Proceed
        }
    });
    for (index, (name, field)) in [
        ("X", &crop_x),
        ("Y", &crop_y),
        ("Width", &crop_w),
        ("Height", &crop_h),
    ]
    .into_iter()
    .enumerate()
    {
        crop_grid.attach(
            &ui::label(name, "muted"),
            (index % 2 * 2) as i32,
            (index / 2) as i32,
            1,
            1,
        );
        crop_grid.attach(field, (index % 2 * 2 + 1) as i32, (index / 2) as i32, 1, 1);
    }
    let reset_crop = ui::button("Reset crop");
    accessible_name(&reset_crop, "Reset crop to full recording");
    crop_grid.attach(&reset_crop, 0, 2, 4, 1);
    picture_card.pack_start(&crop_grid, false, false, 0);
    let aspect_locked = gtk::CheckButton::with_label("Lock aspect ratio");
    aspect_locked.set_active(true);
    picture_card.pack_start(&aspect_locked, false, false, 0);
    let resolution = gtk::ComboBoxText::new();
    for (id, text) in [
        ("original", "Original"),
        ("1080", "1080p"),
        ("720", "720p"),
        ("custom", "Custom"),
    ] {
        resolution.append(Some(id), text);
    }
    resolution.set_active_id(Some("original"));
    accessible_name(&resolution, "Output resolution");
    picture_card.pack_start(&labeled("Output resolution", &resolution), false, false, 0);
    let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let out_w = spin(2.0, 20_000.0, 2.0, "Output width");
    let out_h = spin(2.0, 20_000.0, 2.0, "Output height");
    size_row.pack_start(&labeled("Width", &out_w), true, true, 0);
    size_row.pack_start(&labeled("Height", &out_h), true, true, 0);
    picture_card.pack_start(&size_row, false, false, 0);
    options.attach(&picture_card, 0, 0, 1, 1);

    let quality_card = card("Save quality");
    let format = gtk::ComboBoxText::new();
    format.append(Some("mp4"), ".mp4");
    format.append(Some("gif"), ".gif");
    format.append(Some("webm"), ".webm — unavailable");
    format.set_active_id(Some("mp4"));
    accessible_name(&format, "Format");
    let quality = gtk::ComboBoxText::new();
    for (id, name) in [
        ("preserve", "Preserve quality"),
        ("highest", "Compress · Highest"),
        ("high", "Compress · High"),
        ("standard", "Compress · Balanced"),
        ("small", "Compress · Smaller"),
        ("tiny", "Compress · Tiny"),
    ] {
        quality.append(Some(id), name);
    }
    quality.set_active_id(Some("preserve"));
    accessible_name(&quality, "Save quality");
    quality_card.pack_start(&labeled("Save quality", &quality), false, false, 0);
    let maximum_enabled = gtk::CheckButton::with_label("Maximum file size");
    let maximum_mb = spin(0.1, 10_000.0, 0.5, "Maximum file size in MB");
    maximum_mb.set_value(10.0);
    quality_card.pack_start(&maximum_enabled, false, false, 0);
    quality_card.pack_start(&labeled("Size limit (MB)", &maximum_mb), false, false, 0);
    let gif_fps = spin(1.0, 30.0, 1.0, "GIF frames per second");
    gif_fps.set_value(15.0);
    quality_card.pack_start(&labeled("GIF frame rate", &gif_fps), false, false, 0);
    let estimate = ui::label("Estimated saved size —", "muted");
    quality_card.pack_start(&estimate, false, false, 0);
    let compare = ui::button("Compare before / after");
    accessible_name(&compare, "Compare compression before and after");
    quality_card.pack_start(&compare, false, false, 0);
    options.attach(&quality_card, 1, 0, 1, 1);

    let audio_card = card("Audio");
    let audio_row = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let system_volume = scale("System audio volume");
    let microphone_volume = scale("Microphone volume");
    for volume in [&system_volume, &microphone_volume] {
        volume.set_range(0.0, 200.0);
        volume.set_value(100.0);
        volume.set_draw_value(true);
        volume.set_value_pos(gtk::PositionType::Right);
    }
    let mute_system = gtk::CheckButton::with_label("Mute system audio");
    let mute_microphone = gtk::CheckButton::with_label("Mute microphone");
    let mono = gtk::CheckButton::with_label("Mix output to mono");
    let system_box = gtk::Box::new(gtk::Orientation::Vertical, 5);
    system_box.pack_start(&labeled("System audio", &system_volume), false, false, 0);
    system_box.pack_start(&mute_system, false, false, 0);
    let microphone_box = gtk::Box::new(gtk::Orientation::Vertical, 5);
    microphone_box.pack_start(&labeled("Microphone", &microphone_volume), false, false, 0);
    microphone_box.pack_start(&mute_microphone, false, false, 0);
    audio_row.pack_start(&system_box, true, true, 0);
    audio_row.pack_start(&microphone_box, true, true, 0);
    audio_row.pack_start(&mono, false, false, 0);
    audio_card.pack_start(&audio_row, false, false, 0);
    options.attach(&audio_card, 0, 1, 2, 1);

    page.pack_start(&options, false, false, 0);
    page_scroll.add(&page);
    root.pack_start(&page_scroll, true, true, 0);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer.style_context().add_class("recording-save-footer");
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    let filename_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
    filename_box.set_size_request(380, -1);
    let destination_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    destination_row.pack_start(&ui::label("Filename", "muted"), false, false, 0);
    let destination = Rc::new(RefCell::new(directory));
    let destination_label = ui::label(
        &format!("Saving to  {}", destination.borrow().display()),
        "muted",
    );
    destination_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let choose_destination = ui::button("Change…");
    accessible_name(&choose_destination, "Change save location");
    destination_row.pack_end(&choose_destination, false, false, 0);
    destination_row.pack_end(&destination_label, true, true, 0);
    let filename_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let filename = gtk::Entry::new();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Recording");
    filename.set_text(&format!("{stem} edited"));
    filename.set_hexpand(true);
    accessible_name(&filename, "Saved filename");
    filename_row.pack_start(&filename, true, true, 0);
    filename_row.pack_start(&format, false, false, 0);
    filename_box.pack_start(&destination_row, false, false, 0);
    filename_box.pack_start(&filename_row, false, false, 0);
    let make_copy = gtk::CheckButton::with_label("Save as new file");
    make_copy.set_active(true);
    make_copy.set_sensitive(false);
    make_copy.set_tooltip_text(Some(
        "The Linux experiment always preserves the source recording",
    ));
    let status = ui::label("Preparing editor…", "muted");
    let cancel = ui::button("Cancel export");
    let export = ui::button("Save");
    export.set_image(Some(&ui::icon("save", 16)));
    export.set_always_show_image(true);
    export.style_context().add_class("primary");
    cancel.set_sensitive(false);
    cancel.set_opacity(0.0);
    footer.pack_start(&filename_box, true, true, 0);
    footer.pack_start(&make_copy, false, false, 8);
    footer.pack_start(&status, true, true, 0);
    footer.pack_start(&cancel, false, false, 0);
    footer.pack_start(&export, false, false, 0);
    root.pack_start(&footer, false, false, 0);

    window.add(&root);
    window.show_all();
    comparison_overlay.hide();
    comparison_handle.hide();
    crop_grid.set_sensitive(false);
    maximum_mb.set_sensitive(false);
    gif_fps.set_sensitive(false);

    let scratch = match create_private_work_directory(&std::env::temp_dir(), "recording-editor") {
        Ok(path) => path,
        Err(error) => {
            ui::error(&window, &error);
            window.close();
            return;
        }
    };
    let closed = Rc::new(Cell::new(false));
    let duration_ms = Rc::new(Cell::new(1_u64));
    let trim_start_ms = Rc::new(Cell::new(0_u64));
    let trim_end_ms = Rc::new(Cell::new(1_u64));
    let source_audio = Rc::new(Cell::new(SourceAudio::default()));
    let playback_slot = Rc::new(RefCell::new(None::<Rc<Playback>>));

    let probe_path = path.clone();
    let probe_scratch = scratch.clone();
    let probe_closed = closed.clone();
    let probe_window = window.clone();
    let probe_playback = playback_slot.clone();
    let probe_audio = source_audio.clone();
    let probe_duration = duration_ms.clone();
    let probe_trim_end = trim_end_ms.clone();
    let probe_trim_start = trim_start_ms.clone();
    let probe_preview = preview.clone();
    let probe_set_preview_size = set_preview_size.clone();
    let probe_position = position.clone();
    let probe_play = play.clone();
    let probe_status = status.clone();
    let status_ready = status.clone();
    let playback_path = path.clone();
    let playback_scratch = scratch.clone();
    let trim_start_probe = trim_start.clone();
    let trim_end_probe = trim_end.clone();
    let crop_x_probe = crop_x.clone();
    let crop_y_probe = crop_y.clone();
    let crop_w_probe = crop_w.clone();
    let crop_h_probe = crop_h.clone();
    let source_size_probe = source_size.clone();
    let out_w_probe = out_w.clone();
    let out_h_probe = out_h.clone();
    let system_volume_probe = system_volume.clone();
    let microphone_volume_probe = microphone_volume.clone();
    let mute_system_probe = mute_system.clone();
    let mute_microphone_probe = mute_microphone.clone();
    let probe_compare = compare.clone();
    ui::job(
        move || {
            let media = MediaToolchain::from_command_names();
            media.verify().map_err(|error| error.to_string())?;
            let probe = media
                .probe(&probe_path)
                .map_err(|error| error.to_string())?;
            let audio = inspect_audio_streams(&probe_path, probe.audio_stream_count);
            let sprite = probe_scratch.join("timeline.png");
            media
                .create_timeline_sprite(
                    &probe_path,
                    &sprite,
                    TimelineSpriteSpec {
                        duration_ms: probe.metadata.duration_ms.unwrap_or(1).max(1),
                        frame_count: 12,
                        frame_width: 100,
                        frame_height: 68,
                    },
                    &CancelToken::default(),
                )
                .map_err(|error| error.to_string())?;
            Ok((probe, audio, sprite))
        },
        move |result| match result {
            Ok((probe, audio, sprite)) if !probe_closed.get() => {
                let duration = probe.metadata.duration_ms.unwrap_or(1).max(1);
                source_size_probe.set(Size {
                    width: probe.metadata.width as f64,
                    height: probe.metadata.height as f64,
                });
                probe_set_preview_size(false);
                probe_duration.set(duration);
                probe_trim_end.set(duration);
                probe_audio.set(audio);
                for scale in [&probe_position, &trim_start_probe, &trim_end_probe] {
                    scale.set_range(0.0, duration as f64);
                }
                trim_end_probe.set_value(duration as f64);
                crop_x_probe.set_range(0.0, probe.metadata.width.saturating_sub(2) as f64);
                crop_y_probe.set_range(0.0, probe.metadata.height.saturating_sub(2) as f64);
                crop_w_probe.set_range(2.0, probe.metadata.width as f64);
                crop_h_probe.set_range(2.0, probe.metadata.height as f64);
                crop_w_probe.set_value(probe.metadata.width as f64);
                crop_h_probe.set_value(probe.metadata.height as f64);
                out_w_probe.set_value(probe.metadata.width as f64);
                out_h_probe.set_value(probe.metadata.height as f64);
                source_summary.set_text(&format!(
                    "{} × {}  •  {}  •  {}",
                    probe.metadata.width,
                    probe.metadata.height,
                    super::format_duration(Duration::from_millis(duration)),
                    format_bytes(probe.metadata.size_bytes)
                ));
                estimate.set_text(&format!(
                    "Estimated saved size {}",
                    format_bytes(probe.metadata.size_bytes)
                ));
                system_box.set_sensitive(audio.system);
                microphone_box.set_sensitive(audio.microphone);
                if !audio.system && !audio.microphone {
                    audio_card.set_sensitive(false);
                    audio_card.set_tooltip_text(Some("This recording has no audio tracks."));
                }
                if let Ok(sprite) =
                    gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&sprite, 1080, 68, false)
                {
                    timeline.set_from_pixbuf(Some(&sprite));
                }
                let playback = Playback::new(
                    playback_path,
                    probe_preview,
                    probe_position.clone(),
                    probe_play,
                    probe_status,
                    probe_duration.clone(),
                    probe_trim_start,
                    probe_trim_end,
                    probe_closed,
                    playback_scratch,
                    playback_audio(
                        audio,
                        &system_volume_probe,
                        &microphone_volume_probe,
                        &mute_system_probe,
                        &mute_microphone_probe,
                    ),
                );
                playback.request_frame(0);
                *probe_playback.borrow_mut() = Some(playback);
                status_ready.set_text("Ready to save.");
                probe_compare.clicked();
            }
            Err(error) if !probe_closed.get() => ui::error(&probe_window, &error),
            _ => {}
        },
    );

    {
        let playback = playback_slot.clone();
        let comparison = comparison_overlay.clone();
        let handle = comparison_handle.clone();
        let dragging = comparison_dragging.clone();
        let play_overlay = play_overlay.clone();
        let crop_overlay = crop_overlay.clone();
        play.connect_clicked(move |_| {
            comparison.set_no_show_all(true);
            comparison.hide();
            handle.set_no_show_all(true);
            handle.hide();
            dragging.set(false);
            play_overlay.set_valign(gtk::Align::Center);
            play_overlay.set_margin_bottom(0);
            crop_overlay.show();
            if let Some(playback) = playback.borrow().as_ref() {
                playback.toggle();
            }
        });
    }
    {
        let playback = playback_slot.clone();
        loop_toggle.connect_toggled(move |toggle| {
            if let Some(playback) = playback.borrow().as_ref() {
                playback.set_looping(toggle.is_active());
            }
        });
    }
    fit.connect_toggled({
        let set_preview_size = set_preview_size.clone();
        move |toggle| {
            if toggle.is_active() {
                set_preview_size(false);
            }
        }
    });
    actual.connect_toggled({
        let set_preview_size = set_preview_size.clone();
        move |toggle| {
            if toggle.is_active() {
                set_preview_size(true);
            }
        }
    });
    {
        let playback = playback_slot.clone();
        let timeline_interaction = timeline_interaction.clone();
        position.connect_change_value(move |_, _, value| {
            if let Some(playback) = playback.borrow().as_ref() {
                playback.seek(value.max(0.0) as u64);
            }
            timeline_interaction.queue_draw();
            glib::Propagation::Proceed
        });
    }
    position.connect_value_changed({
        let timeline_interaction = timeline_interaction.clone();
        move |_| timeline_interaction.queue_draw()
    });
    {
        let start = trim_start_ms.clone();
        let end = trim_end_ms.clone();
        let playback = playback_slot.clone();
        let timeline_interaction = timeline_interaction.clone();
        let timeline_range = timeline_range.clone();
        let timeline_selected = timeline_selected.clone();
        trim_start.connect_value_changed(move |scale| {
            let value = (scale.value() as u64).min(end.get().saturating_sub(1));
            if value as f64 != scale.value() {
                scale.set_value(value as f64);
            }
            start.set(value);
            if let Some(playback) = playback.borrow().as_ref() {
                playback.seek(value);
            }
            timeline_range.set_text(&format!(
                "{} – {}",
                editor_time(value),
                editor_time(end.get())
            ));
            timeline_selected.set_text(&format!(
                "{} selected",
                editor_time(end.get().saturating_sub(value))
            ));
            timeline_interaction.queue_draw();
        });
    }
    {
        let start = trim_start_ms.clone();
        let end = trim_end_ms.clone();
        let timeline_interaction = timeline_interaction.clone();
        let timeline_range = timeline_range.clone();
        let timeline_selected = timeline_selected.clone();
        trim_end.connect_value_changed(move |scale| {
            let value = (scale.value() as u64).max(start.get().saturating_add(1));
            if value as f64 != scale.value() {
                scale.set_value(value as f64);
            }
            end.set(value);
            timeline_range.set_text(&format!(
                "{} – {}",
                editor_time(start.get()),
                editor_time(value)
            ));
            timeline_selected.set_text(&format!(
                "{} selected",
                editor_time(value.saturating_sub(start.get()))
            ));
            timeline_interaction.queue_draw();
        });
    }
    crop_enabled.connect_toggled({
        let grid = crop_grid.clone();
        let overlay = crop_overlay.clone();
        let preview_overlay = preview_overlay.clone();
        move |toggle| {
            grid.set_sensitive(toggle.is_active());
            preview_overlay.set_overlay_pass_through(&overlay, !toggle.is_active());
            overlay.queue_draw();
        }
    });
    let updating_crop = Rc::new(Cell::new(false));
    for (field_index, field) in [&crop_x, &crop_y, &crop_w, &crop_h].into_iter().enumerate() {
        field.connect_value_changed({
            let overlay = crop_overlay.clone();
            let source_size = source_size.clone();
            let updating_crop = updating_crop.clone();
            let crop_x = crop_x.clone();
            let crop_y = crop_y.clone();
            let crop_w = crop_w.clone();
            let crop_h = crop_h.clone();
            move |_| {
                let source = source_size.get();
                if source.width > 0.0 && source.height > 0.0 && !updating_crop.replace(true) {
                    set_crop_widgets(
                        crop_after_numeric_change(
                            crop_from_widgets(&crop_x, &crop_y, &crop_w, &crop_h),
                            source,
                            field_index,
                        ),
                        &crop_x,
                        &crop_y,
                        &crop_w,
                        &crop_h,
                    );
                    updating_crop.set(false);
                }
                overlay.queue_draw();
            }
        });
    }
    reset_crop.connect_clicked({
        let source_size = source_size.clone();
        let crop_x = crop_x.clone();
        let crop_y = crop_y.clone();
        let crop_w = crop_w.clone();
        let crop_h = crop_h.clone();
        move |_| {
            let source = source_size.get();
            if source.width > 0.0 && source.height > 0.0 {
                set_crop_widgets(
                    Crop {
                        x: 0.0,
                        y: 0.0,
                        width: source.width,
                        height: source.height,
                    },
                    &crop_x,
                    &crop_y,
                    &crop_w,
                    &crop_h,
                );
            }
        }
    });
    crop_overlay.connect_button_press_event({
        let crop_enabled = crop_enabled.clone();
        let source_size = source_size.clone();
        let crop_drag = crop_drag.clone();
        let crop_x = crop_x.clone();
        let crop_y = crop_y.clone();
        let crop_w = crop_w.clone();
        let crop_h = crop_h.clone();
        move |area, event| {
            if event.button() != 1 || !crop_enabled.is_active() {
                return glib::Propagation::Proceed;
            }
            let source = source_size.get();
            let Some(fit) = MediaFit::new(
                Size {
                    width: f64::from(area.allocated_width()),
                    height: f64::from(area.allocated_height()),
                },
                source,
            ) else {
                return glib::Propagation::Proceed;
            };
            let (x, y) = event.position();
            let pointer = Point { x, y };
            if !fit.contains_view(pointer) {
                return glib::Propagation::Stop;
            }
            let initial = bounded_crop(
                crop_from_widgets(&crop_x, &crop_y, &crop_w, &crop_h),
                source,
            );
            *crop_drag.borrow_mut() = Some(CropDrag {
                handle: crop_handle_at(initial, fit, pointer),
                start: fit.view_to_source(pointer),
                initial,
            });
            glib::Propagation::Stop
        }
    });
    crop_overlay.connect_motion_notify_event({
        let source_size = source_size.clone();
        let crop_drag = crop_drag.clone();
        let crop_x = crop_x.clone();
        let crop_y = crop_y.clone();
        let crop_w = crop_w.clone();
        let crop_h = crop_h.clone();
        move |area, event| {
            let Some(drag) = *crop_drag.borrow() else {
                return glib::Propagation::Proceed;
            };
            let source = source_size.get();
            let Some(fit) = MediaFit::new(
                Size {
                    width: f64::from(area.allocated_width()),
                    height: f64::from(area.allocated_height()),
                },
                source,
            ) else {
                return glib::Propagation::Proceed;
            };
            let (x, y) = event.position();
            let current = fit.view_to_source(Point { x, y });
            set_crop_widgets(
                if aspect_locked.is_active() {
                    crop_after_locked_drag(drag, current, source)
                } else {
                    crop_after_drag(drag, current, source)
                },
                &crop_x,
                &crop_y,
                &crop_w,
                &crop_h,
            );
            glib::Propagation::Stop
        }
    });
    crop_overlay.connect_button_release_event({
        let crop_drag = crop_drag.clone();
        move |_, event| {
            if event.button() == 1 && crop_drag.borrow_mut().take().is_some() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });

    let timeline_drag = Rc::new(Cell::new(None::<TimelineDrag>));
    let timeline_keyboard_target = Rc::new(Cell::new(TimelineDrag::Start));
    timeline_interaction.connect_draw({
        let duration_ms = duration_ms.clone();
        let trim_start_ms = trim_start_ms.clone();
        let trim_end_ms = trim_end_ms.clone();
        let position = position.clone();
        let keyboard_target = timeline_keyboard_target.clone();
        move |area, context| {
            let width = f64::from(area.allocated_width());
            let height = f64::from(area.allocated_height());
            let duration = duration_ms.get().max(1) as f64;
            let start_x = width * trim_start_ms.get() as f64 / duration;
            let end_x = width * trim_end_ms.get() as f64 / duration;
            let playhead_x = width * position.value() / duration;
            context.set_source_rgba(0.0, 0.0, 0.0, 0.58);
            context.rectangle(0.0, 0.0, start_x, height);
            context.rectangle(end_x, 0.0, (width - end_x).max(0.0), height);
            let _ = context.fill();
            let accent = area
                .style_context()
                .lookup_color("captures_accent")
                .unwrap_or_else(|| gtk::gdk::RGBA::new(0.10, 0.58, 0.96, 1.0));
            context.set_source_rgba(
                f64::from(accent.red()),
                f64::from(accent.green()),
                f64::from(accent.blue()),
                1.0,
            );
            for x in [start_x, end_x] {
                context.rectangle(
                    (x - 4.0).clamp(0.0, (width - 8.0).max(0.0)),
                    0.0,
                    8.0,
                    height,
                );
                let _ = context.fill();
                context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                context.rectangle(
                    (x - 1.0).clamp(0.0, (width - 2.0).max(0.0)),
                    height / 2.0 - 8.0,
                    2.0,
                    16.0,
                );
                let _ = context.fill();
                context.set_source_rgba(
                    f64::from(accent.red()),
                    f64::from(accent.green()),
                    f64::from(accent.blue()),
                    1.0,
                );
            }
            context.set_source_rgba(1.0, 1.0, 1.0, 0.96);
            context.set_line_width(2.0);
            context.move_to(playhead_x, 0.0);
            context.line_to(playhead_x, height);
            let _ = context.stroke();
            if area.has_focus() {
                let focus_x = match keyboard_target.get() {
                    TimelineDrag::Start => start_x,
                    TimelineDrag::End => end_x,
                    TimelineDrag::Scrub => playhead_x,
                };
                context.set_source_rgba(
                    f64::from(accent.red()),
                    f64::from(accent.green()),
                    f64::from(accent.blue()),
                    1.0,
                );
                context.set_line_width(2.0);
                context.rectangle(
                    (focus_x - 7.0).clamp(0.0, (width - 14.0).max(0.0)),
                    2.0,
                    14.0,
                    (height - 4.0).max(1.0),
                );
                let _ = context.stroke();
            }
            glib::Propagation::Proceed
        }
    });
    timeline_interaction.connect_button_press_event({
        let duration_ms = duration_ms.clone();
        let trim_start_ms = trim_start_ms.clone();
        let trim_end_ms = trim_end_ms.clone();
        let timeline_drag = timeline_drag.clone();
        let keyboard_target = timeline_keyboard_target.clone();
        let trim_start = trim_start.clone();
        let trim_end = trim_end.clone();
        let position = position.clone();
        let playback = playback_slot.clone();
        move |area, event| {
            if event.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (x, _) = event.position();
            let width = f64::from(area.allocated_width()).max(1.0);
            let duration = duration_ms.get().max(1);
            let start_x = width * trim_start_ms.get() as f64 / duration as f64;
            let end_x = width * trim_end_ms.get() as f64 / duration as f64;
            let drag = if (x - start_x).abs() <= TRIM_HIT_RADIUS {
                TimelineDrag::Start
            } else if (x - end_x).abs() <= TRIM_HIT_RADIUS {
                TimelineDrag::End
            } else {
                TimelineDrag::Scrub
            };
            timeline_drag.set(Some(drag));
            keyboard_target.set(drag);
            area.grab_focus();
            apply_timeline_drag(drag, x, width, duration, &trim_start, &trim_end, &position);
            if let Some(playback) = playback.borrow().as_ref() {
                playback.seek(position.value().max(0.0) as u64);
            }
            glib::Propagation::Stop
        }
    });
    timeline_interaction.connect_motion_notify_event({
        let duration_ms = duration_ms.clone();
        let timeline_drag = timeline_drag.clone();
        let trim_start = trim_start.clone();
        let trim_end = trim_end.clone();
        let position = position.clone();
        let playback = playback_slot.clone();
        move |area, event| {
            let Some(drag) = timeline_drag.get() else {
                return glib::Propagation::Proceed;
            };
            let (x, _) = event.position();
            apply_timeline_drag(
                drag,
                x,
                f64::from(area.allocated_width()).max(1.0),
                duration_ms.get().max(1),
                &trim_start,
                &trim_end,
                &position,
            );
            if let Some(playback) = playback.borrow().as_ref() {
                playback.seek(position.value().max(0.0) as u64);
            }
            glib::Propagation::Stop
        }
    });
    timeline_interaction.connect_button_release_event({
        let timeline_drag = timeline_drag.clone();
        move |_, event| {
            if event.button() == 1 && timeline_drag.replace(None).is_some() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    timeline_interaction.connect_focus_in_event({
        let keyboard_target = timeline_keyboard_target.clone();
        move |area, _| {
            set_timeline_accessible_name(area, keyboard_target.get());
            area.queue_draw();
            glib::Propagation::Proceed
        }
    });
    timeline_interaction.connect_focus_out_event(|area, _| {
        area.queue_draw();
        glib::Propagation::Proceed
    });
    timeline_interaction.connect_key_press_event({
        let target = timeline_keyboard_target.clone();
        let duration = duration_ms.clone();
        let start = trim_start.clone();
        let end = trim_end.clone();
        let position = position.clone();
        let playback = playback_slot.clone();
        move |area, event| {
            let shift = event.state().contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if event.keyval() == gtk::gdk::Key::Tab || event.keyval() == gtk::gdk::Key::ISO_Left_Tab
            {
                let next = match (target.get(), shift) {
                    (TimelineDrag::Start, false) => Some(TimelineDrag::End),
                    (TimelineDrag::End, false) => Some(TimelineDrag::Scrub),
                    (TimelineDrag::Scrub, true) => Some(TimelineDrag::End),
                    (TimelineDrag::End, true) => Some(TimelineDrag::Start),
                    _ => None,
                };
                if let Some(next) = next {
                    target.set(next);
                    set_timeline_accessible_name(area, next);
                    area.queue_draw();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            let duration = duration.get().max(1);
            let step = (duration / 100).max(1);
            let large_step = (duration / 10).max(1);
            let current = match target.get() {
                TimelineDrag::Start => start.value().max(0.0) as u64,
                TimelineDrag::End => end.value().max(0.0) as u64,
                TimelineDrag::Scrub => position.value().max(0.0) as u64,
            };
            let next = match event.keyval() {
                gtk::gdk::Key::Left | gtk::gdk::Key::Down => Some(current.saturating_sub(step)),
                gtk::gdk::Key::Right | gtk::gdk::Key::Up => Some(current.saturating_add(step)),
                gtk::gdk::Key::Page_Down => Some(current.saturating_sub(large_step)),
                gtk::gdk::Key::Page_Up => Some(current.saturating_add(large_step)),
                gtk::gdk::Key::Home => Some(match target.get() {
                    TimelineDrag::End => start.value().max(0.0) as u64 + 1,
                    _ => 0,
                }),
                gtk::gdk::Key::End => Some(match target.get() {
                    TimelineDrag::Start => (end.value().max(1.0) as u64).saturating_sub(1),
                    _ => duration,
                }),
                _ => None,
            };
            let Some(next) = next else {
                return glib::Propagation::Proceed;
            };
            match target.get() {
                TimelineDrag::Start => {
                    let next = next.min((end.value().max(1.0) as u64).saturating_sub(1));
                    start.set_value(next as f64);
                    position.set_value(next as f64);
                }
                TimelineDrag::End => {
                    let next = next.clamp(start.value().max(0.0) as u64 + 1, duration);
                    end.set_value(next as f64);
                    position.set_value(next as f64);
                }
                TimelineDrag::Scrub => {
                    let next =
                        next.clamp(start.value().max(0.0) as u64, end.value().max(1.0) as u64);
                    position.set_value(next as f64);
                }
            }
            if let Some(playback) = playback.borrow().as_ref() {
                playback.seek(position.value().max(0.0) as u64);
            }
            area.queue_draw();
            glib::Propagation::Stop
        }
    });
    resolution.connect_changed({
        let crop_enabled = crop_enabled.clone();
        let crop_w = crop_w.clone();
        let crop_h = crop_h.clone();
        let out_w = out_w.clone();
        let out_h = out_h.clone();
        move |resolution| {
            let source_width = if crop_enabled.is_active() {
                crop_w.value()
            } else {
                crop_w.adjustment().upper()
            };
            let source_height = if crop_enabled.is_active() {
                crop_h.value()
            } else {
                crop_h.adjustment().upper()
            };
            let maximum_height = match resolution.active_id().as_deref() {
                Some("1080") => Some(1080.0),
                Some("720") => Some(720.0),
                Some("original") => Some(source_height),
                _ => None,
            };
            if let Some(height) = maximum_height {
                let height = height.min(source_height).max(2.0).round() as i32 & !1;
                let width =
                    ((source_width * f64::from(height) / source_height).round() as i32).max(2) & !1;
                out_w.set_value(f64::from(width));
                out_h.set_value(f64::from(height));
            }
        }
    });
    maximum_enabled.connect_toggled({
        let field = maximum_mb.clone();
        move |toggle| field.set_sensitive(toggle.is_active())
    });
    format.connect_changed({
        let gif_fps = gif_fps.clone();
        let quality = quality.clone();
        let window = window.clone();
        move |format| {
            let id = format.active_id().unwrap_or_default();
            if id.as_str() == "webm" {
                ui::error(&window, "WebM export is not available in the shipping Linux media backend. Choose MP4 or GIF.");
                format.set_active_id(Some("mp4"));
                return;
            }
            gif_fps.set_sensitive(id.as_str() == "gif");
            if id.as_str() == "gif" && quality.active_id().as_deref() == Some("preserve") {
                quality.set_active_id(Some("highest"));
            }
        }
    });

    let refresh_audio = {
        let playback = playback_slot.clone();
        let source_audio = source_audio.clone();
        let system_volume = system_volume.clone();
        let microphone_volume = microphone_volume.clone();
        let mute_system = mute_system.clone();
        let mute_microphone = mute_microphone.clone();
        Rc::new(move || {
            if let Some(playback) = playback.borrow().as_ref() {
                playback.set_audio(playback_audio(
                    source_audio.get(),
                    &system_volume,
                    &microphone_volume,
                    &mute_system,
                    &mute_microphone,
                ));
            }
        })
    };
    system_volume.connect_value_changed({
        let refresh = refresh_audio.clone();
        move |_| refresh()
    });
    microphone_volume.connect_value_changed({
        let refresh = refresh_audio.clone();
        move |_| refresh()
    });
    mute_system.connect_toggled({
        let refresh = refresh_audio.clone();
        move |_| refresh()
    });
    mute_microphone.connect_toggled({
        let refresh = refresh_audio.clone();
        move |_| refresh()
    });

    let active_cancel = Rc::new(RefCell::new(None::<CancelToken>));
    choose_destination.connect_clicked({
        let window = window.clone();
        let destination = destination.clone();
        let label = destination_label.clone();
        move |_| {
            let (destination, label) = (destination.clone(), label.clone());
            let initial = destination.borrow().clone();
            ui::choose_folder(&window, "Save recording to", &initial, move |path| {
                label.set_text(&format!("Saving to  {}", path.display()));
                *destination.borrow_mut() = path;
            });
        }
    });
    compare.connect_clicked({
        let playback = playback_slot.clone();
        let crop_overlay = crop_overlay.clone();
        move |_| {
            // Comparison images already include the crop. Drawing the original
            // source-space handles over them would misrepresent the result.
            crop_overlay.hide();
            if let Some(playback) = playback.borrow().as_ref() {
                playback.pause();
            }
        }
    });
    connect_compare(
        &compare,
        &comparison_overlay,
        &comparison_handle,
        &play_overlay,
        &preview_overlay,
        &comparison_images,
        &status,
        &path,
        &scratch,
        &position,
        &trim_start_ms,
        &trim_end_ms,
        &crop_enabled,
        &crop_x,
        &crop_y,
        &crop_w,
        &crop_h,
        &resolution,
        &out_w,
        &out_h,
        &format,
        &quality,
        &gif_fps,
        &maximum_enabled,
        &maximum_mb,
        &system_volume,
        &microphone_volume,
        &mute_system,
        &mute_microphone,
        &mono,
        &source_audio,
        &duration_ms,
        &closed,
    );
    connect_export(
        &export,
        &cancel,
        &status,
        &window,
        &path,
        &destination,
        &filename,
        on_saved,
        &trim_start_ms,
        &trim_end_ms,
        &crop_enabled,
        &crop_x,
        &crop_y,
        &crop_w,
        &crop_h,
        &resolution,
        &out_w,
        &out_h,
        &format,
        &quality,
        &gif_fps,
        &maximum_enabled,
        &maximum_mb,
        &system_volume,
        &microphone_volume,
        &mute_system,
        &mute_microphone,
        &mono,
        &source_audio,
        &active_cancel,
        &closed,
    );
    cancel.connect_clicked({
        let active_cancel = active_cancel.clone();
        move |_| {
            if let Some(token) = active_cancel.borrow().as_ref() {
                token.cancel();
            }
        }
    });
    window.connect_key_press_event({
        let play = play.clone();
        move |_, event| {
            if event.keyval() == gtk::gdk::Key::space {
                play.clicked();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    window.connect_close_request({
        let closed = closed.clone();
        let active_cancel = active_cancel.clone();
        let playback = playback_slot.clone();
        move |_| {
            closed.set(true);
            if let Some(token) = active_cancel.borrow().as_ref() {
                token.cancel();
            }
            if let Some(playback) = playback.borrow().as_ref() {
                playback.pause();
            }
            let _ = fs::remove_dir_all(&scratch);
            glib::Propagation::Proceed
        }
    });
}

fn crop_from_widgets(
    x: &gtk::SpinButton,
    y: &gtk::SpinButton,
    width: &gtk::SpinButton,
    height: &gtk::SpinButton,
) -> Crop {
    Crop {
        x: x.value(),
        y: y.value(),
        width: width.value(),
        height: height.value(),
    }
}

fn set_crop_widgets(
    crop: Crop,
    x: &gtk::SpinButton,
    y: &gtk::SpinButton,
    width: &gtk::SpinButton,
    height: &gtk::SpinButton,
) {
    x.set_value(crop.x);
    y.set_value(crop.y);
    width.set_value(crop.width);
    height.set_value(crop.height);
}

fn crop_handle_points(crop: Crop, fit: MediaFit) -> [Point; 8] {
    let left = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y,
        })
        .x;
    let right = fit
        .source_to_view(Point {
            x: crop.x + crop.width,
            y: crop.y,
        })
        .x;
    let top = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y,
        })
        .y;
    let bottom = fit
        .source_to_view(Point {
            x: crop.x,
            y: crop.y + crop.height,
        })
        .y;
    let center_x = (left + right) / 2.0;
    let center_y = (top + bottom) / 2.0;
    [
        Point { x: left, y: top },
        Point {
            x: center_x,
            y: top,
        },
        Point { x: right, y: top },
        Point {
            x: right,
            y: center_y,
        },
        Point {
            x: right,
            y: bottom,
        },
        Point {
            x: center_x,
            y: bottom,
        },
        Point { x: left, y: bottom },
        Point {
            x: left,
            y: center_y,
        },
    ]
}

fn apply_timeline_drag(
    drag: TimelineDrag,
    x: f64,
    width: f64,
    duration_ms: u64,
    trim_start: &gtk::Scale,
    trim_end: &gtk::Scale,
    position: &gtk::Scale,
) {
    let (start, end, playhead) = trim_after_drag(
        drag,
        x,
        width,
        duration_ms,
        trim_start.value().max(0.0) as u64,
        trim_end.value().max(1.0) as u64,
    );
    trim_start.set_value(start as f64);
    trim_end.set_value(end as f64);
    position.set_value(playhead as f64);
}

#[allow(clippy::too_many_arguments)]
fn connect_export(
    button: &gtk::Button,
    cancel: &gtk::Button,
    status: &gtk::Label,
    window: &gtk::Window,
    path: &Path,
    directory: &Rc<RefCell<PathBuf>>,
    filename: &gtk::Entry,
    on_saved: Rc<dyn Fn(PathBuf)>,
    trim_start: &Rc<Cell<u64>>,
    trim_end: &Rc<Cell<u64>>,
    crop_enabled: &gtk::CheckButton,
    crop_x: &gtk::SpinButton,
    crop_y: &gtk::SpinButton,
    crop_w: &gtk::SpinButton,
    crop_h: &gtk::SpinButton,
    resolution: &gtk::ComboBoxText,
    out_w: &gtk::SpinButton,
    out_h: &gtk::SpinButton,
    format: &gtk::ComboBoxText,
    quality: &gtk::ComboBoxText,
    gif_fps: &gtk::SpinButton,
    maximum_enabled: &gtk::CheckButton,
    maximum_mb: &gtk::SpinButton,
    system_volume: &gtk::Scale,
    microphone_volume: &gtk::Scale,
    mute_system: &gtk::CheckButton,
    mute_microphone: &gtk::CheckButton,
    mono: &gtk::CheckButton,
    source_audio: &Rc<Cell<SourceAudio>>,
    active_cancel: &Rc<RefCell<Option<CancelToken>>>,
    closed: &Rc<Cell<bool>>,
) {
    let values = ExportWidgets::new(
        trim_start,
        trim_end,
        crop_enabled,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
        resolution,
        out_w,
        out_h,
        format,
        quality,
        gif_fps,
        maximum_enabled,
        maximum_mb,
        system_volume,
        microphone_volume,
        mute_system,
        mute_microphone,
        mono,
        source_audio,
    );
    let (button, cancel, status, window, path, directory, filename, active_cancel, closed) = (
        button.clone(),
        cancel.clone(),
        status.clone(),
        window.clone(),
        path.to_path_buf(),
        directory.clone(),
        filename.clone(),
        active_cancel.clone(),
        closed.clone(),
    );
    button.clone().connect_clicked(move |_| {
        if active_cancel.borrow().is_some() {
            return;
        }
        let (edit, spec, extension) = values.spec();
        let destination = match named_output(
            directory.borrow().as_path(),
            filename.text().as_str(),
            extension,
        ) {
            Ok(value) => value,
            Err(error) => {
                ui::error(&window, &error);
                return;
            }
        };
        let token = CancelToken::default();
        *active_cancel.borrow_mut() = Some(token.clone());
        button.set_sensitive(false);
        cancel.set_sensitive(true);
        cancel.set_opacity(1.0);
        status.set_text("Exporting…");
        let (window, button, cancel, status, active_cancel, closed, on_saved, path) = (
            window.clone(),
            button.clone(),
            cancel.clone(),
            status.clone(),
            active_cancel.clone(),
            closed.clone(),
            on_saved.clone(),
            path.clone(),
        );
        ui::job(
            move || {
                MediaToolchain::from_command_names()
                    .export(&path, &destination.path, &edit, &spec, &token, |_| {})
                    .map_err(|error| error.to_string())?;
                if token.is_cancelled() {
                    return Err("Export cancelled".into());
                }
                destination.commit()
            },
            move |result| {
                *active_cancel.borrow_mut() = None;
                if closed.get() {
                    return;
                }
                button.set_sensitive(true);
                cancel.set_sensitive(false);
                cancel.set_opacity(0.0);
                match result {
                    Ok(path) => {
                        status.set_text("Export complete");
                        on_saved(path);
                    }
                    Err(error) => {
                        status.set_text("Export failed — draft and source are unchanged");
                        ui::error(&window, &error);
                    }
                }
            },
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn connect_compare(
    button: &gtk::Button,
    comparison: &gtk::DrawingArea,
    comparison_handle: &gtk::Button,
    play: &gtk::Box,
    preview_overlay: &gtk::Overlay,
    comparison_images: &Rc<RefCell<Option<(gtk::gdk_pixbuf::Pixbuf, gtk::gdk_pixbuf::Pixbuf)>>>,
    status: &gtk::Label,
    path: &Path,
    scratch: &Path,
    position: &gtk::Scale,
    trim_start: &Rc<Cell<u64>>,
    trim_end: &Rc<Cell<u64>>,
    crop_enabled: &gtk::CheckButton,
    crop_x: &gtk::SpinButton,
    crop_y: &gtk::SpinButton,
    crop_w: &gtk::SpinButton,
    crop_h: &gtk::SpinButton,
    resolution: &gtk::ComboBoxText,
    out_w: &gtk::SpinButton,
    out_h: &gtk::SpinButton,
    format: &gtk::ComboBoxText,
    quality: &gtk::ComboBoxText,
    gif_fps: &gtk::SpinButton,
    maximum_enabled: &gtk::CheckButton,
    maximum_mb: &gtk::SpinButton,
    system_volume: &gtk::Scale,
    microphone_volume: &gtk::Scale,
    mute_system: &gtk::CheckButton,
    mute_microphone: &gtk::CheckButton,
    mono: &gtk::CheckButton,
    source_audio: &Rc<Cell<SourceAudio>>,
    duration: &Rc<Cell<u64>>,
    closed: &Rc<Cell<bool>>,
) {
    let values = ExportWidgets::new(
        trim_start,
        trim_end,
        crop_enabled,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
        resolution,
        out_w,
        out_h,
        format,
        quality,
        gif_fps,
        maximum_enabled,
        maximum_mb,
        system_volume,
        microphone_volume,
        mute_system,
        mute_microphone,
        mono,
        source_audio,
    );
    let (
        button,
        comparison,
        comparison_handle,
        play,
        preview_overlay,
        comparison_images,
        status,
        path,
        scratch,
        position,
        duration,
        closed,
    ) = (
        button.clone(),
        comparison.clone(),
        comparison_handle.clone(),
        play.clone(),
        preview_overlay.clone(),
        comparison_images.clone(),
        status.clone(),
        path.to_path_buf(),
        scratch.to_path_buf(),
        position.clone(),
        duration.clone(),
        closed.clone(),
    );
    button.clone().connect_clicked(move |_| {
        let (mut edit, spec, extension) = values.spec();
        let at = (position.value() as u64).clamp(
            edit.trim_start_ms,
            edit.trim_end_ms.unwrap_or(duration.get()).saturating_sub(1),
        );
        let sample_start = at.saturating_sub(500).max(edit.trim_start_ms);
        edit.trim_start_ms = sample_start;
        edit.trim_end_ms = Some((sample_start + 1_000).min(values.trim_end.get()));
        let before_path = scratch.join("compare-before.png");
        let before_sample_path = scratch.join(format!("compare-before-sample.{extension}"));
        let sample_path = scratch.join(format!("compare-sample.{extension}"));
        let after_path = scratch.join("compare-after.png");
        button.set_sensitive(false);
        comparison.set_no_show_all(false);
        status.set_text("Building compression comparison…");
        let (
            button,
            comparison,
            comparison_handle,
            play,
            preview_overlay,
            comparison_images,
            status,
            closed,
        ) = (
            button.clone(),
            comparison.clone(),
            comparison_handle.clone(),
            play.clone(),
            preview_overlay.clone(),
            comparison_images.clone(),
            status.clone(),
            closed.clone(),
        );
        let path = path.clone();
        let duration = duration.clone();
        ui::job(
            move || {
                let media = MediaToolchain::from_command_names();
                let token = CancelToken::default();
                let before_spec = ExportSpec {
                    quality: QualityPreset::Preserve,
                    max_size_bytes: None,
                    ..spec.clone()
                };
                media
                    .export(
                        &path,
                        &before_sample_path,
                        &edit,
                        &before_spec,
                        &token,
                        |_| {},
                    )
                    .map_err(|e| e.to_string())?;
                let outcome = media
                    .export(&path, &sample_path, &edit, &spec, &token, |_| {})
                    .map_err(|e| e.to_string())?;
                media
                    .extract_frame(&before_sample_path, at - sample_start, &before_path, &token)
                    .map_err(|e| e.to_string())?;
                media
                    .extract_frame(&sample_path, at - sample_start, &after_path, &token)
                    .map_err(|e| e.to_string())?;
                let _ = fs::remove_file(before_sample_path);
                let _ = fs::remove_file(sample_path);
                Ok((
                    before_path,
                    after_path,
                    outcome.size_bytes,
                    edit.trim_end_ms.unwrap_or(sample_start) - sample_start,
                ))
            },
            move |result| {
                button.set_sensitive(true);
                if closed.get() {
                    return;
                }
                match result {
                    Ok((before_path, after_path, bytes, sample_ms)) => {
                        // Playback can dismiss stills while encoding is pending.
                        if !comparison.is_no_show_all()
                            && let (Ok(before), Ok(after)) = (
                                gtk::gdk_pixbuf::Pixbuf::from_file(&before_path),
                                gtk::gdk_pixbuf::Pixbuf::from_file(&after_path),
                            )
                        {
                            *comparison_images.borrow_mut() = Some((before, after));
                            comparison.set_no_show_all(false);
                            comparison_handle.set_no_show_all(false);
                            comparison.show();
                            comparison_handle.show();
                            play.set_valign(gtk::Align::End);
                            play.set_margin_bottom(60);
                            preview_overlay.reorder_overlay(&play, -1);
                            preview_overlay.reorder_overlay(&comparison_handle, -1);
                            comparison.queue_draw();
                        }
                        let projected = bytes.saturating_mul(duration.get()) / sample_ms.max(1);
                        status.set_text(&format!(
                            "Comparison ready · estimated export {}",
                            format_bytes(projected)
                        ));
                        let _ = fs::remove_file(before_path);
                        let _ = fs::remove_file(after_path);
                    }
                    Err(error) => status.set_text(&format!("Comparison failed: {error}")),
                }
            },
        );
    });
}

#[derive(Clone)]
struct ExportWidgets {
    trim_start: Rc<Cell<u64>>,
    trim_end: Rc<Cell<u64>>,
    crop_enabled: gtk::CheckButton,
    crop_x: gtk::SpinButton,
    crop_y: gtk::SpinButton,
    crop_w: gtk::SpinButton,
    crop_h: gtk::SpinButton,
    resolution: gtk::ComboBoxText,
    out_w: gtk::SpinButton,
    out_h: gtk::SpinButton,
    format: gtk::ComboBoxText,
    quality: gtk::ComboBoxText,
    gif_fps: gtk::SpinButton,
    maximum_enabled: gtk::CheckButton,
    maximum_mb: gtk::SpinButton,
    system_volume: gtk::Scale,
    microphone_volume: gtk::Scale,
    mute_system: gtk::CheckButton,
    mute_microphone: gtk::CheckButton,
    mono: gtk::CheckButton,
    source_audio: Rc<Cell<SourceAudio>>,
}
impl ExportWidgets {
    #[allow(clippy::too_many_arguments)]
    fn new(
        trim_start: &Rc<Cell<u64>>,
        trim_end: &Rc<Cell<u64>>,
        crop_enabled: &gtk::CheckButton,
        crop_x: &gtk::SpinButton,
        crop_y: &gtk::SpinButton,
        crop_w: &gtk::SpinButton,
        crop_h: &gtk::SpinButton,
        resolution: &gtk::ComboBoxText,
        out_w: &gtk::SpinButton,
        out_h: &gtk::SpinButton,
        format: &gtk::ComboBoxText,
        quality: &gtk::ComboBoxText,
        gif_fps: &gtk::SpinButton,
        maximum_enabled: &gtk::CheckButton,
        maximum_mb: &gtk::SpinButton,
        system_volume: &gtk::Scale,
        microphone_volume: &gtk::Scale,
        mute_system: &gtk::CheckButton,
        mute_microphone: &gtk::CheckButton,
        mono: &gtk::CheckButton,
        source_audio: &Rc<Cell<SourceAudio>>,
    ) -> Self {
        Self {
            trim_start: trim_start.clone(),
            trim_end: trim_end.clone(),
            crop_enabled: crop_enabled.clone(),
            crop_x: crop_x.clone(),
            crop_y: crop_y.clone(),
            crop_w: crop_w.clone(),
            crop_h: crop_h.clone(),
            resolution: resolution.clone(),
            out_w: out_w.clone(),
            out_h: out_h.clone(),
            format: format.clone(),
            quality: quality.clone(),
            gif_fps: gif_fps.clone(),
            maximum_enabled: maximum_enabled.clone(),
            maximum_mb: maximum_mb.clone(),
            system_volume: system_volume.clone(),
            microphone_volume: microphone_volume.clone(),
            mute_system: mute_system.clone(),
            mute_microphone: mute_microphone.clone(),
            mono: mono.clone(),
            source_audio: source_audio.clone(),
        }
    }
    fn spec(&self) -> (EditSpec, ExportSpec, &'static str) {
        let format = if self.format.active_id().as_deref() == Some("gif") {
            ExportFormat::Gif
        } else {
            ExportFormat::Mp4
        };
        let quality = match self.quality.active_id().as_deref() {
            Some("highest") => QualityPreset::Highest,
            Some("high") => QualityPreset::High,
            Some("standard") => QualityPreset::Standard,
            Some("small") => QualityPreset::Small,
            Some("tiny") => QualityPreset::Tiny,
            _ => QualityPreset::Preserve,
        };
        let resolution = self.resolution.active_id().unwrap_or_default();
        let (output_width, output_height) = if resolution.as_str() == "original" {
            (None, None)
        } else {
            (
                Some(self.out_w.value_as_int() as u32),
                Some(self.out_h.value_as_int() as u32),
            )
        };
        let audio = self.source_audio.get();
        (
            EditSpec {
                trim_start_ms: self.trim_start.get(),
                trim_end_ms: Some(self.trim_end.get()),
                crop: self.crop_enabled.is_active().then(|| CropRect {
                    x: self.crop_x.value_as_int() as u32,
                    y: self.crop_y.value_as_int() as u32,
                    width: self.crop_w.value_as_int() as u32,
                    height: self.crop_h.value_as_int() as u32,
                }),
                output_width,
                output_height,
                audio: AudioEdit {
                    system_volume: (self.system_volume.value() / 100.0) as f32,
                    microphone_volume: (self.microphone_volume.value() / 100.0) as f32,
                    mute_system_audio: format == ExportFormat::Gif || self.mute_system.is_active(),
                    mute_microphone: format == ExportFormat::Gif
                        || self.mute_microphone.is_active(),
                    mono_output: self.mono.is_active(),
                    source_has_system_audio: audio.system,
                    source_has_microphone_audio: audio.microphone,
                },
            },
            ExportSpec {
                format,
                quality,
                max_size_bytes: self
                    .maximum_enabled
                    .is_active()
                    .then(|| (self.maximum_mb.value() * 1_000_000.0) as u64),
                frames_per_second: (format == ExportFormat::Gif)
                    .then(|| self.gif_fps.value_as_int() as u16),
                gif_max_colors: (format == ExportFormat::Gif).then(|| gif_colors(quality)),
            },
            if format == ExportFormat::Gif {
                "gif"
            } else {
                "mp4"
            },
        )
    }
}

fn playback_audio(
    source: SourceAudio,
    system: &gtk::Scale,
    microphone: &gtk::Scale,
    mute_system: &gtk::CheckButton,
    mute_microphone: &gtk::CheckButton,
) -> AudioPlayback {
    AudioPlayback {
        system_stream: source.system,
        microphone_stream: source.microphone,
        system_volume: system.value() / 100.0,
        microphone_volume: microphone.value() / 100.0,
        mute_system: mute_system.is_active(),
        mute_microphone: mute_microphone.is_active(),
    }
}
fn inspect_audio_streams(path: &PathBuf, count: usize) -> SourceAudio {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream_tags=title",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .ok();
    let text = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    SourceAudio {
        system: count > 0 && (count > 1 || !text.contains("microphone")),
        microphone: count > 1 || text.contains("microphone"),
    }
}
fn gif_colors(quality: QualityPreset) -> u16 {
    match quality {
        QualityPreset::Tiny => 64,
        QualityPreset::Small => 96,
        QualityPreset::Standard => 128,
        _ => 256,
    }
}
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes / 1_000)
    }
}
fn editor_time(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    let seconds = (milliseconds % 60_000) as f64 / 1_000.0;
    format!("{minutes}:{seconds:04.1}")
}
fn install_editor_styles(window: &gtk::Window) {
    let has_product_palette = window
        .style_context()
        .lookup_color("captures_surface")
        .is_some();
    let (surface, raised, sunken, text, border) = if has_product_palette {
        (
            "@captures_surface",
            "@captures_raised",
            "@captures_sunken",
            "@captures_text",
            "@captures_border",
        )
    } else {
        (
            "@theme_bg_color",
            "@theme_base_color",
            "@theme_base_color",
            "@theme_fg_color",
            "@borders",
        )
    };
    let provider = gtk::CssProvider::new();
    let css = format!(
        r#"
        .recording-editor-root {{
            background-color: {surface};
            color: {text};
        }}
        .recording-editor-root .recording-editor-scroll,
        .recording-editor-root .recording-editor-scroll viewport {{
            background-color: {surface};
            border: none;
        }}
        .recording-editor-root .recording-editor-heading {{
            font-size: 22px;
            font-weight: 600;
        }}
        .recording-editor-root .editor-card {{
            background-color: {raised};
            border: 1px solid {border};
            border-radius: 14px;
            padding: 16px;
        }}
        .recording-editor-root .editor-card > label.title {{
            font-size: 16px;
            font-weight: 600;
        }}
        .recording-editor-root .recording-preview-card {{
            padding: 0;
        }}
        .recording-editor-root .recording-preview-toolbar {{
            min-height: 46px;
            padding-left: 16px;
            padding-right: 16px;
            border-bottom: 1px solid {border};
        }}
        .recording-editor-root .recording-loop-toggle {{
            padding: 0 10px;
        }}
        .recording-editor-root .preview-size-button {{
            min-height: 30px;
            min-width: 58px;
            padding: 0 10px;
            border-radius: 0;
        }}
        .recording-editor-root .preview-size-first {{
            border-radius: 7px 0 0 7px;
        }}
        .recording-editor-root .preview-size-last {{
            border-radius: 0 7px 7px 0;
        }}
        .recording-editor-root .preview-play-button {{
            min-width: 44px;
            min-height: 44px;
            border-radius: 999px;
            color: @captures_accent_ink;
            background-color: @captures_accent;
        }}
        .recording-editor-root .recording-preview-frame {{
            background-color: {sunken};
            border: none;
            border-radius: 0 0 14px 14px;
            padding: 12px;
        }}
        .recording-editor-root .recording-preview-frame scrolledwindow,
        .recording-editor-root .recording-preview-frame viewport {{
            background-color: {sunken};
            border: none;
        }}
        .recording-editor-root .recording-timeline-card {{
            padding: 16px 20px 20px;
        }}
        .recording-editor-root .recording-comparison-handle-hit {{
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            padding: 0;
        }}
        .recording-editor-root .recording-save-footer {{
            min-height: 64px;
            padding-top: 6px;
            padding-bottom: 6px;
            border-top: 1px solid {border};
            background-color: {raised};
        }}
        .recording-editor-root .recording-save-footer entry {{
            min-height: 22px;
            padding-top: 0;
            padding-bottom: 0;
            border-radius: 7px 0 0 7px;
        }}
        .recording-editor-root .recording-save-footer combobox button {{
            min-height: 22px;
            padding-top: 0;
            padding-bottom: 0;
            border-radius: 0 7px 7px 0;
        }}
        .recording-editor-root .recording-save-footer button.primary {{
            min-width: 92px;
        }}
    "#
    );
    provider.load_from_data(&css);
    ui::install_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
}
fn card(title: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 9);
    card.style_context().add_class("editor-card");
    if !title.is_empty() {
        card.pack_start(&ui::label(title, "title"), false, false, 0);
    }
    card
}
fn labeled<W: IsA<gtk::Widget>>(text: &str, widget: &W) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 3);
    row.pack_start(&ui::label(text, "muted"), false, false, 0);
    row.pack_start(widget, false, false, 0);
    row
}
fn accessible_name<W: IsA<gtk::Widget>>(widget: &W, name: &str) {
    ui::named(widget, name);
}
fn scale(name: &str) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    accessible_name(&scale, name);
    scale
}
fn spin(min: f64, max: f64, step: f64, name: &str) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    accessible_name(&spin, name);
    spin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxed_preview_maps_only_the_fitted_media() {
        let fit = MediaFit::new(
            Size {
                width: 600.0,
                height: 600.0,
            },
            Size {
                width: 1_140.0,
                height: 692.0,
            },
        )
        .unwrap();

        assert!((fit.offset.y - 117.894_736_842).abs() < 0.001);
        assert!(!fit.contains_view(Point { x: 300.0, y: 80.0 }));
        let mapped = fit.view_to_source(Point {
            x: fit.offset.x + 421.0 * fit.scale,
            y: fit.offset.y + 317.0 * fit.scale,
        });
        assert!((mapped.x - 421.0).abs() < 0.000_001);
        assert!((mapped.y - 317.0).abs() < 0.000_001);
        assert_eq!(
            fit.source_to_view(Point {
                x: 1_140.0,
                y: 692.0,
            }),
            Point {
                x: 600.0,
                y: fit.offset.y + 692.0 * fit.scale,
            }
        );
    }

    #[test]
    fn reverse_direction_new_crop_is_normalized_in_source_coordinates() {
        let source = Size {
            width: 1_140.0,
            height: 692.0,
        };
        let crop = crop_after_drag(
            CropDrag {
                handle: CropHandle::New,
                start: Point { x: 913.0, y: 601.0 },
                initial: Crop::default(),
            },
            Point { x: 127.0, y: 83.0 },
            source,
        );
        assert_eq!(
            crop,
            Crop {
                x: 127.0,
                y: 83.0,
                width: 786.0,
                height: 518.0,
            }
        );
    }

    #[test]
    fn numeric_width_clamps_at_right_edge_without_moving_origin() {
        let crop = crop_after_numeric_change(
            Crop {
                x: 731.0,
                y: 47.0,
                width: 900.0,
                height: 211.0,
            },
            Size {
                width: 1_140.0,
                height: 692.0,
            },
            2,
        );
        assert_eq!(
            crop,
            Crop {
                x: 731.0,
                y: 47.0,
                width: 409.0,
                height: 211.0,
            }
        );
    }

    #[test]
    fn moving_and_resizing_crop_clamp_without_losing_unaffected_edges() {
        let source = Size {
            width: 1_140.0,
            height: 692.0,
        };
        let initial = Crop {
            x: 101.0,
            y: 53.0,
            width: 407.0,
            height: 211.0,
        };
        let moved = crop_after_drag(
            CropDrag {
                handle: CropHandle::Move,
                start: Point { x: 205.0, y: 107.0 },
                initial,
            },
            Point {
                x: 1_900.0,
                y: 1_400.0,
            },
            source,
        );
        assert_eq!(
            moved,
            Crop {
                x: 733.0,
                y: 481.0,
                ..initial
            }
        );

        let resized = crop_after_drag(
            CropDrag {
                handle: CropHandle::NorthWest,
                start: Point { x: 101.0, y: 53.0 },
                initial,
            },
            Point { x: 500.0, y: 400.0 },
            source,
        );
        assert_eq!(resized.x + resized.width, 508.0);
        assert_eq!(resized.y + resized.height, 264.0);
        assert_eq!(resized.width, 8.0);
        assert_eq!(resized.height, CROP_MIN_SIZE);
    }

    #[test]
    fn direct_timeline_handles_keep_a_nonempty_asymmetric_range() {
        assert_eq!(
            trim_after_drag(TimelineDrag::Start, 940.0, 1_000.0, 9_137, 1_111, 7_003),
            (7_002, 7_003, 7_002)
        );
        assert_eq!(
            trim_after_drag(TimelineDrag::End, -200.0, 1_000.0, 9_137, 1_111, 7_003),
            (1_111, 1_112, 1_112)
        );
        assert_eq!(
            trim_after_drag(TimelineDrag::Scrub, 731.0, 1_000.0, 9_137, 1_111, 7_003),
            (1_111, 7_003, 6_679)
        );
    }
}
