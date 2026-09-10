//! Native GTK screenshot editor.  The document stays in memory until an export succeeds;
//! the source image is never modified.
use crate::ui;
use gtk::{cairo, gdk, prelude::*};
use image::RgbaImage;
use std::{cell::RefCell, f64::consts::PI, path::PathBuf, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}
impl Rect {
    fn normalized(a: Point, b: Point) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }
    fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
}

#[derive(Clone)]
enum LayerKind {
    Stroke(Vec<Point>),
    Arrow(Point, Point),
    Rectangle(Rect),
    Ellipse(Rect),
    Text { at: Point, text: String },
    Image { at: Point, image: RgbaImage },
}
#[derive(Clone)]
struct Layer {
    kind: LayerKind,
    color: gdk::RGBA,
    width: f64,
}

#[derive(Clone)]
struct Document {
    width: u32,
    height: u32,
    background: RgbaImage,
    layers: Vec<Layer>,
}
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Draw,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Crop,
}
struct State {
    doc: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    tool: Tool,
    selected: Option<usize>,
    drag_start: Option<Point>,
    before_drag: Option<Document>,
    draft: Option<LayerKind>,
    zoom: f64,
    color: gdk::RGBA,
    stroke: f64,
}

fn image_layer_bounds(at: Point, image: &RgbaImage) -> Rect {
    Rect {
        x: at.x,
        y: at.y,
        w: image.width() as f64,
        h: image.height() as f64,
    }
}
fn layer_bounds(layer: &Layer) -> Rect {
    match &layer.kind {
        LayerKind::Stroke(points) => points.iter().fold(
            Rect {
                x: f64::INFINITY,
                y: f64::INFINITY,
                w: 0.,
                h: 0.,
            },
            |r, p| {
                if !r.x.is_finite() {
                    Rect {
                        x: p.x,
                        y: p.y,
                        w: 0.,
                        h: 0.,
                    }
                } else {
                    let x = r.x.min(p.x);
                    let y = r.y.min(p.y);
                    Rect {
                        x,
                        y,
                        w: (r.x + r.w).max(p.x) - x,
                        h: (r.y + r.h).max(p.y) - y,
                    }
                }
            },
        ),
        LayerKind::Arrow(a, b) => Rect::normalized(*a, *b),
        LayerKind::Rectangle(r) | LayerKind::Ellipse(r) => *r,
        LayerKind::Text { at, text } => Rect {
            x: at.x,
            y: at.y - 24.,
            w: (text.chars().count() as f64 * 14.).max(14.),
            h: 30.,
        },
        LayerKind::Image { at, image } => image_layer_bounds(*at, image),
    }
}
fn distance_segment(p: Point, a: Point, b: Point) -> f64 {
    let d = Point {
        x: b.x - a.x,
        y: b.y - a.y,
    };
    let l = d.x * d.x + d.y * d.y;
    if l == 0. {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / l).clamp(0., 1.);
    ((p.x - a.x - t * d.x).powi(2) + (p.y - a.y - t * d.y).powi(2)).sqrt()
}
fn hit(layer: &Layer, p: Point) -> bool {
    match &layer.kind {
        LayerKind::Stroke(ps) => ps
            .windows(2)
            .any(|v| distance_segment(p, v[0], v[1]) <= layer.width + 5.),
        LayerKind::Arrow(a, b) => distance_segment(p, *a, *b) <= layer.width + 7.,
        LayerKind::Ellipse(r) => {
            let nx = (p.x - (r.x + r.w / 2.)) / (r.w / 2.).max(1.);
            let ny = (p.y - (r.y + r.h / 2.)) / (r.h / 2.).max(1.);
            (nx * nx + ny * ny - 1.).abs() < 0.15
        }
        _ => layer_bounds(layer).contains(p),
    }
}
fn translate(layer: &mut Layer, dx: f64, dy: f64) {
    let mv = |p: &mut Point| {
        p.x += dx;
        p.y += dy;
    };
    match &mut layer.kind {
        LayerKind::Stroke(ps) => ps.iter_mut().for_each(mv),
        LayerKind::Arrow(a, b) => {
            mv(a);
            mv(b)
        }
        LayerKind::Rectangle(r) | LayerKind::Ellipse(r) => {
            r.x += dx;
            r.y += dy
        }
        LayerKind::Text { at, .. } | LayerKind::Image { at, .. } => mv(at),
    }
}

