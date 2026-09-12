//! Native GTK/Cairo image editor. Its draft store is intentionally independent from Tauri data.
#[path = "editor/encoder.rs"]
mod encoder;
#[path = "editor/model.rs"]
mod model;

use crate::compat::prelude::*;
use crate::ui;
use gtk::{gdk, glib, prelude::*};
use image::RgbaImage;
use model::*;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    cell::{Cell, RefCell},
    collections::hash_map::DefaultHasher,
    f64::consts::PI,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Crop,
    Text,
    Line,
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Star,
    Arrow,
    Pen,
    RemoveBg,
}
#[derive(Clone, Copy, PartialEq)]
enum EraseMode {
    Wand,
    Erase,
    Restore,
}
enum Gesture {
    Move {
        index: usize,
        start: Point,
        frame: Rect,
        before: Document,
    },
    Resize {
        index: usize,
        start: Rect,
        handle: usize,
        before: Document,
    },
    Rotate {
        index: usize,
        center: Point,
        offset: f64,
        before: Document,
    },
    Draw {
        start: Point,
        points: Vec<Point>,
    },
    Crop {
        start: Point,
    },
    Erase {
        index: usize,
    },
    Pan {
        x: f64,
        y: f64,
        horizontal: f64,
        vertical: f64,
    },
}
struct State {
    doc: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    selected: Option<usize>,
    tool: Tool,
    gesture: Option<Gesture>,
    preview: Option<Layer>,
    zoom: f64,
    color: Color,
    stroke: f64,
    fill_shapes: bool,
    erase_mode: EraseMode,
    brush_softness: f64,
    wand_tolerance: u8,
    wand_contiguous: bool,
    layer_clipboard: Option<(Layer, usize)>,
    space_down: bool,
    dirty: bool,
    source: Option<PathBuf>,
    directory: PathBuf,
    draft: PathBuf,
}
type Refresh = Rc<RefCell<Option<Box<dyn Fn()>>>>;

fn native_data() -> PathBuf {
    crate::settings::data_dir()
}
fn draft_path(source: Option<&Path>, image: &RgbaImage) -> PathBuf {
    let mut h = DefaultHasher::new();
    if let Some(path) = source {
        path.to_string_lossy().hash(&mut h);
    }
    image.width().hash(&mut h);
    image.height().hash(&mut h);
    image.as_raw().hash(&mut h);
    native_data()
        .join("editor-drafts")
        .join(format!("{:016x}.json", h.finish()))
}
fn write_draft(s: &State) -> Result<(), String> {
    let Some(parent) = s.draft.parent() else {
        return Err("invalid draft path".into());
    };
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&s.doc).map_err(|e| e.to_string())?;
    atomic_write_private(&s.draft, &bytes)
}
fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid output path")?;
    // Source Save can target a user's existing directory; never replace that
    // directory's ACL. Only the app-owned draft directory inherits a private ACL.
    #[cfg(target_os = "windows")]
    if parent == native_data().join("editor-drafts") {
        crate::windows::private_directory(parent).map_err(|e| e.to_string())?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid output filename")?;
    for nonce in 0..100_u32 {
        let staging = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&staging) {
            Ok(mut file) => {
                let result = file.write_all(bytes).and_then(|_| file.sync_all());
                drop(file);
                let result = result.and_then(|_| std::fs::rename(&staging, path));
                if result.is_err() {
                    let _ = std::fs::remove_file(&staging);
                }
                return result.map_err(|error| error.to_string());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a private staging file".into())
}
fn changed(s: &mut State) {
    s.dirty = true;
    if let Err(e) = write_draft(s) {
        eprintln!("Editor draft: {e}")
    }
}
fn checkpoint(s: &mut State) {
    s.undo.push(s.doc.clone());
    if s.undo.len() > 50 {
        s.undo.remove(0);
    }
    s.redo.clear()
}
fn undo(s: &mut State, redo: bool) {
    let from = if redo { &mut s.redo } else { &mut s.undo };
    if let Some(d) = from.pop() {
        let old = std::mem::replace(&mut s.doc, d);
        if redo {
            s.undo.push(old)
        } else {
            s.redo.push(old)
        }
        s.selected = None;
        changed(s)
    }
}
fn refresh(refresh: &Refresh, area: &gtk::DrawingArea) {
    area.queue_draw();
    if let Some(f) = refresh.borrow().as_ref() {
        f()
    }
}

fn color(c: gdk::RGBA) -> Color {
    Color(
        f64::from(c.red()),
        f64::from(c.green()),
        f64::from(c.blue()),
        f64::from(c.alpha()),
    )
}
fn button(text: &str, tip: &str) -> gtk::Button {
    let b = ui::button(text);
    b.set_tooltip_text(Some(tip));
    let name = if text.chars().count() > 1 && text.chars().any(char::is_alphabetic) {
        text
    } else {
        tip.split(" (").next().unwrap_or(tip)
    };
    ui::named(&b, name);
    b
}
fn icon_button(label: &str, icon: &str) -> gtk::Button {
    ui::icon_button(label, icon)
}
fn field_label(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.style_context().add_class("editor-export-field");
    column.set_valign(gtk::Align::End);
    column.pack_start(&ui::label(text, "muted"), false, false, 0);
    column.pack_start(widget, false, false, 0);
    column
}
fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.style_context().add_class("editor-property-section");
    let l = ui::label(title, "section-title");
    b.pack_start(&l, false, false, 0);
    b
}
fn clear(container: &gtk::Box) {
    for child in container.children() {
        container.remove(&child);
    }
}

pub fn open(image: RgbaImage, directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    open_impl(image, None, directory, saved)
}
#[allow(dead_code)] // Used by the parent integration to preserve imported-file identity.
pub fn open_file(path: PathBuf, directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    match image::open(&path) {
        Ok(i) => open_impl(i.to_rgba8(), Some(path), directory, saved),
        Err(e) => eprintln!("Open editor file: {e}"),
    }
}

fn open_impl(
    image: RgbaImage,
    source: Option<PathBuf>,
    directory: PathBuf,
    on_saved: Rc<dyn Fn(PathBuf)>,
) {
    let draft = draft_path(source.as_deref(), &image);
    let restored = std::fs::read(&draft)
        .ok()
        .and_then(|b| serde_json::from_slice::<Document>(&b).ok())
        .filter(|d| d.width > 0 && d.height > 0);
    let doc = restored.clone().unwrap_or_else(|| Document::new(image));
    let state = Rc::new(RefCell::new(State {
        doc,
        undo: vec![],
        redo: vec![],
        selected: None,
        tool: Tool::Select,
        gesture: None,
        preview: None,
        zoom: 1.,
        color: color(ui::color("signal")),
        stroke: 4.,
        fill_shapes: false,
        erase_mode: EraseMode::Wand,
        brush_softness: 0.18,
        wand_tolerance: 36,
        wand_contiguous: true,
        layer_clipboard: None,
        space_down: false,
        dirty: restored.is_some(),
        source,
        directory,
        draft,
    }));
    let window = gtk::Window::new();
    window.set_title(Some("Captures — Image editor"));
    window.set_default_size(1280, 840);
    window.style_context().add_class("editor-window");
    install_editor_css();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_halign(gtk::Align::Fill);
    root.style_context().add_class("image-editor");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.style_context().add_class("editor-header");
    let area = gtk::DrawingArea::new();
    area.set_can_focus(true);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_tooltip_text(Some("Screenshot editing canvas"));
    let layers = gtk::Box::new(gtk::Orientation::Vertical, 4);
    layers.set_halign(gtk::Align::Fill);
    layers.set_hexpand(true);
    layers.set_margin_start(2);
    layers.set_margin_end(2);
    let properties = gtk::Box::new(gtk::Orientation::Vertical, 7);
    properties.set_halign(gtk::Align::Fill);
    properties.set_hexpand(true);
    let refresh_cb: Refresh = Rc::new(RefCell::new(None));

    let canvas_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    canvas_toolbar.style_context().add_class("canvas-toolbar");
    canvas_toolbar.set_valign(gtk::Align::Center);
    canvas_toolbar.pack_start(&ui::label("Canvas", "toolbar-label"), false, false, 4);
    let canvas_width = gtk::SpinButton::with_range(1., 16_384., 1.);
    canvas_width.set_value(state.borrow().doc.width as f64);
    canvas_width.set_tooltip_text(Some("Canvas width"));
    canvas_width.set_size_request(72, 28);
    let canvas_height = gtk::SpinButton::with_range(1., 16_384., 1.);
    canvas_height.set_value(state.borrow().doc.height as f64);
    canvas_height.set_tooltip_text(Some("Canvas height"));
    canvas_height.set_size_request(72, 28);
    canvas_toolbar.pack_start(&ui::label("W", "canvas-dimensions"), false, false, 2);
    canvas_toolbar.pack_start(&canvas_width, false, false, 0);
    canvas_toolbar.pack_start(&ui::label("×  H", "canvas-dimensions"), false, false, 3);
    canvas_toolbar.pack_start(&canvas_height, false, false, 0);
    let toolbar_split = gtk::Separator::new(gtk::Orientation::Vertical);
    toolbar_split.style_context().add_class("toolbar-split");
    canvas_toolbar.pack_start(&toolbar_split, false, false, 4);
    let trim = icon_button("Trim edges", "trim");
    trim.set_label("Trim edges");
    trim.style_context().add_class("canvas-tool");
    canvas_toolbar.pack_start(&trim, false, false, 0);
    header.pack_start(&canvas_toolbar, false, false, 0);

    let header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.pack_start(&header_spacer, true, true, 0);
    let undo_b = icon_button("Undo", "undo");
    let redo_b = icon_button("Redo", "redo");
    undo_b.set_valign(gtk::Align::Center);
    redo_b.set_valign(gtk::Align::Center);
    header.pack_start(&undo_b, false, false, 0);
    header.pack_start(&redo_b, false, false, 0);
    let zoom_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    zoom_group.style_context().add_class("zoom-group");
    zoom_group.set_valign(gtk::Align::Center);
    let fit = icon_button("Fit", "fit");
    let zoom_out = icon_button("Zoom out", "minus");
    let zoom = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5., 800., 1.);
    zoom.set_value(100.);
    zoom.set_size_request(76, -1);
    zoom.set_tooltip_text(Some("Canvas zoom"));
    zoom.set_draw_value(false);
    let zoom_label = gtk::Label::new(Some("100%"));
    zoom_label.set_width_chars(5);
    zoom_label.style_context().add_class("zoom-label");
    let zoom_in = icon_button("Zoom in", "plus");
    for widget in [
        fit.clone().upcast::<gtk::Widget>(),
        zoom_out.clone().upcast(),
        zoom.clone().upcast(),
        zoom_in.clone().upcast(),
        zoom_label.clone().upcast(),
    ] {
        zoom_group.pack_start(&widget, false, false, 0);
    }
    header.pack_start(&zoom_group, false, false, 0);
    let add_images = icon_button("Add images", "image");
    add_images.set_label("Add images");
    add_images.style_context().add_class("add-images");
    add_images.set_valign(gtk::Align::Center);
    header.pack_start(&add_images, false, false, 0);
    root.pack_start(&header, false, false, 0);
    if restored.is_some() {
        let banner = gtk::InfoBar::new();
        banner.set_message_type(gtk::MessageType::Info);
        banner.add_child(&gtk::Label::new(Some(
            "Unsaved editing draft restored — export, save, or keep editing.",
        )));
        root.pack_start(&banner, false, false, 0)
    }
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_hexpand(true);
    body.set_halign(gtk::Align::Fill);
    body.set_size_request(1280, -1);
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rail.set_size_request(40, -1);
    rail.style_context().add_class("tool-rail");
    let tool_defs = [
        ("select", Tool::Select, "Select & move (V)"),
        ("crop", Tool::Crop, "Crop (C)"),
        ("text", Tool::Text, "Text (T)"),
    ];
    for (icon, tool, tip) in tool_defs {
        let b = icon_button(tip, icon);
        b.set_widget_name(tip);
        if tool == Tool::Select {
            b.style_context().add_class("active");
        }
        let rail_buttons = rail.clone();
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        b.connect_clicked(move |clicked| {
            for child in rail_buttons.children() {
                child.style_context().remove_class("active");
            }
            clicked.style_context().add_class("active");
            let mut s = s.borrow_mut();
            s.tool = tool;
            s.selected = if matches!(tool, Tool::Crop | Tool::Pen) {
                None
            } else {
                s.selected
            };
            drop(s);
            refresh(&r, &a)
        });
        rail.pack_start(&b, false, false, 0)
    }
    let shape_button = icon_button("Shapes", "shapes");
    shape_button.set_widget_name("Shapes");
    let shape_popover = gtk::Popover::new();
    shape_popover.set_parent(&shape_button);
    shape_popover
        .style_context()
        .add_class("editor-shape-popover");
    let shape_grid = gtk::Grid::new();
    shape_grid.set_row_spacing(4);
    shape_grid.set_column_spacing(4);
    for (index, (icon, tool, tip)) in [
        ("rectangle", Tool::Rectangle, "Rectangle"),
        ("ellipse", Tool::Ellipse, "Ellipse"),
        ("line", Tool::Line, "Line"),
        ("triangle", Tool::Triangle, "Triangle"),
        ("diamond", Tool::Diamond, "Diamond"),
        ("star", Tool::Star, "Star"),
    ]
    .into_iter()
    .enumerate()
    {
        let item = icon_button(tip, icon);
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        let popover = shape_popover.clone();
        item.connect_clicked(move |_| {
            s.borrow_mut().tool = tool;
            popover.popdown();
            refresh(&r, &a)
        });
        shape_grid.attach(&item, (index % 3) as i32, (index / 3) as i32, 1, 1);
    }
    shape_popover.set_child(Some(&shape_grid));
    {
        let popover = shape_popover.clone();
        shape_button.connect_clicked(move |_| {
            popover.show_all();
            popover.popup()
        });
    }
    rail.pack_start(&shape_button, false, false, 0);
    for (icon, tool, tip) in [
        ("arrow", Tool::Arrow, "Arrow (A)"),
        ("pen", Tool::Pen, "Freehand (P)"),
        ("remove-bg", Tool::RemoveBg, "Remove background (B)"),
    ] {
        let b = icon_button(tip, icon);
        b.set_widget_name(tip);
        let rail_buttons = rail.clone();
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        b.connect_clicked(move |clicked| {
            for child in rail_buttons.children() {
                child.style_context().remove_class("active");
            }
            clicked.style_context().add_class("active");
            let mut state = s.borrow_mut();
            state.tool = tool;
            if tool == Tool::Pen {
                state.selected = None;
            }
            drop(state);
            refresh(&r, &a)
        });
        rail.pack_start(&b, false, false, 0);
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hexpand(true);
    scroll.style_context().add_class("canvas-viewport");
    scroll.set_propagate_natural_width(false);
    scroll.set_propagate_natural_height(false);
    scroll.set_min_content_width(320);
    scroll.set_min_content_height(200);
    scroll.add(&area);
    body.pack_start(&rail, false, false, 0);
    body.pack_start(&scroll, true, true, 0);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let sidebar_shell = gtk::ScrolledWindow::new();
    sidebar_shell.set_size_request(320, -1);
    sidebar_shell.set_hexpand(false);
    sidebar_shell.set_halign(gtk::Align::End);
    sidebar_shell.set_propagate_natural_width(false);
    sidebar_shell.set_propagate_natural_height(false);
    sidebar_shell.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    sidebar_shell.style_context().add_class("editor-sidebar");
    sidebar_shell.add(&sidebar);
    let layer_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    layer_header.style_context().add_class("layers-heading");
    layer_header.pack_start(&ui::label("Layers", "title"), false, false, 0);
    let layer_count = ui::label(&state.borrow().doc.layers.len().to_string(), "layer-count");
    layer_count.set_halign(gtk::Align::Start);
    layer_header.pack_start(&layer_count, false, false, 0);
    let layer_header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layer_header.pack_start(&layer_header_spacer, true, true, 0);
    let add_image = icon_button("Add image layer", "plus");
    add_image.set_valign(gtk::Align::Center);
    layer_count.set_valign(gtk::Align::Center);
    layer_header.pack_start(&add_image, false, false, 0);
    sidebar.pack_start(&layer_header, false, false, 0);
    let layer_scroll = gtk::ScrolledWindow::new();
    layer_scroll.set_halign(gtk::Align::Fill);
    layer_scroll.set_hexpand(true);
    layer_scroll.set_min_content_height(188);
    layer_scroll.set_max_content_height(260);
    layer_scroll.set_propagate_natural_height(false);
    layer_scroll.add(&layers);
    sidebar.pack_start(&layer_scroll, true, true, 0);
    let properties_scroll = gtk::ScrolledWindow::new();
    properties_scroll.set_min_content_height(180);
    properties_scroll.set_propagate_natural_height(false);
    properties_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    properties_scroll.add(&properties);
    properties_scroll
        .style_context()
        .add_class("properties-scroll");
    sidebar.pack_start(&properties_scroll, true, true, 0);
    body.pack_end(&sidebar_shell, false, false, 0);
    root.pack_start(&body, true, true, 0);
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    footer.style_context().add_class("editor-footer");
    let name = gtk::Entry::new();
    let initial_name = state
        .borrow()
        .source
        .as_ref()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or("Capture-edited")
        .to_owned();
    name.set_text(&initial_name);
    name.set_tooltip_text(Some("Saved filename"));
    ui::named(&name, "Filename");
    let format = gtk::ComboBoxText::new();
    for f in ["PNG", "JPEG", "WebP"] {
        format.append_text(f)
    }
    format.set_active(Some(0));
    format.set_tooltip_text(Some("Format"));
    let quality_mode = gtk::ComboBoxText::new();
    for q in ["Preserve quality", "Compress", "Maximum file size"] {
        quality_mode.append_text(q)
    }
    quality_mode.set_active(Some(0));
    quality_mode.set_tooltip_text(Some("Save quality"));
    let quality = gtk::ComboBoxText::new();
    for (id, label) in [
        ("98", "Highest"),
        ("92", "High"),
        ("80", "Balanced"),
        ("65", "Smaller"),
    ] {
        quality.append(Some(id), label);
    }
    quality.set_active_id(Some("92"));
    quality.set_tooltip_text(Some("Compression quality preset"));
    quality.set_visible(false);
    let maximum_size = gtk::SpinButton::with_range(0.01, 1024., 0.01);
    maximum_size.set_value(10.);
    maximum_size.set_digits(2);
    maximum_size.set_tooltip_text(Some("Maximum export file size"));
    maximum_size.set_visible(false);
    let maximum_unit = gtk::ComboBoxText::new();
    for unit in ["KB", "MB", "GB"] {
        maximum_unit.append_text(unit)
    }
    maximum_unit.set_active(Some(1));
    maximum_unit.set_tooltip_text(Some("Maximum file size unit"));
    maximum_unit.set_visible(false);
    let compare = button("Compare", "Preview compression before exporting");
    let export_settings = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    export_settings.style_context().add_class("export-settings");
    let output_size = gtk::ComboBoxText::new();
    output_size.append_text("Original");
    output_size.set_active(Some(0));
    export_settings.pack_start(&field_label("Output size", &output_size), false, false, 0);
    export_settings.pack_start(&field_label("Save quality", &quality_mode), false, false, 0);
    let quality_field = field_label("Quality", &quality);
    quality_field.set_visible(false);
    export_settings.pack_start(&quality_field, false, false, 0);
    let maximum = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    maximum.pack_start(&maximum_size, true, true, 0);
    maximum.pack_start(&maximum_unit, false, false, 0);
    let maximum_field = field_label("Maximum file size", &maximum);
    maximum_field.set_visible(false);
    export_settings.pack_start(&maximum_field, false, false, 0);
    let settings_revealer = gtk::Revealer::new();
    settings_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    settings_revealer.add(&export_settings);
    footer.pack_start(&settings_revealer, false, false, 0);

    let save_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    save_row.style_context().add_class("editor-save-row");
    let disclosure = gtk::Button::new();
    ui::named(&disclosure, "Export settings");
    let disclosure_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let disclosure_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    disclosure_text.pack_start(
        &ui::label("Export settings", "export-disclosure-label"),
        false,
        false,
        0,
    );
    disclosure_text.pack_start(
        &ui::label(
            &format!(
                "PNG · {} × {}",
                state.borrow().doc.width,
                state.borrow().doc.height
            ),
            "export-summary",
        ),
        false,
        false,
        0,
    );
    disclosure_content.pack_start(&disclosure_text, true, true, 0);
    disclosure_content.pack_end(&ui::icon("chevron-down", 15), false, false, 0);
    disclosure.add(&disclosure_content);
    disclosure.set_valign(gtk::Align::End);
    disclosure.style_context().add_class("export-disclosure");
    save_row.pack_start(&disclosure, false, false, 0);
    let filename_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    filename_row.style_context().add_class("filename-row");
    name.set_hexpand(true);
    filename_row.pack_start(&name, true, true, 0);
    filename_row.pack_start(&format, false, false, 0);
    let filename = field_label("Filename", &filename_row);
    filename.set_size_request(240, -1);
    save_row.pack_start(&filename, false, false, 0);
    let copy = icon_button("Copy image", "copy");
    copy.set_label("Copy image");
    copy.style_context().add_class("secondary-action");
    copy.set_valign(gtk::Align::End);
    save_row.pack_start(&copy, false, false, 0);
    let status = gtk::Box::new(gtk::Orientation::Vertical, 2);
    status.set_hexpand(true);
    status.set_halign(gtk::Align::End);
    status.set_valign(gtk::Align::End);
    let status_notice = ui::label("", "positive");
    status_notice.set_xalign(1.0);
    let save_hint = ui::label(
        if state.borrow().source.is_some() {
            "Replaces the original unless Save as new file is on."
        } else {
            "Creates a new file. Your original stays untouched."
        },
        "muted",
    );
    save_hint.set_xalign(1.0);
    save_hint.set_wrap(true);
    save_hint.set_max_width_chars(24);
    status.pack_start(&status_notice, false, false, 0);
    status.pack_start(&save_hint, false, false, 0);
    save_row.pack_start(&status, true, true, 0);
    let make_copy = gtk::CheckButton::with_label("Save as new file");
    make_copy.set_active(state.borrow().source.is_none());
    make_copy.set_sensitive(state.borrow().source.is_some());
    make_copy.style_context().add_class("make-copy");
    make_copy.set_valign(gtk::Align::End);
    save_row.pack_start(&make_copy, false, false, 0);
    let save = icon_button("Save", "save");
    save.set_label("Save");
    save.style_context().add_class("primary");
    save.set_valign(gtk::Align::End);
    save_row.pack_start(&save, false, false, 0);
    footer.pack_start(&save_row, false, false, 0);
    root.pack_start(&footer, false, false, 0);

    {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        undo_b.connect_clicked(move |_| {
            undo(&mut s.borrow_mut(), false);
            refresh(&r, &a)
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        redo_b.connect_clicked(move |_| {
            undo(&mut s.borrow_mut(), true);
            refresh(&r, &a)
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let zl = zoom_label.clone();
        zoom.connect_value_changed(move |z| {
            s.borrow_mut().zoom = z.value() / 100.;
            zl.set_text(&format!("{:.0}%", z.value()));
            a.queue_draw()
        });
    }
    for (b, factor) in [(zoom_out.clone(), 0.8), (zoom_in.clone(), 1.25)] {
        let z = zoom.clone();
        b.connect_clicked(move |_| z.set_value((z.value() * factor).clamp(5., 800.)));
    }
    {
        let z = zoom.clone();
        let s = state.clone();
        let scroll = scroll.clone();
        fit.connect_clicked(move |_| {
            let value = {
                let state = s.borrow();
                let alloc = scroll.allocation();
                ((alloc.width() as f64 - 48.) / state.doc.width as f64)
                    .min((alloc.height() as f64 - 48.) / state.doc.height as f64)
                    .clamp(0.05, 8.)
                    * 100.
            };
            z.set_value(value)
        });
    }
    {
        let q = quality_field.clone();
        let maximum = maximum_field.clone();
        quality_mode.connect_changed(move |m| {
            q.set_visible(m.active() == Some(1));
            maximum.set_visible(m.active() == Some(2));
        });
    }
    for (dimension, width) in [(canvas_width, true), (canvas_height, false)] {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        dimension.connect_value_changed(move |input| {
            let value = input.value().round().clamp(1., 16_384.) as u32;
            let mut state = s.borrow_mut();
            let current = if width {
                state.doc.width
            } else {
                state.doc.height
            };
            if current == value {
                return;
            }
            checkpoint(&mut state);
            if width {
                state.doc.width = value;
            } else {
                state.doc.height = value;
            }
            changed(&mut state);
            drop(state);
            refresh(&r, &a);
        });
    }
    {
        let s = state.clone();
        let w = window.clone();
        let fmt = format.clone();
        let qm = quality_mode.clone();
        let q = quality.clone();
        let maximum_size = maximum_size.clone();
        let maximum_unit = maximum_unit.clone();
        compare.connect_clicked(move |_| {
            let format = fmt
                .active_text()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "PNG".into());
            let quality = selected_quality(&qm, &q, &maximum_size, &maximum_unit);
            compression_comparison(&w, &s.borrow().doc, &format, quality);
        });
    }
    for add_image in [add_image, add_images] {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        let w = window.clone();
        add_image.connect_clicked(move |_| {
            let (s, a, r, w) = (s.clone(), a.clone(), r.clone(), w.clone());
            ui::open_file(&w.clone(), move |p| match image::open(p) {
                Ok(i) => {
                    let mut s = s.borrow_mut();
                    checkpoint(&mut s);
                    let index = s.doc.add_image(i.to_rgba8(), Point { x: 24., y: 24. });
                    s.selected = Some(index);
                    changed(&mut s);
                    drop(s);
                    refresh(&r, &a)
                }
                Err(e) => ui::error(&w, &e.to_string()),
            });
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        trim.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            checkpoint(&mut state);
            trim_to_content(&mut state.doc, 0.);
            changed(&mut state);
            drop(state);
            refresh(&r, &a);
        });
    }
    {
        let revealer = settings_revealer.clone();
        let compare = compare.clone();
        disclosure.connect_clicked(move |_| {
            let opening = !revealer.reveals_child();
            revealer.set_reveal_child(opening);
            if opening {
                compare.clicked();
            }
        });
    }
    {
        let s = state.clone();
        let notice = status_notice.clone();
        copy.connect_clicked(move |_| match render(&s.borrow().doc) {
            Ok(image) => {
                gdk::Display::default()
                    .unwrap()
                    .clipboard()
                    .set_texture(&ui::texture(&image));
                notice.set_text("Copied");
            }
            Err(error) => notice.set_text(&error),
        });
    }
    setup_canvas(&area, &scroll, &state, &refresh_cb, &window);
    setup_keys(&window, &state, &area, &refresh_cb, &zoom);
    setup_sidebar(
        &layers,
        &properties,
        &layer_count,
        &rail,
        &state,
        &area,
        &refresh_cb,
    );
    setup_output(
        &save,
        &make_copy,
        &format,
        &quality_mode,
        &quality,
        &maximum_size,
        &maximum_unit,
        &name,
        &status_notice,
        &state,
        &window,
        on_saved,
    );
    {
        let s = state.clone();
        window.connect_close_request(move |_| {
            if s.borrow().dirty {
                let _ = write_draft(&s.borrow());
            }
            glib::Propagation::Proceed
        });
    }
    window.add(&root);
    let initial_zoom = {
        let state = state.borrow();
        (620. / state.doc.width as f64)
            .min(500. / state.doc.height as f64)
            .min(1.)
            * 100.
    };
    zoom.set_value(initial_zoom.max(5.));
    window.show_all();
    quality_field.set_visible(false);
    maximum_field.set_visible(false);
    if let Some(f) = refresh_cb.borrow().as_ref() {
        f()
    }
    // The sidebar/footer determine the real viewport only after allocation.
    // The provisional zoom above must not leave the source clipped on open.
    glib::idle_add_local_once(move || fit.emit_clicked());
}

fn install_editor_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(r#"
.editor-window { background: @captures_surface; color: @captures_text; }
.editor-window .image-editor { background: @captures_surface; color: @captures_text; }
.editor-window .editor-header {
  min-height: 51px; padding: 0 12px; border-bottom: 1px solid @captures_border;
  background: @captures_raised;
}
.editor-window .editor-header button { min-width: 34px; min-height: 34px; padding: 0; border: 1px solid transparent; border-radius: 7px; background: transparent; }
.editor-window .editor-header button:hover { background: alpha(@captures_text,.07); }
.editor-window .canvas-toolbar { min-height: 26px; padding: 3px; border: 1px solid @captures_border; border-radius: 9px; background: @captures_sunken; }
.editor-window .canvas-toolbar .toolbar-label, .editor-window .canvas-toolbar .canvas-dimensions { color: @captures_text_muted; font-size: 11px; }
.editor-window .canvas-toolbar .canvas-dimensions { font-family: monospace; padding: 0 6px; }
.editor-window .canvas-toolbar spinbutton { min-height: 28px; padding: 0; border: 1px solid transparent; border-radius: 5px; background: transparent; font-family: monospace; font-size: 11px; }
.editor-window .canvas-toolbar spinbutton:focus { border-color: @captures_accent; background: @captures_surface; }
.editor-window .canvas-toolbar spinbutton entry { min-height: 26px; padding: 0 3px; border: 0; background: transparent; }
.editor-window .canvas-toolbar spinbutton button { min-width: 14px; min-height: 13px; padding: 0; }
.editor-window .canvas-toolbar .toolbar-split { min-height: 16px; margin: 6px 3px; }
.editor-window .canvas-toolbar .canvas-tool { min-width: 0; min-height: 26px; padding: 0 8px; font-size: 12px; }
.editor-window .zoom-group { min-height: 34px; border: 1px solid @captures_border; border-radius: 9px; background: @captures_sunken; }
.editor-window .zoom-group button { min-width: 30px; min-height: 32px; border-radius: 0; border-right: 1px solid @captures_border; }
.editor-window .zoom-group scale { min-width: 76px; padding: 0 8px; }
.editor-window .zoom-group .zoom-label { min-width: 48px; padding: 0 7px; font-family: monospace; font-size: 11px; }
.editor-window .editor-header .add-images { min-width: 104px; padding: 0 14px; border-color: @captures_border; background: @captures_surface; font-size: 12px; }

.editor-window .tool-rail { min-width: 40px; padding: 12px 8px; border-right: 1px solid @captures_border; background: @captures_raised; }
.editor-window .tool-rail button { min-width: 38px; min-height: 38px; padding: 0; border: 1px solid transparent; border-radius: 9px; background: transparent; }
.editor-window .tool-rail button:hover { background: alpha(@captures_text,.07); }
.editor-window .tool-rail button.active { color: @captures_accent_ink; background: @captures_accent; }
.editor-window .editor-shape-popover button { min-width: 44px; min-height: 44px; padding: 0; border: 0; border-radius: 7px; background: transparent; }
.editor-window .editor-shape-popover button:hover { background: alpha(@captures_text,.07); }

.editor-window .canvas-viewport { padding: 32px; background-color: @captures_sunken; }
.editor-window .editor-sidebar { min-width: 320px; border-left: 1px solid @captures_border; background-color: @captures_raised; }
.editor-window .layers-heading { min-height: 48px; padding: 0 16px 0 20px; border-bottom: 1px solid @captures_border; }
.editor-window .layers-heading .title { font-size: 14px; font-weight: 600; }
.editor-window .layers-heading .layer-count { min-width: 19px; min-height: 19px; padding: 0 5px; border-radius: 10px; background: @captures_sunken; color: @captures_text_muted; font-family: monospace; font-size: 10px; }
.editor-window .layers-heading button { min-width: 30px; min-height: 30px; padding: 0; border: 0; border-radius: 7px; background: transparent; }
.editor-window .properties-scroll { border-top: 1px solid @captures_border; }
.editor-window .editor-sidebar .section-title { margin: 14px 20px 4px; font-size: 11px; font-weight: 600; opacity: .72; }
.editor-window .editor-property-section .section-title { margin-left: 0; margin-right: 0; }
.editor-window .editor-sidebar .muted { color: @captures_text_muted; font-size: 12px; }
.editor-window .editor-color-swatch { border: 2px solid @captures_accent; border-radius: 7px; box-shadow: 0 0 0 2px alpha(@captures_accent,.18); }
.editor-window .editor-sidebar spinbutton { min-width: 70px; }
.editor-window .editor-property-section { padding: 0 20px 14px; border-bottom: 1px solid @captures_border; }
.editor-window .editor-layer-row { min-height: 54px; margin: 2px; padding: 4px 7px; border: 1px solid transparent; border-radius: 9px; background: transparent; }
.editor-window .editor-layer-row:hover { background: alpha(@captures_text,.06); }
.editor-window .editor-layer-row.selected { border-color: alpha(@captures_accent,.45); background: alpha(@captures_accent,.12); }
.editor-window .editor-layer-row image { min-width: 44px; min-height: 32px; border: 1px solid @captures_border; border-radius: 5px; }
.editor-window .editor-layer-row entry { min-width: 80px; min-height: 28px; padding: 0 6px; border: 0; background: transparent; font-size: 12px; }
.editor-window .editor-layer-row .layer-copy entry { min-height: 18px; padding: 0; font-weight: 500; }
.editor-window .editor-layer-row .layer-kind { color: @captures_text_muted; font-size: 11px; }
.editor-window .editor-layer-row button { min-width: 25px; min-height: 28px; padding: 0; border: 0; border-radius: 5px; background: transparent; opacity: .72; }
.editor-window .editor-layer-row button:hover { background: alpha(@captures_text,.10); opacity: 1; }

.editor-window .editor-footer { padding: 8px 12px; border-top: 1px solid @captures_border; background: @captures_raised; }
.editor-window .export-settings { padding: 12px; border: 1px solid @captures_border; border-radius: 12px; background: @captures_sunken; }
.editor-window .editor-export-field > label { color: @captures_text_muted; font-size: 11px; }
.editor-window .editor-export-field entry, .editor-window .editor-export-field spinbutton, .editor-window .editor-export-field combobox button, .editor-window .editor-export-field button { min-height: 36px; border: 1px solid @captures_border; border-radius: 7px; background: @captures_surface; }
.editor-window .editor-save-row { min-height: 56px; }
.editor-window .export-disclosure { min-width: 158px; min-height: 26px; padding: 4px 8px 4px 12px; border: 1px solid @captures_border; border-radius: 7px; background: @captures_surface; font-size: 12px; }
.editor-window .export-summary { font-family: monospace; font-size: 10px; color: @captures_text_muted; }
.editor-window .filename-row { min-height: 34px; border: 1px solid @captures_border; border-radius: 7px; background: @captures_surface; }
.editor-window .filename-row entry { min-height: 34px; padding: 0 10px; border: 0; background: transparent; }
.editor-window .filename-row combobox button { min-height: 34px; border: 0; border-left: 1px solid @captures_border; border-radius: 0; background: transparent; }
.editor-window .secondary-action { min-width: 84px; min-height: 34px; padding: 0 12px; border: 1px solid @captures_border; border-radius: 7px; background: @captures_surface; }
.editor-window .make-copy { min-height: 36px; font-size: 11px; color: @captures_text_muted; }
.editor-window .editor-footer button.primary { min-width: 82px; min-height: 36px; padding: 0 12px; border: 0; border-radius: 7px; color: @captures_accent_ink; background: @captures_accent; font-weight: 600; }
.editor-window .editor-footer button.primary:hover { background: @captures_accent_hover; }
.editor-window .secondary-action, .editor-window .editor-header .add-images { color: @captures_text; }
.editor-window .secondary-action:hover, .editor-window .export-disclosure:hover, .editor-window .editor-header .add-images:hover { background: alpha(@captures_text,.05); }
.editor-window .editor-export-field spinbutton entry { min-height: 0; border: 0; background: transparent; }
.editor-window .editor-export-field spinbutton button { min-height: 0; border: 0; border-left: 1px solid @captures_border; border-radius: 0; background: transparent; }
.editor-window .editor-footer .positive { color: #27864c; font-size: 11px; }
.editor-window .editor-footer .muted { color: @captures_text_muted; font-size: 11px; }
"#);
    ui::install_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
}

fn handles(l: &Layer) -> [Point; 8] {
    let r = l.frame;
    [
        Point { x: r.x, y: r.y },
        Point {
            x: r.x + r.w / 2.,
            y: r.y,
        },
        Point {
            x: r.x + r.w,
            y: r.y,
        },
        Point {
            x: r.x,
            y: r.y + r.h / 2.,
        },
        Point {
            x: r.x + r.w,
            y: r.y + r.h / 2.,
        },
        Point {
            x: r.x,
            y: r.y + r.h,
        },
        Point {
            x: r.x + r.w / 2.,
            y: r.y + r.h,
        },
        Point {
            x: r.x + r.w,
            y: r.y + r.h,
        },
    ]
}
fn setup_canvas(
    area: &gtk::DrawingArea,
    scroll: &gtk::ScrolledWindow,
    state: &Rc<RefCell<State>>,
    refresh_cb: &Refresh,
    window: &gtk::Window,
) {
    {
        let s = state.clone();
        area.connect_draw(move |a, c| {
            let s = s.borrow();
            a.set_size_request(
                (s.doc.width as f64 * s.zoom) as i32,
                (s.doc.height as f64 * s.zoom) as i32,
            );
            let background = ui::color("surface-sunken");
            c.set_source_rgba(
                f64::from(background.red()),
                f64::from(background.green()),
                f64::from(background.blue()),
                1.,
            );
            let _ = c.paint();
            c.scale(s.zoom, s.zoom);
            c.rectangle(0., 0., s.doc.width as f64, s.doc.height as f64);
            c.clip();
            let tile = 12.;
            for y in (0..s.doc.height).step_by(tile as usize) {
                for x in (0..s.doc.width).step_by(tile as usize) {
                    let v = if (x / tile as u32 + y / tile as u32).is_multiple_of(2) {
                        0.82
                    } else {
                        0.68
                    };
                    c.set_source_rgb(v, v, v);
                    c.rectangle(x as f64, y as f64, tile, tile);
                    let _ = c.fill();
                }
            }
            // The export renderer clears to transparency first. On-screen we
            // keep the checkerboard underneath transparent and erased pixels.
            for layer in &s.doc.layers {
                let _ = draw_layer(c, layer);
            }
            if let Some(p) = &s.preview {
                let _ = draw_layer(c, p);
            }
            if s.tool == Tool::Select
                && let Some(l) = s
                    .selected
                    .and_then(|i| s.doc.layers.get(i))
                    .filter(|l| !l.locked && l.visible)
            {
                let accent = ui::color("accent");
                c.set_source_rgba(
                    f64::from(accent.red()),
                    f64::from(accent.green()),
                    f64::from(accent.blue()),
                    1.,
                );
                c.set_line_width(1.5 / s.zoom);
                let r = layer_bounds(l);
                c.rectangle(r.x, r.y, r.w, r.h);
                let _ = c.stroke();
                for p in handles(l) {
                    c.rectangle(
                        p.x - 4. / s.zoom,
                        p.y - 4. / s.zoom,
                        8. / s.zoom,
                        8. / s.zoom,
                    );
                    let _ = c.fill();
                }
                let p = rotate_handle(l);
                c.arc(p.x, p.y, 5. / s.zoom, 0., 2. * PI);
                let _ = c.fill();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let w = window.clone();
        let r = refresh_cb.clone();
        let scroll = scroll.clone();
        area.connect_button_press_event(move |_, e| {
            let state_ref = s.clone();
            let mut s = s.borrow_mut();
            let (x, y) = e.position();
            let p = Point {
                x: x / s.zoom,
                y: y / s.zoom,
            };
            let before = s.doc.clone();
            if s.space_down || e.button() == 2 {
                s.gesture = Some(Gesture::Pan {
                    x,
                    y,
                    horizontal: scroll.hadjustment().value(),
                    vertical: scroll.vadjustment().value(),
                });
                drop(s);
                a.grab_focus();
                return glib::Propagation::Stop;
            }
            match s.tool {
                Tool::Select => {
                    let picked = s.doc.layers.iter().rposition(|l| hit(l, p));
                    s.selected = picked;
                    if let Some(i) = picked.filter(|i| !s.doc.layers[*i].locked) {
                        let l = &s.doc.layers[i];
                        let rh = rotate_handle(l);
                        if ((p.x - rh.x).powi(2) + (p.y - rh.y).powi(2)).sqrt() < 10. / s.zoom {
                            s.gesture = Some(Gesture::Rotate {
                                index: i,
                                center: Point {
                                    x: l.frame.x + l.frame.w / 2.,
                                    y: l.frame.y + l.frame.h / 2.,
                                },
                                offset: (p.y - (l.frame.y + l.frame.h / 2.))
                                    .atan2(p.x - (l.frame.x + l.frame.w / 2.))
                                    - l.rotation,
                                before,
                            })
                        } else if let Some(handle) = handles(l).iter().position(|h| {
                            ((p.x - h.x).powi(2) + (p.y - h.y).powi(2)).sqrt() < 9. / s.zoom
                        }) {
                            s.gesture = Some(Gesture::Resize {
                                index: i,
                                start: l.frame,
                                handle,
                                before,
                            })
                        } else {
                            s.gesture = Some(Gesture::Move {
                                index: i,
                                start: p,
                                frame: l.frame,
                                before,
                            })
                        }
                    }
                }
                Tool::Crop => s.gesture = Some(Gesture::Crop { start: p }),
                Tool::RemoveBg => {
                    if let Some(i) = s
                        .doc
                        .layers
                        .iter()
                        .rposition(|l| hit(l, p) && matches!(l.kind, LayerKind::Image { .. }))
                    {
                        s.selected = Some(i);
                        s.undo.push(before);
                        s.redo.clear();
                        if s.erase_mode == EraseMode::Wand {
                            let tolerance = s.wand_tolerance;
                            let contiguous = s.wand_contiguous;
                            let _ = remove_color(&mut s.doc.layers[i], p, tolerance, contiguous);
                            changed(&mut s)
                        } else {
                            let restore = s.erase_mode == EraseMode::Restore;
                            let radius = s.stroke * 3.;
                            let softness = s.brush_softness;
                            let _ = erase_soft(&mut s.doc.layers[i], p, radius, restore, softness);
                            s.gesture = Some(Gesture::Erase { index: i })
                        }
                    }
                }
                Tool::Text => {
                    drop(s);
                    text_dialog(&w, &a, &state_ref, p, &r);
                    return glib::Propagation::Stop;
                }
                _ => {
                    s.gesture = Some(Gesture::Draw {
                        start: p,
                        points: vec![p],
                    })
                }
            }
            drop(s);
            a.grab_focus();
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let scroll = scroll.clone();
        area.connect_motion_notify_event(move |_, e| {
            let mut s = s.borrow_mut();
            let (x, y) = e.position();
            let p = Point {
                x: x / s.zoom,
                y: y / s.zoom,
            };
            let Some(g) = s.gesture.take() else {
                return glib::Propagation::Proceed;
            };
            match g {
                Gesture::Move {
                    index,
                    start,
                    frame,
                    before,
                } => {
                    let candidate = Rect {
                        x: frame.x + p.x - start.x,
                        y: frame.y + p.y - start.y,
                        ..frame
                    };
                    let snapped = snap_translation(&s.doc, index, candidate, 10. / s.zoom);
                    s.doc.layers[index].frame.x = snapped.x;
                    s.doc.layers[index].frame.y = snapped.y;
                    s.gesture = Some(Gesture::Move {
                        index,
                        start,
                        frame,
                        before,
                    })
                }
                Gesture::Resize {
                    index,
                    start,
                    handle,
                    before,
                } => {
                    let keep = matches!(s.doc.layers[index].kind, LayerKind::Image { .. });
                    resize_layer(&mut s.doc.layers[index], start, handle, p, keep);
                    s.gesture = Some(Gesture::Resize {
                        index,
                        start,
                        handle,
                        before,
                    })
                }
                Gesture::Rotate {
                    index,
                    center,
                    offset,
                    before,
                } => {
                    s.doc.layers[index].rotation = (p.y - center.y).atan2(p.x - center.x) - offset;
                    s.gesture = Some(Gesture::Rotate {
                        index,
                        center,
                        offset,
                        before,
                    })
                }
                Gesture::Crop { start } => {
                    s.preview = Some(preview_layer(
                        LayerKind::Rectangle,
                        Rect::normalized(start, p),
                        s.color,
                        s.stroke,
                        false,
                    ));
                    s.gesture = Some(Gesture::Crop { start })
                }
                Gesture::Draw { start, mut points } => {
                    points.push(p);
                    let frame = Rect::normalized(start, p);
                    let local = |point: Point| Point {
                        x: point.x - frame.x,
                        y: point.y - frame.y,
                    };
                    let kind = match s.tool {
                        Tool::Pen => LayerKind::Stroke(points.iter().copied().map(local).collect()),
                        Tool::Arrow => LayerKind::Arrow(local(start), local(p)),
                        Tool::Line => LayerKind::Line(local(start), local(p)),
                        Tool::Ellipse => LayerKind::Ellipse,
                        Tool::Triangle => LayerKind::Triangle,
                        Tool::Diamond => LayerKind::Diamond,
                        Tool::Star => LayerKind::Star,
                        _ => LayerKind::Rectangle,
                    };
                    let fill = s.fill_shapes
                        && matches!(
                            s.tool,
                            Tool::Rectangle
                                | Tool::Ellipse
                                | Tool::Triangle
                                | Tool::Diamond
                                | Tool::Star
                        );
                    s.preview = Some(preview_layer(kind, frame, s.color, s.stroke, fill));
                    s.gesture = Some(Gesture::Draw { start, points })
                }
                Gesture::Erase { index } => {
                    let restore = s.erase_mode == EraseMode::Restore;
                    let radius = s.stroke * 3.;
                    let softness = s.brush_softness;
                    let _ = erase_soft(&mut s.doc.layers[index], p, radius, restore, softness);
                    s.gesture = Some(Gesture::Erase { index })
                }
                Gesture::Pan {
                    x: start_x,
                    y: start_y,
                    horizontal,
                    vertical,
                } => {
                    let horizontal_adjustment = scroll.hadjustment();
                    let vertical_adjustment = scroll.vadjustment();
                    horizontal_adjustment.set_value((horizontal - (x - start_x)).clamp(
                        horizontal_adjustment.lower(),
                        horizontal_adjustment.upper() - horizontal_adjustment.page_size(),
                    ));
                    vertical_adjustment.set_value((vertical - (y - start_y)).clamp(
                        vertical_adjustment.lower(),
                        vertical_adjustment.upper() - vertical_adjustment.page_size(),
                    ));
                    s.gesture = Some(Gesture::Pan {
                        x: start_x,
                        y: start_y,
                        horizontal,
                        vertical,
                    })
                }
            }
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let s = state.clone();
        let a = area.clone();
        let r = refresh_cb.clone();
        area.connect_button_release_event(move |_, e| {
            let mut s = s.borrow_mut();
            let (x, y) = e.position();
            let p = Point {
                x: x / s.zoom,
                y: y / s.zoom,
            };
            if let Some(g) = s.gesture.take() {
                match g {
                    Gesture::Crop { start } => {
                        let rect = Rect::normalized(start, p);
                        if rect.w >= 1. && rect.h >= 1. {
                            checkpoint(&mut s);
                            s.doc.crop(rect);
                            s.selected = None;
                            s.preview = None
                        }
                    }
                    Gesture::Draw { .. } => {
                        if let Some(l) = s.preview.take().filter(|l| l.frame.w + l.frame.h > 2.) {
                            checkpoint(&mut s);
                            s.doc.layers.push(l);
                            s.selected = Some(s.doc.layers.len() - 1)
                        }
                    }
                    Gesture::Move { before, .. }
                    | Gesture::Resize { before, .. }
                    | Gesture::Rotate { before, .. } => {
                        if s.doc != before {
                            s.undo.push(before);
                            s.redo.clear()
                        }
                    }
                    Gesture::Erase { .. } => {}
                    Gesture::Pan { .. } => return glib::Propagation::Stop,
                }
                changed(&mut s)
            }
            drop(s);
            refresh(&r, &a);
            glib::Propagation::Stop
        });
    }
    {
        let s = state.clone();
        let z_area = area.clone();
        area.connect_scroll_event(move |_, e| {
            if e.state().contains(gdk::ModifierType::CONTROL_MASK) {
                let mut s = s.borrow_mut();
                let factor = match e.direction() {
                    gdk::ScrollDirection::Up => 1.1,
                    gdk::ScrollDirection::Down => 0.9,
                    _ => 1.,
                };
                s.zoom = (s.zoom * factor).clamp(0.05, 8.);
                z_area.queue_draw();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
}

fn text_dialog(
    parent: &gtk::Window,
    area: &gtk::DrawingArea,
    state: &Rc<RefCell<State>>,
    at: Point,
    refresh_cb: &Refresh,
) {
    let dialog = gtk::Dialog::with_buttons(
        Some("Add text"),
        Some(parent),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Add", gtk::ResponseType::Accept),
        ],
    );
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Type your text"));
    entry.set_activates_default(true);
    dialog.set_default_response(gtk::ResponseType::Accept);
    dialog.content_area().add(&entry);
    dialog.show_all();
    if dialog.run() == gtk::ResponseType::Accept && !entry.text().trim().is_empty() {
        let mut state = state.borrow_mut();
        checkpoint(&mut state);
        let text = entry.text().to_string();
        let frame = Rect {
            x: at.x,
            y: at.y,
            w: (text.chars().count() as f64 * 15.).max(30.),
            h: 34.,
        };
        let color = state.color;
        let stroke = state.stroke;
        let index = state.doc.add(LayerKind::Text(text), frame, color, stroke);
        state.selected = Some(index);
        state.tool = Tool::Select;
        changed(&mut state);
    }
    dialog.close();
    refresh(refresh_cb, area);
}

fn preview_layer(kind: LayerKind, frame: Rect, color: Color, stroke: f64, fill: bool) -> Layer {
    let name = match &kind {
        LayerKind::Stroke(_) => "Freehand",
        LayerKind::Arrow(_, _) => "Arrow",
        LayerKind::Line(_, _) => "Line",
        LayerKind::Rectangle => "Rectangle",
        LayerKind::Ellipse => "Ellipse",
        LayerKind::Triangle => "Triangle",
        LayerKind::Diamond => "Diamond",
        LayerKind::Star => "Star",
        LayerKind::Text(_) => "Text",
        LayerKind::Image { .. } => "Image",
    };
    Layer {
        id: 0,
        name: name.into(),
        kind,
        frame,
        rotation: 0.,
        visible: true,
        locked: false,
        opacity: 1.,
        blend: Blend::Normal,
        color,
        fill: fill.then_some(Color(color.0, color.1, color.2, color.3 * 0.28)),
        stroke,
    }
}

fn setup_sidebar(
    layers: &gtk::Box,
    properties: &gtk::Box,
    layer_count: &gtk::Label,
    rail: &gtk::Box,
    state: &Rc<RefCell<State>>,
    area: &gtk::DrawingArea,
    refresh_cb: &Refresh,
) {
    let s = state.clone();
    let a = area.clone();
    let ls = layers.clone();
    let ps = properties.clone();
    let count = layer_count.clone();
    let tools = rail.clone();
    let r = refresh_cb.clone();
    *refresh_cb.borrow_mut() = Some(Box::new(move || {
        clear(&ls);
        clear(&ps);
        let state = s.borrow();
        count.set_text(&state.doc.layers.len().to_string());
        let active_name = match state.tool {
            Tool::Select => "Select & move (V)",
            Tool::Crop => "Crop (C)",
            Tool::Text => "Text (T)",
            Tool::Line
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Triangle
            | Tool::Diamond
            | Tool::Star => "Shapes",
            Tool::Arrow => "Arrow (A)",
            Tool::Pen => "Freehand (P)",
            Tool::RemoveBg => "Remove background (B)",
        };
        for child in tools.children() {
            child.style_context().remove_class("active");
            if child.widget_name() == active_name {
                child.style_context().add_class("active");
            }
        }
        for index in (0..state.doc.layers.len()).rev() {
            let l = &state.doc.layers[index];
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            row.style_context().add_class("editor-layer-row");
            if state.selected == Some(index) {
                row.style_context().add_class("selected")
            }
            let thumbnail = render_layer_thumbnail(&state.doc, l, 38, 30)
                .ok()
                .and_then(|mut surface| {
                    surface.flush();
                    let stride = surface.stride() as usize;
                    let bytes = glib::Bytes::from_owned(surface.data().ok()?.to_vec());
                    let texture = gdk::MemoryTexture::new(
                        38,
                        30,
                        gdk::MemoryFormat::B8g8r8a8Premultiplied,
                        &bytes,
                        stride,
                    );
                    Some(gtk::Image::from_paintable(Some(&texture)))
                })
                .unwrap_or_default();
            thumbnail.set_tooltip_text(Some("Layer thumbnail"));
            let select = gtk::Entry::new();
            select.set_text(&l.name);
            select.set_tooltip_text(Some("Rename layer"));
            select.set_hexpand(true);
            ui::named(&select, "Rename layer");
            let eye = icon_button(
                if l.visible {
                    "Hide layer"
                } else {
                    "Show layer"
                },
                if l.visible { "eye" } else { "hide" },
            );
            let lock = icon_button(
                if l.locked {
                    "Unlock layer"
                } else {
                    "Lock layer"
                },
                if l.locked { "lock" } else { "unlock" },
            );
            row.pack_start(&thumbnail, false, false, 0);
            let layer_copy = gtk::Box::new(gtk::Orientation::Vertical, 1);
            layer_copy.set_hexpand(true);
            layer_copy.set_valign(gtk::Align::Center);
            eye.set_valign(gtk::Align::Center);
            lock.set_valign(gtk::Align::Center);
            layer_copy.style_context().add_class("layer-copy");
            layer_copy.pack_start(&select, false, false, 0);
            let kind = match &l.kind {
                LayerKind::Image { .. } => {
                    if l.locked {
                        "Locked background"
                    } else {
                        "Image"
                    }
                }
                LayerKind::Text(_) => "Text",
                LayerKind::Stroke(_) => "Freehand",
                LayerKind::Arrow(..) => "Arrow",
                LayerKind::Line(..) => "Line",
                LayerKind::Rectangle => "Rectangle",
                LayerKind::Ellipse => "Ellipse",
                LayerKind::Triangle => "Triangle",
                LayerKind::Diamond => "Diamond",
                LayerKind::Star => "Star",
            };
            layer_copy.pack_start(&ui::label(kind, "layer-kind"), false, false, 0);
            row.pack_start(&layer_copy, true, true, 0);
            row.pack_start(&eye, false, false, 0);
            row.pack_start(&lock, false, false, 0);
            {
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                select.connect_activate(move |entry| {
                    let mut s = s.borrow_mut();
                    let name = entry.text().trim().to_owned();
                    if !name.is_empty() && s.doc.layers[index].name != name {
                        checkpoint(&mut s);
                        s.doc.layers[index].name = name;
                        changed(&mut s);
                    }
                    s.selected = Some(index);
                    s.tool = Tool::Select;
                    drop(s);
                    refresh(&r, &a)
                });
            }
            if !l.locked {
                let source = gtk::DragSource::builder()
                    .actions(gdk::DragAction::MOVE)
                    .build();
                source.connect_prepare(move |_, _, _| {
                    Some(gdk::ContentProvider::for_value(&(index as u64).to_value()))
                });
                row.add_controller(source);
            }
            {
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                let row_for_drop = row.clone();
                let target = gtk::DropTarget::new(u64::static_type(), gdk::DragAction::MOVE);
                target.connect_drop(move |_, value, _, y| {
                    let Ok(moved) = value.get::<u64>() else {
                        return false;
                    };
                    let moved = moved as usize;
                    let mut s = s.borrow_mut();
                    if moved == index || moved >= s.doc.layers.len() || s.doc.layers[moved].locked {
                        return false;
                    }
                    checkpoint(&mut s);
                    let visual_above = y < f64::from(row_for_drop.allocated_height()) / 2.0;
                    let mut target = index + usize::from(visual_above);
                    if moved < target {
                        target -= 1;
                    }
                    let layer = s.doc.layers.remove(moved);
                    let id = layer.id;
                    let locked_floor = s.doc.layers.iter().take_while(|layer| layer.locked).count();
                    target = target.max(locked_floor).min(s.doc.layers.len());
                    s.doc.layers.insert(target, layer);
                    s.selected = s.doc.layers.iter().position(|layer| layer.id == id);
                    changed(&mut s);
                    drop(s);
                    refresh(&r, &a);
                    true
                });
                row.add_controller(target);
            }
            for (b, action) in [(eye, 0), (lock, 1)] {
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                b.connect_clicked(move |_| {
                    let mut s = s.borrow_mut();
                    checkpoint(&mut s);
                    match action {
                        0 => s.doc.layers[index].visible = !s.doc.layers[index].visible,
                        1 => s.doc.layers[index].locked = !s.doc.layers[index].locked,
                        _ => {}
                    }
                    changed(&mut s);
                    drop(s);
                    refresh(&r, &a)
                });
            }
            ls.pack_start(&row, false, false, 0);
        }
        if state.tool == Tool::RemoveBg {
            let eraser = section("REMOVE BACKGROUND");
            eraser.pack_start(
                &ui::label("Remove a color, paint it out, or paint it back.", "muted"),
                false,
                false,
                0,
            );
            let modes = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            for (text, mode) in [
                ("Wand", EraseMode::Wand),
                ("Erase", EraseMode::Erase),
                ("Restore", EraseMode::Restore),
            ] {
                let b = button(text, text);
                if state.erase_mode == mode {
                    b.style_context().add_class("suggested-action");
                }
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                b.connect_clicked(move |_| {
                    s.borrow_mut().erase_mode = mode;
                    refresh(&r, &a)
                });
                modes.pack_start(&b, true, true, 0);
            }
            eraser.pack_start(&modes, false, false, 0);
            if state.erase_mode == EraseMode::Wand {
                eraser.pack_start(&gtk::Label::new(Some("Tolerance")), false, false, 0);
                let tolerance = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 120., 1.);
                tolerance.set_value(f64::from(state.wand_tolerance));
                tolerance.set_tooltip_text(Some("Color tolerance"));
                ui::named(&tolerance, "Color tolerance");
                let s2 = s.clone();
                tolerance.connect_value_changed(move |w| {
                    s2.borrow_mut().wand_tolerance = w.value() as u8
                });
                eraser.pack_start(&tolerance, false, false, 0);
                let contiguous = gtk::CheckButton::with_label("Contiguous only");
                contiguous.set_active(state.wand_contiguous);
                let s2 = s.clone();
                contiguous
                    .connect_toggled(move |w| s2.borrow_mut().wand_contiguous = w.is_active());
                eraser.pack_start(&contiguous, false, false, 0);
            } else {
                eraser.pack_start(&gtk::Label::new(Some("Brush size")), false, false, 0);
                let size = gtk::Scale::with_range(gtk::Orientation::Horizontal, 4., 120., 1.);
                size.set_value(state.stroke * 3.);
                size.set_tooltip_text(Some("Brush size"));
                ui::named(&size, "Brush size");
                let s2 = s.clone();
                size.connect_value_changed(move |w| s2.borrow_mut().stroke = w.value() / 3.);
                eraser.pack_start(&size, false, false, 0);
                eraser.pack_start(&gtk::Label::new(Some("Softness")), false, false, 0);
                let softness = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
                softness.set_value(state.brush_softness * 100.);
                softness.set_tooltip_text(Some("Brush softness"));
                ui::named(&softness, "Brush softness");
                let s2 = s.clone();
                softness.connect_value_changed(move |w| {
                    s2.borrow_mut().brush_softness = w.value() / 100.
                });
                eraser.pack_start(&softness, false, false, 0);
            }
            ps.pack_start(&eraser, false, false, 0);
        } else if matches!(
            state.tool,
            Tool::Line
                | Tool::Rectangle
                | Tool::Ellipse
                | Tool::Triangle
                | Tool::Diamond
                | Tool::Star
                | Tool::Arrow
                | Tool::Pen
                | Tool::Text
        ) {
            let style = section("STYLE");
            style.pack_start(&gtk::Label::new(Some("Color")), false, false, 0);
            let color_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let swatch = gtk::DrawingArea::new();
            swatch.set_size_request(28, 28);
            swatch.style_context().add_class("editor-color-swatch");
            let swatch_color = Rc::new(Cell::new(state.color));
            let drawn_color = swatch_color.clone();
            swatch.connect_draw(move |area, context| {
                let value = drawn_color.get();
                context.set_source_rgba(value.0, value.1, value.2, value.3);
                context.rectangle(
                    0.0,
                    0.0,
                    f64::from(area.allocated_width()),
                    f64::from(area.allocated_height()),
                );
                let _ = context.fill();
                glib::Propagation::Proceed
            });
            let hex = gtk::Entry::new();
            hex.set_width_chars(8);
            hex.set_max_length(7);
            hex.set_text(&format!(
                "#{:02x}{:02x}{:02x}",
                (state.color.0 * 255.).round() as u8,
                (state.color.1 * 255.).round() as u8,
                (state.color.2 * 255.).round() as u8,
            ));
            ui::named(&hex, "Drawing color hex value");
            let (s2, a2, swatch2, swatch_color2) =
                (s.clone(), a.clone(), swatch.clone(), swatch_color.clone());
            hex.connect_changed(move |entry| {
                if let Ok(value) = gdk::RGBA::parse(entry.text().as_str()) {
                    let value = color(value);
                    s2.borrow_mut().color = value;
                    swatch_color2.set(value);
                    swatch2.queue_draw();
                    a2.queue_draw();
                }
            });
            color_row.pack_start(&swatch, false, false, 0);
            color_row.pack_start(&hex, true, true, 0);
            style.pack_start(&color_row, false, false, 0);
            style.pack_start(&gtk::Label::new(Some("Stroke width")), false, false, 0);
            let width = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1., 40., 1.);
            width.set_value(state.stroke);
            width.set_tooltip_text(Some("Stroke width"));
            let s2 = s.clone();
            width.connect_value_changed(move |w| s2.borrow_mut().stroke = w.value());
            style.pack_start(&width, false, false, 0);
            if matches!(
                state.tool,
                Tool::Rectangle | Tool::Ellipse | Tool::Triangle | Tool::Diamond | Tool::Star
            ) {
                let fill = gtk::CheckButton::with_label("Filled shape");
                fill.set_active(state.fill_shapes);
                let s2 = s.clone();
                fill.connect_toggled(move |w| s2.borrow_mut().fill_shapes = w.is_active());
                style.pack_start(&fill, false, false, 0);
            }
            ps.pack_start(&style, false, false, 0);
        } else if let Some(i) = state.selected.filter(|i| *i < state.doc.layers.len()) {
            let l = &state.doc.layers[i];
            let transform = section("TRANSFORM");
            let grid = gtk::Grid::new();
            grid.set_row_spacing(5);
            grid.set_column_spacing(5);
            grid.set_column_homogeneous(true);
            for (n, v, col, row) in [
                ("X", l.frame.x, 0, 0),
                ("Y", l.frame.y, 1, 0),
                ("W", l.frame.w, 0, 1),
                ("H", l.frame.h, 1, 1),
                ("Rotation", l.rotation.to_degrees(), 0, 2),
            ] {
                let spin = gtk::SpinButton::with_range(
                    if n == "Rotation" { -360. } else { -16384. },
                    16384.,
                    1.,
                );
                spin.set_width_chars(5);
                spin.set_size_request(76, -1);
                spin.set_value(v);
                spin.set_sensitive(!l.locked);
                spin.set_tooltip_text(Some(&format!("Layer {n}")));
                let label = gtk::Label::new(Some(n));
                label.set_halign(gtk::Align::Start);
                let field = gtk::Box::new(gtk::Orientation::Vertical, 3);
                field.pack_start(&label, false, false, 0);
                field.pack_start(&spin, false, false, 0);
                grid.attach(&field, col, row, if n == "Rotation" { 2 } else { 1 }, 1);
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                spin.connect_value_changed(move |w| {
                    let mut s = s.borrow_mut();
                    if s.doc.layers.get(i).is_none() {
                        return;
                    }
                    match n {
                        "X" => s.doc.layers[i].frame.x = w.value(),
                        "Y" => s.doc.layers[i].frame.y = w.value(),
                        "W" => s.doc.layers[i].frame.w = w.value().max(1.),
                        "H" => s.doc.layers[i].frame.h = w.value().max(1.),
                        _ => s.doc.layers[i].rotation = w.value().to_radians(),
                    }
                    changed(&mut s);
                    drop(s);
                    refresh(&r, &a)
                });
            }
            transform.pack_start(&grid, false, false, 0);
            ps.pack_start(&transform, false, false, 0);
            let appearance = section("APPEARANCE");
            let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
            opacity.set_value(l.opacity * 100.);
            opacity.set_tooltip_text(Some("Layer opacity"));
            appearance.pack_start(&opacity, false, false, 0);
            let blend = gtk::ComboBoxText::new();
            for b in Blend::ALL {
                blend.append_text(b.label())
            }
            blend.set_active(Some(
                Blend::ALL.iter().position(|b| *b == l.blend).unwrap_or(0) as u32,
            ));
            blend.set_tooltip_text(Some("Blend mode"));
            appearance.pack_start(&blend, false, false, 0);
            let s1 = s.clone();
            let a1 = a.clone();
            opacity.connect_value_changed(move |w| {
                if let Some(l) = s1.borrow_mut().doc.layers.get_mut(i) {
                    l.opacity = w.value() / 100.;
                    a1.queue_draw()
                }
            });
            let s1 = s.clone();
            let a1 = a.clone();
            blend.connect_changed(move |w| {
                if let Some(l) = s1.borrow_mut().doc.layers.get_mut(i) {
                    l.blend = Blend::ALL[w.active().unwrap_or(0) as usize];
                    a1.queue_draw()
                }
            });
            ps.pack_start(&appearance, false, false, 0);
            if matches!(l.kind, LayerKind::Image { .. }) {
                let image_ops = section("IMAGE");
                for (text, op) in [
                    ("↶ Rotate", 0),
                    ("↷ Rotate", 1),
                    ("⇆ Flip", 2),
                    ("⇅ Flip", 3),
                ] {
                    let b = button(text, text);
                    let s = s.clone();
                    let a = a.clone();
                    let r = r.clone();
                    b.connect_clicked(move |_| {
                        let mut s = s.borrow_mut();
                        checkpoint(&mut s);
                        if op < 2 {
                            transform_image(&mut s.doc.layers[i], op == 1)
                        } else {
                            flip_image(&mut s.doc.layers[i], op == 2)
                        }
                        changed(&mut s);
                        drop(s);
                        refresh(&r, &a)
                    });
                    image_ops.pack_start(&b, false, false, 0)
                }
                ps.pack_start(&image_ops, false, false, 0)
            }
            let actions = section("LAYER ACTIONS");
            for (text, op) in [
                ("Bring to front", 5),
                ("Send to back", 6),
                ("Duplicate", 0),
                ("Merge down", 1),
                ("Merge visible", 2),
                ("Flatten image", 3),
                ("Delete", 4),
            ] {
                let b = button(text, text);
                if (op == 1 && i == 0)
                    || (op == 2 && state.doc.layers.iter().filter(|l| l.visible).count() < 2)
                    || (op == 4 && l.locked)
                {
                    b.set_sensitive(false)
                }
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                b.connect_clicked(move |_| {
                    let mut s = s.borrow_mut();
                    checkpoint(&mut s);
                    match op {
                        0 => s.selected = s.doc.duplicate(i),
                        1 => {
                            if i > 0 {
                                s.selected =
                                    merge_indices(&mut s.doc, &[i - 1, i], "Merged layer").ok()
                            }
                        }
                        2 => {
                            let ids: Vec<_> = s
                                .doc
                                .layers
                                .iter()
                                .enumerate()
                                .filter(|(_, l)| l.visible)
                                .map(|(i, _)| i)
                                .collect();
                            s.selected = merge_indices(&mut s.doc, &ids, "Merged visible").ok()
                        }
                        3 => {
                            let ids: Vec<_> = (0..s.doc.layers.len()).collect();
                            if let Ok(i) = merge_indices(&mut s.doc, &ids, "Background") {
                                s.doc.layers[i].locked = true;
                                s.selected = Some(i)
                            }
                        }
                        4 => {
                            if i < s.doc.layers.len() && !s.doc.layers[i].locked {
                                s.doc.layers.remove(i);
                                s.selected = None
                            }
                        }
                        5 if i + 1 < s.doc.layers.len() => {
                            let target = s.doc.layers.len() - 1;
                            s.doc.reorder(i, target);
                            s.selected = Some(target)
                        }
                        6 if i > 0 => s.selected = s.doc.reorder(i, 0),
                        _ => {}
                    }
                    changed(&mut s);
                    drop(s);
                    refresh(&r, &a)
                });
                actions.pack_start(&b, false, false, 0)
            }
            ps.pack_start(&actions, false, false, 0);
        } else {
            let canvas = section("CANVAS");
            canvas.pack_start(
                &ui::label(
                    &format!("{} × {} px", state.doc.width, state.doc.height),
                    "muted",
                ),
                false,
                false,
                0,
            );
            for (text, trim) in [("Trim edges", true), ("Expand to content", false)] {
                let action = button(text, text);
                let s = s.clone();
                let a = a.clone();
                let r = r.clone();
                action.connect_clicked(move |_| {
                    let mut state = s.borrow_mut();
                    checkpoint(&mut state);
                    if trim {
                        trim_to_content(&mut state.doc, 0.);
                    } else {
                        expand_to_content(&mut state.doc, 0.);
                    }
                    changed(&mut state);
                    drop(state);
                    refresh(&r, &a)
                });
                canvas.pack_start(&action, false, false, 0);
            }
            ps.pack_start(&canvas, false, false, 0)
        }
        drop(state);
        ls.show_all();
        ps.show_all();
        a.queue_draw();
    }));
}

fn setup_keys(
    window: &gtk::Window,
    state: &Rc<RefCell<State>>,
    area: &gtk::DrawingArea,
    refresh_cb: &Refresh,
    zoom: &gtk::Scale,
) {
    {
        let s = state.clone();
        window.connect_key_release_event(move |_, e| {
            if e.keyval() == gdk::Key::space {
                s.borrow_mut().space_down = false;
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    let s = state.clone();
    let a = area.clone();
    let r = refresh_cb.clone();
    let z = zoom.clone();
    let key_window = window.clone();
    window.connect_key_press_event(move |_, e| {
        if matches!(
            e.keyval(),
            gdk::Key::plus | gdk::Key::equal | gdk::Key::minus | gdk::Key::_0
        ) && e.state().contains(gdk::ModifierType::CONTROL_MASK)
        {
            z.set_value(match e.keyval() {
                gdk::Key::minus => z.value() / 1.25,
                gdk::Key::_0 => 100.,
                _ => z.value() * 1.25,
            });
            return glib::Propagation::Stop;
        }
        let mut s = s.borrow_mut();
        let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
        if e.keyval() == gdk::Key::space {
            s.space_down = true;
            return glib::Propagation::Stop;
        }
        let editing_field = key_window
            .focus_child()
            .is_some_and(|widget| widget.is::<gtk::Entry>() || widget.is::<gtk::SpinButton>());
        if ctrl && editing_field {
            return glib::Propagation::Proceed;
        }
        if ctrl
            && e.keyval() == gdk::Key::c
            && let Some(layer) = s
                .selected
                .and_then(|index| s.doc.layers.get(index))
                .cloned()
        {
            s.layer_clipboard = Some((layer, 0));
            return glib::Propagation::Stop;
        }
        if ctrl
            && e.keyval() == gdk::Key::v
            && let Some((layer, count)) = s.layer_clipboard.clone()
        {
            checkpoint(&mut s);
            let next_count = count + 1;
            let selected = s.selected;
            let index = paste_layer(&mut s.doc, &layer, selected, 24. * next_count as f64);
            s.selected = Some(index);
            s.layer_clipboard = Some((layer, next_count));
            s.tool = Tool::Select;
            changed(&mut s);
            drop(s);
            refresh(&r, &a);
            return glib::Propagation::Stop;
        }
        if ctrl
            && e.keyval() == gdk::Key::d
            && let Some(index) = s.selected
        {
            checkpoint(&mut s);
            s.selected = s.doc.duplicate(index);
            changed(&mut s);
            drop(s);
            refresh(&r, &a);
            return glib::Propagation::Stop;
        }
        if ctrl && (e.keyval() == gdk::Key::z || e.keyval() == gdk::Key::y) {
            undo(
                &mut s,
                e.keyval() == gdk::Key::y || e.state().contains(gdk::ModifierType::SHIFT_MASK),
            );
            drop(s);
            refresh(&r, &a);
            return glib::Propagation::Stop;
        }
        match e.keyval() {
            gdk::Key::v => s.tool = Tool::Select,
            gdk::Key::c => s.tool = Tool::Crop,
            gdk::Key::t => s.tool = Tool::Text,
            gdk::Key::r => s.tool = Tool::Rectangle,
            gdk::Key::o => s.tool = Tool::Ellipse,
            gdk::Key::a => s.tool = Tool::Arrow,
            gdk::Key::l => s.tool = Tool::Line,
            gdk::Key::p => s.tool = Tool::Pen,
            gdk::Key::b => s.tool = Tool::RemoveBg,
            gdk::Key::Delete => {
                if let Some(i) = s.selected.take().filter(|i| !s.doc.layers[*i].locked) {
                    checkpoint(&mut s);
                    s.doc.layers.remove(i);
                    changed(&mut s)
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        drop(s);
        refresh(&r, &a);
        glib::Propagation::Stop
    });
}

#[derive(Clone, Copy)]
enum ExportQuality {
    Preserve,
    Compress(u8),
    Maximum(u64),
}

fn selected_quality(
    mode: &gtk::ComboBoxText,
    quality: &gtk::ComboBoxText,
    maximum_size: &gtk::SpinButton,
    maximum_unit: &gtk::ComboBoxText,
) -> ExportQuality {
    match mode.active() {
        Some(1) => ExportQuality::Compress(
            quality
                .active_id()
                .and_then(|value| value.parse().ok())
                .unwrap_or(92),
        ),
        Some(2) => {
            let unit = match maximum_unit.active() {
                Some(0) => 1024_f64,
                Some(2) => 1024_f64 * 1024. * 1024.,
                _ => 1024_f64 * 1024.,
            };
            ExportQuality::Maximum((maximum_size.value() * unit).round().max(10_000.) as u64)
        }
        _ => ExportQuality::Preserve,
    }
}

fn encode_output(doc: &Document, format: &str, quality: ExportQuality) -> Result<Vec<u8>, String> {
    let image = render(doc)?;
    match (format, quality) {
        ("JPEG", ExportQuality::Preserve) => encoder::encode_jpeg(&image, 100),
        ("JPEG", ExportQuality::Compress(quality)) => encoder::encode_jpeg(&image, quality),
        ("JPEG", ExportQuality::Maximum(maximum)) => {
            encoder::encode_jpeg_with_limit(&image, maximum)
        }
        ("PNG", ExportQuality::Preserve) => encoder::encode_png(&image, None),
        ("PNG", ExportQuality::Compress(quality)) => encoder::encode_png(&image, Some(quality)),
        ("PNG", ExportQuality::Maximum(maximum)) => encoder::encode_png_with_limit(&image, maximum),
        ("WebP", ExportQuality::Preserve) => encoder::encode_webp(&image, None),
        ("WebP", ExportQuality::Compress(quality)) => encoder::encode_webp(&image, Some(quality)),
        ("WebP", ExportQuality::Maximum(maximum)) => {
            encoder::encode_webp_with_limit(&image, maximum)
        }
        _ => Err(format!("Unsupported image format: {format}")),
    }
}

fn compression_comparison(
    parent: &gtk::Window,
    doc: &Document,
    format: &str,
    quality: ExportQuality,
) {
    let before = match render(doc) {
        Ok(image) => image,
        Err(error) => return ui::error(parent, &error),
    };
    let bytes = match encode_output(doc, format, quality) {
        Ok(bytes) => bytes,
        Err(error) => return ui::error(parent, &error),
    };
    let after = match image::load_from_memory(&bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => return ui::error(parent, &error.to_string()),
    };
    let dialog = gtk::Dialog::with_buttons(
        Some("Compression comparison"),
        Some(parent),
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(860, 500);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let images = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for (title, image) in [("Before", before), ("After", after)] {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 5);
        column.pack_start(&ui::label(title, "title"), false, false, 0);
        let pixbuf = ui::pixbuf(&image);
        let scale = (390. / image.width() as f64)
            .min(390. / image.height() as f64)
            .min(1.);
        let preview = pixbuf
            .scale_simple(
                (image.width() as f64 * scale).max(1.) as i32,
                (image.height() as f64 * scale).max(1.) as i32,
                gtk::gdk_pixbuf::InterpType::Bilinear,
            )
            .unwrap_or(pixbuf);
        column.pack_start(&gtk::Image::from_pixbuf(Some(&preview)), true, true, 0);
        images.pack_start(&column, true, true, 0);
    }
    root.pack_start(&images, true, true, 0);
    let quality_description = match quality {
        ExportQuality::Preserve => "preserve quality".into(),
        ExportQuality::Compress(quality) => format!("{quality}% quality"),
        ExportQuality::Maximum(maximum) => {
            format!("maximum {} KB", maximum.div_ceil(1024))
        }
    };
    root.pack_start(
        &ui::label(
            &format!(
                "{format} at {quality_description} • estimated export {} KB",
                bytes.len().div_ceil(1024)
            ),
            "muted",
        ),
        false,
        false,
        0,
    );
    dialog.content_area().add(&root);
    dialog.show_all();
    dialog.run();
    dialog.close();
}

fn save_named(
    directory: &Path,
    requested: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let base: String = requested
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .collect();
    let base = if base.trim().is_empty() {
        "Capture-edited"
    } else {
        base.trim()
    };
    for index in 0.. {
        let suffix = format!("-{index:03}");
        let path = directory.join(format!("{base}{suffix}.{extension}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(error.to_string());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

#[allow(clippy::too_many_arguments)]
fn setup_output(
    save: &gtk::Button,
    make_copy: &gtk::CheckButton,
    format: &gtk::ComboBoxText,
    quality_mode: &gtk::ComboBoxText,
    quality: &gtk::ComboBoxText,
    maximum_size: &gtk::SpinButton,
    maximum_unit: &gtk::ComboBoxText,
    name: &gtk::Entry,
    status: &gtk::Label,
    state: &Rc<RefCell<State>>,
    window: &gtk::Window,
    on_saved: Rc<dyn Fn(PathBuf)>,
) {
    let s = state.clone();
    let w = window.clone();
    let fmt = format.clone();
    let qm = quality_mode.clone();
    let q = quality.clone();
    let maximum_size = maximum_size.clone();
    let maximum_unit = maximum_unit.clone();
    let filename = name.clone();
    let make_copy = make_copy.clone();
    let notice = status.clone();
    save.connect_clicked(move |b| {
        let format = fmt
            .active_text()
            .map(|x| x.to_string())
            .unwrap_or_else(|| "PNG".into());
        let quality = selected_quality(&qm, &q, &maximum_size, &maximum_unit);
        let doc = s.borrow().doc.clone();
        let source = s.borrow().source.clone();
        let directory = s.borrow().directory.clone();
        let filename = filename.text().trim().to_owned();
        let overwrite = source.is_some() && !make_copy.is_active();
        b.set_sensitive(false);
        let bb = b.clone();
        let ww = w.clone();
        let ss = s.clone();
        let cb = on_saved.clone();
        let notice = notice.clone();
        ui::job(
            move || {
                if overwrite {
                    let path = source.ok_or("No imported source file")?;
                    let source_format = match path
                        .extension()
                        .and_then(|v| v.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "jpg" | "jpeg" => "JPEG",
                        "webp" => "WebP",
                        _ => "PNG",
                    };
                    let bytes = encode_output(&doc, source_format, quality)?;
                    atomic_write_private(&path, &bytes)?;
                    Ok(path)
                } else {
                    let bytes = encode_output(&doc, &format, quality)?;
                    let ext = match format.as_str() {
                        "JPEG" => "jpg",
                        "WebP" => "webp",
                        _ => "png",
                    };
                    save_named(&directory, &filename, ext, &bytes)
                }
            },
            move |result| {
                bb.set_sensitive(true);
                match result {
                    Ok(path) => {
                        let mut st = ss.borrow_mut();
                        st.dirty = false;
                        let _ = std::fs::remove_file(&st.draft);
                        drop(st);
                        notice.set_text("Saved");
                        cb(path)
                    }
                    Err(e) => ui::error(&ww, &e),
                }
            },
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, symlink};
    #[test]
    fn independent_draft_path_never_uses_tauri_name() {
        let i = RgbaImage::new(2, 3);
        let p = draft_path(Some(Path::new("/tmp/a.png")), &i);
        assert_eq!(p.parent().unwrap(), native_data().join("editor-drafts"));
        assert!(!p.to_string_lossy().contains("screenshot-editor-drafts"));
    }

    #[test]
    #[cfg(unix)]
    fn private_atomic_draft_does_not_follow_predictable_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let victim = directory.path().join("victim");
        let draft = directory.path().join("draft.json");
        let predictable = draft.with_extension("tmp");
        std::fs::write(&victim, b"do not replace").unwrap();
        symlink(&victim, &predictable).unwrap();
        symlink(&victim, &draft).unwrap();

        atomic_write_private(&draft, b"private draft").unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"do not replace");
        assert_eq!(std::fs::read(&draft).unwrap(), b"private draft");
        assert_eq!(std::fs::read_link(&predictable).unwrap(), victim);
        assert_eq!(std::fs::metadata(&draft).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn draft_backed_undo_restores_reordered_document() {
        let directory = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        let mut state = State {
            doc: Document::new(image),
            undo: vec![],
            redo: vec![],
            selected: None,
            tool: Tool::Select,
            gesture: None,
            preview: None,
            zoom: 1.,
            color: Color::default(),
            stroke: 4.,
            fill_shapes: false,
            erase_mode: EraseMode::Erase,
            brush_softness: 0.18,
            wand_tolerance: 36,
            wand_contiguous: true,
            layer_clipboard: None,
            space_down: false,
            dirty: false,
            source: None,
            directory: directory.path().into(),
            draft: directory.path().join("draft.json"),
        };
        checkpoint(&mut state);
        state.doc.duplicate(0);
        let top = state.doc.duplicate(0).unwrap();
        state.doc.reorder(top, 1);
        changed(&mut state);
        let persisted: Document =
            serde_json::from_slice(&std::fs::read(&state.draft).unwrap()).unwrap();
        assert_eq!(persisted.layers.len(), 3);
        assert_eq!(persisted.layers[0].id, 1);
        assert_eq!(persisted.layers[1].id, 3);

        undo(&mut state, false);
        assert_eq!(state.doc.layers.len(), 1);
        assert_eq!(state.doc.layers[0].id, 1);
        assert_eq!(state.redo.len(), 1);
    }

    #[test]
    fn png_and_webp_preserve_alpha_and_apply_compress_quality() {
        let mut image = RgbaImage::new(8, 8);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([
                (x * 29 + y * 7) as u8,
                (x * 11 + y * 23) as u8,
                (x * 17 + y * 13) as u8,
                if x == y { 90 } else { 255 },
            ]);
        }
        let document = Document::new(image);
        let expected = render(&document).unwrap();
        for format in ["PNG", "WebP"] {
            let preserved = image::load_from_memory(
                &encode_output(&document, format, ExportQuality::Preserve).unwrap(),
            )
            .unwrap()
            .to_rgba8();
            let compressed = image::load_from_memory(
                &encode_output(&document, format, ExportQuality::Compress(55)).unwrap(),
            )
            .unwrap()
            .to_rgba8();
            assert_eq!(preserved, expected, "{format} preserve must be pixel exact");
            assert_eq!(
                compressed.get_pixel(3, 3).0[3],
                expected.get_pixel(3, 3).0[3]
            );
            assert_ne!(compressed, expected, "{format} compress must affect pixels");
        }
    }
}