fn source_image(ctx: &cairo::Context, image: &RgbaImage, x: f64, y: f64) {
    // Cairo surfaces are thread-safe to construct on the export worker; GDK calls are not.
    let mut data = Vec::with_capacity(image.as_raw().len());
    for p in image.pixels() {
        let a = u32::from(p[3]);
        let word = (a << 24)
            | ((u32::from(p[0]) * a / 255) << 16)
            | ((u32::from(p[1]) * a / 255) << 8)
            | (u32::from(p[2]) * a / 255);
        data.extend_from_slice(&word.to_ne_bytes());
    }
    let surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        image.width() as i32,
        image.height() as i32,
        image.width() as i32 * 4,
    )
    .expect("valid image surface");
    let _ = ctx.set_source_surface(&surface, x, y);
    let _ = ctx.paint();
}
fn draw_layer(ctx: &cairo::Context, l: &Layer) {
    ctx.set_source_rgba(
        l.color.red(),
        l.color.green(),
        l.color.blue(),
        l.color.alpha(),
    );
    ctx.set_line_width(l.width);
    ctx.set_line_cap(cairo::LineCap::Round);
    ctx.set_line_join(cairo::LineJoin::Round);
    match &l.kind {
        LayerKind::Stroke(ps) => {
            if let Some(p) = ps.first() {
                ctx.move_to(p.x, p.y);
                for p in &ps[1..] {
                    ctx.line_to(p.x, p.y)
                }
                let _ = ctx.stroke();
            }
        }
        LayerKind::Arrow(a, b) => {
            ctx.move_to(a.x, a.y);
            ctx.line_to(b.x, b.y);
            let _ = ctx.stroke();
            let angle = (b.y - a.y).atan2(b.x - a.x);
            let size = (l.width * 4.).max(12.);
            ctx.move_to(b.x, b.y);
            ctx.line_to(
                b.x - size * (angle - PI / 6.).cos(),
                b.y - size * (angle - PI / 6.).sin(),
            );
            ctx.move_to(b.x, b.y);
            ctx.line_to(
                b.x - size * (angle + PI / 6.).cos(),
                b.y - size * (angle + PI / 6.).sin(),
            );
            let _ = ctx.stroke();
        }
        LayerKind::Rectangle(r) => {
            ctx.rectangle(r.x, r.y, r.w, r.h);
            let _ = ctx.stroke();
        }
        LayerKind::Ellipse(r) => {
            let _ = ctx.save();
            ctx.translate(r.x + r.w / 2., r.y + r.h / 2.);
            ctx.scale((r.w / 2.).max(1.), (r.h / 2.).max(1.));
            ctx.arc(0., 0., 1., 0., 2. * PI);
            let _ = ctx.restore();
            let _ = ctx.stroke();
        }
        LayerKind::Text { at, text } => {
            ctx.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            ctx.set_font_size(24.);
            ctx.move_to(at.x, at.y);
            let _ = ctx.show_text(text);
        }
        LayerKind::Image { at, image } => source_image(ctx, image, at.x, at.y),
    }
}
fn paint(ctx: &cairo::Context, doc: &Document) {
    source_image(ctx, &doc.background, 0., 0.);
    for l in &doc.layers {
        draw_layer(ctx, l)
    }
}

fn render(doc: &Document) -> Result<RgbaImage, String> {
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, doc.width as i32, doc.height as i32)
            .map_err(|e| e.to_string())?;
    {
        let c = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        paint(&c, doc);
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|e| e.to_string())?;
    let mut out = RgbaImage::new(doc.width, doc.height);
    for y in 0..doc.height as usize {
        for x in 0..doc.width as usize {
            let i = y * stride + x * 4;
            #[cfg(target_endian = "little")]
            let px = [data[i + 2], data[i + 1], data[i], data[i + 3]];
            #[cfg(target_endian = "big")]
            let px = [data[i + 1], data[i + 2], data[i + 3], data[i]];
            let mut px = px;
            let alpha = px[3];
            if alpha != 0 && alpha != 255 {
                for channel in &mut px[..3] {
                    *channel =
                        ((*channel as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8;
                }
            }
            out.put_pixel(x as u32, y as u32, image::Rgba(px));
        }
    }
    Ok(out)
}
fn checkpoint(s: &mut State) {
    s.undo.push(s.doc.clone());
    if s.undo.len() > 50 {
        s.undo.remove(0);
    }
    s.redo.clear()
}

pub fn open(image: RgbaImage, directory: PathBuf, on_saved: Rc<dyn Fn(PathBuf)>) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — Image editor");
    window.set_default_size(1100, 760);
    let state = Rc::new(RefCell::new(State {
        doc: Document {
            width: image.width(),
            height: image.height(),
            background: image,
            layers: vec![],
        },
        undo: vec![],
        redo: vec![],
        tool: Tool::Select,
        selected: None,
        drag_start: None,
        before_drag: None,
        draft: None,
        zoom: 1.,
        color: ui::color("signal"),
        stroke: 4.,
    }));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.style_context().add_class("toolbar");
    root.pack_start(&bar, false, false, 0);
    let area = gtk::DrawingArea::new();
    area.set_can_focus(true);
    area.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::SCROLL_MASK,
    );
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.add(&area);
    root.pack_start(&scroll, true, true, 0);
    let status = ui::label(
        "Select and move layers. Shortcuts: V/D/A/R/E/T/C, Ctrl+Z/Y, Delete, +/-.",
        "muted",
    );
    root.pack_start(&status, false, false, 8);
    let tools = [
        ("Select", Tool::Select),
        ("Draw", Tool::Draw),
        ("Arrow", Tool::Arrow),
        ("Rectangle", Tool::Rectangle),
        ("Ellipse", Tool::Ellipse),
        ("Text", Tool::Text),
        ("Crop", Tool::Crop),
    ];
    for (name, tool) in tools {
        let b = ui::button(name);
        let s = state.clone();
        let a = area.clone();
        b.connect_clicked(move |_| {
            s.borrow_mut().tool = tool;
            a.queue_draw()
        });
        bar.pack_start(&b, false, false, 0)
    }
    let undo = ui::button("Undo");
    {
        let s = state.clone();
        let a = area.clone();
        undo.connect_clicked(move |_| {
            let mut s = s.borrow_mut();
            if let Some(d) = s.undo.pop() {
                let old = std::mem::replace(&mut s.doc, d);
                s.redo.push(old);
                s.selected = None;
                a.queue_draw()
            }
        });
    }
    bar.pack_start(&undo, false, false, 0);
    let redo = ui::button("Redo");
    {
        let s = state.clone();
        let a = area.clone();
        redo.connect_clicked(move |_| {
            let mut s = s.borrow_mut();
            if let Some(d) = s.redo.pop() {
                let old = std::mem::replace(&mut s.doc, d);
                s.undo.push(old);
                s.selected = None;
                a.queue_draw()
            }
        });
    }
    bar.pack_start(&redo, false, false, 0);
    let import = ui::button("Add image");
    {
        let s = state.clone();
        let a = area.clone();
        let w = window.clone();
        import.connect_clicked(move |_| {
            if let Some(p) = ui::open_file(&w) {
                match image::open(&p) {
                    Ok(i) => {
                        let mut s = s.borrow_mut();
                        checkpoint(&mut s);
                        s.doc.layers.push(Layer {
                            kind: LayerKind::Image {
                                at: Point { x: 20., y: 20. },
                                image: i.to_rgba8(),
                            },
                            color: gdk::RGBA::WHITE,
                            width: 1.,
                        });
                        s.selected = Some(s.doc.layers.len() - 1);
                        a.queue_draw()
                    }
                    Err(e) => ui::error(&w, &e.to_string()),
                }
            }
        });
    }
    bar.pack_start(&import, false, false, 0);
    let resize = ui::button("Canvas…");
    {
        let s = state.clone();
        let a = area.clone();
        let w = window.clone();
        resize.connect_clicked(move |_| canvas_dialog(&w, &s, &a));
    }
    bar.pack_start(&resize, false, false, 0);
    let rotate = ui::button("Rotate 90°");
    {
        let s = state.clone();
        let a = area.clone();
        let w = window.clone();
        rotate.connect_clicked(move |_| {
            let mut s = s.borrow_mut();
            match render(&s.doc) {
                Ok(flattened) => {
                    checkpoint(&mut s);
                    let (old_width, old_height) = (s.doc.width, s.doc.height);
                    s.doc.background = image::imageops::rotate90(&flattened);
                    s.doc.width = old_height;
                    s.doc.height = old_width;
                    s.doc.layers.clear();
                    s.selected = None;
                    a.queue_draw();
                }
                Err(error) => ui::error(&w, &error),
            }
        });
    }
    bar.pack_start(&rotate, false, false, 0);
    let format = gtk::ComboBoxText::new();
    for f in ["PNG", "JPEG", "WebP"] {
        format.append_text(f)
    }
    format.set_active(Some(0));
    bar.pack_end(&format, false, false, 0);
    let save = ui::button("Export");
    save.style_context().add_class("primary");
    {
        let s = state.clone();
        let w = window.clone();
        let format = format.clone();
        let directory = directory.clone();
        let cb = on_saved.clone();
        save.connect_clicked(move |b| {
            b.set_sensitive(false);
            let b = b.clone();
            let w = w.clone();
            let doc = s.borrow().doc.clone();
            let fmt = format
                .active_text()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "PNG".into());
            let dir = directory.clone();
            let cb = cb.clone();
            ui::job(
                move || {
                    let image = render(&doc)?;
                    let kind = match fmt.as_str() {
                        "JPEG" => image::ImageFormat::Jpeg,
                        "WebP" => image::ImageFormat::WebP,
                        _ => image::ImageFormat::Png,
                    };
                    let ext = fmt.to_ascii_lowercase().replace("jpeg", "jpg");
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    let image = if kind == image::ImageFormat::Jpeg {
                        let mut opaque = RgbaImage::from_pixel(
                            image.width(),
                            image.height(),
                            image::Rgba([255, 255, 255, 255]),
                        );
                        image::imageops::overlay(&mut opaque, &image, 0, 0);
                        image::DynamicImage::ImageRgb8(
                            image::DynamicImage::ImageRgba8(opaque).to_rgb8(),
                        )
                    } else {
                        image::DynamicImage::ImageRgba8(image)
                    };
                    image
                        .write_to(&mut bytes, kind)
                        .map_err(|e| e.to_string())?;
                    ui::save_bytes(&dir, &ext, bytes.get_ref())
                },
                move |r| {
                    b.set_sensitive(true);
                    match r {
                        Ok(p) => cb(p),
                        Err(e) => ui::error(&w, &e),
                    }
                },
            )
        });
    }
    bar.pack_end(&save, false, false, 0);
    let copy = ui::button("Copy");
    {
        let s = state.clone();
        let w = window.clone();
        copy.connect_clicked(move |_| match render(&s.borrow().doc) {
            Ok(i) => gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_image(&ui::pixbuf(&i)),
            Err(e) => ui::error(&w, &e),
        });
    }
    bar.pack_end(&copy, false, false, 0);
    setup_canvas(&area, &state, &window);
    setup_keys(&window, &state, &area);
    window.add(&root);
    window.show_all();
}

fn canvas_dialog(parent: &gtk::Window, state: &Rc<RefCell<State>>, area: &gtk::DrawingArea) {
    let d = gtk::Dialog::with_buttons(
        Some("Canvas size"),
        Some(parent),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Resize", gtk::ResponseType::Accept),
        ],
    );
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let w = gtk::SpinButton::with_range(1., 16384., 1.);
    let h = gtk::SpinButton::with_range(1., 16384., 1.);
    w.set_value(state.borrow().doc.width as f64);
    h.set_value(state.borrow().doc.height as f64);
    row.pack_start(&gtk::Label::new(Some("Width")), false, false, 0);
    row.pack_start(&w, false, false, 0);
    row.pack_start(&gtk::Label::new(Some("Height")), false, false, 0);
    row.pack_start(&h, false, false, 0);
    d.content_area().add(&row);
    d.show_all();
    if d.run() == gtk::ResponseType::Accept {
        let mut s = state.borrow_mut();
        checkpoint(&mut s);
        let nw = w.value() as u32;
        let nh = h.value() as u32;
        let mut bg = RgbaImage::from_pixel(nw, nh, image::Rgba([255, 255, 255, 255]));
        image::imageops::overlay(&mut bg, &s.doc.background, 0, 0);
        s.doc.background = bg;
        s.doc.width = nw;
        s.doc.height = nh;
        area.queue_draw()
    }
    d.close()
}

fn setup_canvas(area: &gtk::DrawingArea, state: &Rc<RefCell<State>>, window: &gtk::Window) {
    let s = state.clone();
    area.connect_draw(move |a, c| {
        let s = s.borrow();
        a.set_size_request(
            (s.doc.width as f64 * s.zoom) as i32,
            (s.doc.height as f64 * s.zoom) as i32,
        );
        c.scale(s.zoom, s.zoom);
        paint(c, &s.doc);
        if let Some(d) = &s.draft {
            draw_layer(
                c,
                &Layer {
                    kind: d.clone(),
                    color: s.color,
                    width: s.stroke,
                },
            )
        }
        if let Some(layer) = s.selected.and_then(|i| s.doc.layers.get(i)) {
            let r = layer_bounds(layer);
            let accent = ui::color("accent");
            c.set_source_rgba(accent.red(), accent.green(), accent.blue(), 1.);
            c.set_line_width(1. / s.zoom);
            c.set_dash(&[5. / s.zoom], 0.);
            c.rectangle(r.x - 3., r.y - 3., r.w + 6., r.h + 6.);
            let _ = c.stroke();
        }
        gtk::glib::Propagation::Proceed
    });
    let press_state = state.clone();
    let a = area.clone();
    let w = window.clone();
    area.connect_button_press_event(move |_, e| {
        let mut s = press_state.borrow_mut();
        let (x, y) = e.position();
        let p = Point {
            x: x / s.zoom,
            y: y / s.zoom,
        };
        s.drag_start = Some(p);
        s.before_drag = Some(s.doc.clone());
        match s.tool {
            Tool::Select => s.selected = s.doc.layers.iter().rposition(|l| hit(l, p)),
            Tool::Draw => s.draft = Some(LayerKind::Stroke(vec![p])),
            Tool::Arrow => s.draft = Some(LayerKind::Arrow(p, p)),
            Tool::Rectangle => s.draft = Some(LayerKind::Rectangle(Rect::normalized(p, p))),
            Tool::Ellipse => s.draft = Some(LayerKind::Ellipse(Rect::normalized(p, p))),
            Tool::Crop => s.draft = Some(LayerKind::Rectangle(Rect::normalized(p, p))),
            Tool::Text => {
                s.drag_start = None;
                s.before_drag = None;
                drop(s);
                text_dialog(&w, &press_state, &a, p);
                return gtk::glib::Propagation::Stop;
            }
        }
        a.queue_draw();
        gtk::glib::Propagation::Stop
    });
    let s = state.clone();
    let a = area.clone();
    area.connect_motion_notify_event(move |_, e| {
        let mut s = s.borrow_mut();
        let Some(start) = s.drag_start else {
            return gtk::glib::Propagation::Proceed;
        };
        let (x, y) = e.position();
        let p = Point {
            x: x / s.zoom,
            y: y / s.zoom,
        };
        let tool = s.tool;
        match (&mut s.draft, tool) {
            (Some(LayerKind::Stroke(ps)), Tool::Draw) => ps.push(p),
            (Some(LayerKind::Arrow(_, b)), Tool::Arrow) => *b = p,
            (Some(LayerKind::Rectangle(r)), Tool::Rectangle | Tool::Crop) => {
                *r = Rect::normalized(start, p)
            }
            (Some(LayerKind::Ellipse(r)), Tool::Ellipse) => *r = Rect::normalized(start, p),
            (None, Tool::Select) => {
                if let Some(i) = s.selected
                    && let Some(before) = &s.before_drag
                {
                    let mut l = before.layers[i].clone();
                    translate(&mut l, p.x - start.x, p.y - start.y);
                    s.doc.layers[i] = l
                }
            }
            _ => {}
        }
        a.queue_draw();
        gtk::glib::Propagation::Stop
    });
    let s = state.clone();
    let a = area.clone();
    area.connect_button_release_event(move |_, _| {
        let mut s = s.borrow_mut();
        if let Some(before) = s.before_drag.take() {
            match s.tool {
                Tool::Crop => {
                    if let Some(LayerKind::Rectangle(r)) = s.draft.take()
                        && r.w >= 1.
                        && r.h >= 1.
                    {
                        s.undo.push(before);
                        crop(&mut s.doc, r);
                        s.selected = None;
                        s.redo.clear()
                    }
                }
                Tool::Draw | Tool::Arrow | Tool::Rectangle | Tool::Ellipse => {
                    if let Some(kind) = s.draft.take()
                        && layer_bounds(&Layer {
                            kind: kind.clone(),
                            color: s.color,
                            width: s.stroke,
                        })
                        .w + layer_bounds(&Layer {
                            kind: kind.clone(),
                            color: s.color,
                            width: s.stroke,
                        })
                        .h > 1.
                    {
                        s.undo.push(before);
                        let layer = Layer {
                            kind,
                            color: s.color,
                            width: s.stroke,
                        };
                        s.doc.layers.push(layer);
                        s.selected = Some(s.doc.layers.len() - 1);
                        s.redo.clear()
                    }
                }
                Tool::Select => {
                    if s.drag_start.is_some()
                        && s.selected.is_some()
                        && s.doc
                            .layers
                            .iter()
                            .zip(before.layers.iter())
                            .any(|(a, b)| layer_bounds(a) != layer_bounds(b))
                    {
                        s.undo.push(before);
                        s.redo.clear()
                    }
                }
                _ => {}
            }
        }
        s.drag_start = None;
        a.queue_draw();
        gtk::glib::Propagation::Stop
    });
}

fn crop(doc: &mut Document, r: Rect) {
    let x = r.x.clamp(0., doc.width as f64) as u32;
    let y = r.y.clamp(0., doc.height as f64) as u32;
    let right = (r.x + r.w).clamp(0., doc.width as f64) as u32;
    let bottom = (r.y + r.h).clamp(0., doc.height as f64) as u32;
    let w = right.saturating_sub(x);
    let h = bottom.saturating_sub(y);
    if w == 0 || h == 0 {
        return;
    }
    let rendered = render(doc).unwrap_or_else(|_| doc.background.clone());
    doc.background = image::imageops::crop_imm(&rendered, x, y, w, h).to_image();
    doc.width = w;
    doc.height = h;
    doc.layers.clear()
}
fn text_dialog(
    parent: &gtk::Window,
    state: &Rc<RefCell<State>>,
    area: &gtk::DrawingArea,
    at: Point,
) {
    let d = gtk::Dialog::with_buttons(
        Some("Add text"),
        Some(parent),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Add", gtk::ResponseType::Accept),
        ],
    );
    let entry = gtk::Entry::new();
    entry.set_activates_default(true);
    d.content_area().add(&entry);
    d.set_default_response(gtk::ResponseType::Accept);
    d.show_all();
    if d.run() == gtk::ResponseType::Accept && !entry.text().trim().is_empty() {
        let mut s = state.borrow_mut();
        checkpoint(&mut s);
        let layer = Layer {
            kind: LayerKind::Text {
                at: Point {
                    x: at.x,
                    y: at.y + 24.,
                },
                text: entry.text().to_string(),
            },
            color: s.color,
            width: s.stroke,
        };
        s.doc.layers.push(layer);
        s.selected = Some(s.doc.layers.len() - 1);
        area.queue_draw()
    }
    d.close()
}
fn setup_keys(window: &gtk::Window, state: &Rc<RefCell<State>>, area: &gtk::DrawingArea) {
    let s = state.clone();
    let a = area.clone();
    window.connect_key_press_event(move |_, e| {
        let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
        let key = e.keyval();
        let mut s = s.borrow_mut();
        if ctrl && (key == gdk::keys::constants::z || key == gdk::keys::constants::y) {
            let redo = key == gdk::keys::constants::y;
            if redo {
                if let Some(d) = s.redo.pop() {
                    let old = std::mem::replace(&mut s.doc, d);
                    s.undo.push(old)
                }
            } else if let Some(d) = s.undo.pop() {
                let old = std::mem::replace(&mut s.doc, d);
                s.redo.push(old)
            }
            s.selected = None;
            a.queue_draw();
            return gtk::glib::Propagation::Stop;
        }
        match key {
            gdk::keys::constants::v => s.tool = Tool::Select,
            gdk::keys::constants::d => s.tool = Tool::Draw,
            gdk::keys::constants::a => s.tool = Tool::Arrow,
            gdk::keys::constants::r => s.tool = Tool::Rectangle,
            gdk::keys::constants::e => s.tool = Tool::Ellipse,
            gdk::keys::constants::t => s.tool = Tool::Text,
            gdk::keys::constants::c => s.tool = Tool::Crop,
            gdk::keys::constants::Delete => {
                if let Some(i) = s.selected.take() {
                    checkpoint(&mut s);
                    s.doc.layers.remove(i);
                }
            }
            gdk::keys::constants::plus | gdk::keys::constants::equal => {
                s.zoom = (s.zoom * 1.2).min(4.)
            }
            gdk::keys::constants::minus => s.zoom = (s.zoom / 1.2).max(0.1),
            _ => return gtk::glib::Propagation::Proceed,
        }
        a.queue_draw();
        gtk::glib::Propagation::Stop
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renderer_preserves_channel_order_alpha_and_crop_origin() {
        let mut background = RgbaImage::from_pixel(8, 6, image::Rgba([21, 83, 147, 255]));
        background.put_pixel(3, 2, image::Rgba([190, 30, 70, 128]));
        let mut doc = Document {
            width: 8,
            height: 6,
            background,
            layers: vec![],
        };
        let rendered = render(&doc).unwrap();
        assert_eq!(rendered.get_pixel(0, 0).0, [21, 83, 147, 255]);
        let p = rendered.get_pixel(3, 2).0;
        assert_eq!(p[3], 128);
        for (actual, expected) in p[..3].iter().zip([190i16, 30, 70]) {
            assert!((i16::from(*actual) - expected).abs() <= 1);
        }
        crop(
            &mut doc,
            Rect {
                x: 3.,
                y: 2.,
                w: 4.,
                h: 3.,
            },
        );
        assert_eq!(doc.background.dimensions(), (4, 3));
        assert_eq!(doc.background.get_pixel(0, 0).0, p);
        crop(
            &mut doc,
            Rect {
                x: 100.,
                y: 100.,
                w: 2.,
                h: 3.,
            },
        );
        assert_eq!(doc.background.dimensions(), (4, 3));
    }

    #[test]
    fn negative_crop_intersects_instead_of_shifting_the_requested_area() {
        let mut doc = Document {
            width: 8,
            height: 6,
            background: RgbaImage::new(8, 6),
            layers: vec![],
        };
        crop(
            &mut doc,
            Rect {
                x: -2.,
                y: -1.,
                w: 5.,
                h: 4.,
            },
        );
        assert_eq!(doc.background.dimensions(), (3, 3));
    }

    fn layer(kind: LayerKind) -> Layer {
        Layer {
            kind,
            color: gdk::RGBA::BLACK,
            width: 4.,
        }
    }
    #[test]
    fn rect_normalizes() {
        assert_eq!(
            Rect::normalized(Point { x: 8., y: 7. }, Point { x: 2., y: 3. }),
            Rect {
                x: 2.,
                y: 3.,
                w: 6.,
                h: 4.
            }
        )
    }
    #[test]
    fn arrow_hit_is_geometric() {
        let l = layer(LayerKind::Arrow(
            Point { x: 0., y: 0. },
            Point { x: 100., y: 0. },
        ));
        assert!(hit(&l, Point { x: 50., y: 5. }));
        assert!(!hit(&l, Point { x: 50., y: 30. }))
    }
    #[test]
    fn translated_stroke_moves_every_point() {
        let mut l = layer(LayerKind::Stroke(vec![
            Point { x: 1., y: 2. },
            Point { x: 3., y: 4. },
        ]));
        translate(&mut l, 5., -1.);
        match l.kind {
            LayerKind::Stroke(p) => assert_eq!(p[1], Point { x: 8., y: 3. }),
            _ => unreachable!(),
        }
    }
}
