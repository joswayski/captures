use base64::{Engine as _, engine::general_purpose::STANDARD};
use gtk::cairo;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{f64::consts::PI, io::Cursor};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}
impl Rect {
    pub fn normalized(a: Point, b: Point) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color(pub f64, pub f64, pub f64, pub f64);
impl Default for Color {
    fn default() -> Self {
        Self(0.95, 0.2, 0.23, 1.)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}
impl Blend {
    pub const ALL: [Self; 6] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::Darken => "Darken",
            Self::Lighten => "Lighten",
        }
    }
    fn operator(self) -> cairo::Operator {
        match self {
            Self::Normal => cairo::Operator::Over,
            Self::Multiply => cairo::Operator::Multiply,
            Self::Screen => cairo::Operator::Screen,
            Self::Overlay => cairo::Operator::Overlay,
            Self::Darken => cairo::Operator::Darken,
            Self::Lighten => cairo::Operator::Lighten,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    Image { png: String, original_png: String },
    Stroke(Vec<Point>),
    Arrow(Point, Point),
    Line(Point, Point),
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Star,
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub kind: LayerKind,
    pub frame: Rect,
    pub rotation: f64,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f64,
    pub blend: Blend,
    pub color: Color,
    pub fill: Option<Color>,
    pub stroke: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub background: Option<Color>,
    pub layers: Vec<Layer>,
    pub next_id: u64,
}

fn encode(image: &RgbaImage) -> String {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("memory PNG");
    STANDARD.encode(out.into_inner())
}
fn decode(value: &str) -> Result<RgbaImage, String> {
    image::load_from_memory(&STANDARD.decode(value).map_err(|e| e.to_string())?)
        .map(|i| i.to_rgba8())
        .map_err(|e| e.to_string())
}

impl Document {
    pub fn new(image: RgbaImage) -> Self {
        let (w, h) = image.dimensions();
        let png = encode(&image);
        Self {
            width: w,
            height: h,
            // Opening an image must not silently flatten its transparent pixels.
            // The editor uses a light document surface for presentation; only an
            // explicit Background color selection becomes part of the export.
            background: None,
            next_id: 2,
            layers: vec![Layer {
                id: 1,
                name: "Original screenshot".into(),
                kind: LayerKind::Image {
                    png: png.clone(),
                    original_png: png,
                },
                frame: Rect {
                    x: 0.,
                    y: 0.,
                    w: w as f64,
                    h: h as f64,
                },
                rotation: 0.,
                visible: true,
                locked: true,
                opacity: 1.,
                blend: Blend::Normal,
                color: Color::default(),
                fill: None,
                stroke: 1.,
            }],
        }
    }
    #[cfg(test)]
    pub fn transparent(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            background: None,
            layers: vec![],
            next_id: 1,
        }
    }
    pub fn add(&mut self, kind: LayerKind, frame: Rect, color: Color, stroke: f64) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let name = match &kind {
            LayerKind::Image { .. } => "Image",
            LayerKind::Stroke(_) => "Freehand",
            LayerKind::Arrow(..) => "Arrow",
            LayerKind::Line(..) => "Line",
            LayerKind::Rectangle => "Rectangle",
            LayerKind::Ellipse => "Ellipse",
            LayerKind::Triangle => "Triangle",
            LayerKind::Diamond => "Diamond",
            LayerKind::Star => "Star",
            LayerKind::Text(_) => "Text",
        };
        self.layers.push(Layer {
            id,
            name: name.into(),
            kind,
            frame,
            rotation: 0.,
            visible: true,
            locked: false,
            opacity: 1.,
            blend: Blend::Normal,
            color,
            fill: None,
            stroke,
        });
        self.layers.len() - 1
    }
    pub fn add_image(&mut self, image: RgbaImage, at: Point) -> usize {
        let (w, h) = image.dimensions();
        let png = encode(&image);
        self.add(
            LayerKind::Image {
                png: png.clone(),
                original_png: png,
            },
            Rect {
                x: at.x,
                y: at.y,
                w: w as f64,
                h: h as f64,
            },
            Color::default(),
            1.,
        )
    }
    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        let mut l = self.layers.get(index)?.clone();
        l.id = self.next_id;
        self.next_id += 1;
        l.name = format!("{} copy", l.name);
        l.locked = false;
        l.visible = true;
        l.frame.x += 12.;
        l.frame.y += 12.;
        self.layers.push(l);
        Some(self.layers.len() - 1)
    }
    pub fn reorder(&mut self, index: usize, new_index: usize) -> Option<usize> {
        if index >= self.layers.len() || new_index >= self.layers.len() || self.layers[index].locked
        {
            return None;
        }
        let locked_floor = self.layers.iter().take_while(|layer| layer.locked).count();
        let new_index = new_index.max(locked_floor);
        let l = self.layers.remove(index);
        self.layers.insert(new_index, l);
        Some(new_index)
    }
    pub fn crop(&mut self, r: Rect) {
        let x = r.x.clamp(0., self.width as f64) as u32;
        let y = r.y.clamp(0., self.height as f64) as u32;
        let right = (r.x + r.w).clamp(0., self.width as f64) as u32;
        let bottom = (r.y + r.h).clamp(0., self.height as f64) as u32;
        if right <= x || bottom <= y {
            return;
        }
        self.width = right - x;
        self.height = bottom - y;
        for l in &mut self.layers {
            l.frame.x -= f64::from(x);
            l.frame.y -= f64::from(y)
        }
    }
}

pub fn source_image(ctx: &cairo::Context, image: &RgbaImage, x: f64, y: f64, w: f64, h: f64) {
    let mut data = Vec::with_capacity(image.as_raw().len());
    for p in image.pixels() {
        let a = u32::from(p[3]);
        let word = (a << 24)
            | ((u32::from(p[0]) * a / 255) << 16)
            | ((u32::from(p[1]) * a / 255) << 8)
            | (u32::from(p[2]) * a / 255);
        data.extend_from_slice(&word.to_ne_bytes())
    }
    let surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        image.width() as i32,
        image.height() as i32,
        image.width() as i32 * 4,
    )
    .expect("image surface");
    let _ = ctx.save();
    ctx.translate(x, y);
    ctx.scale(w / image.width() as f64, h / image.height() as f64);
    let _ = ctx.set_source_surface(&surface, 0., 0.);
    let _ = ctx.paint();
    let _ = ctx.restore();
}
fn path(ctx: &cairo::Context, l: &Layer) {
    match &l.kind {
        LayerKind::Stroke(ps) => {
            if let Some(p) = ps.first() {
                ctx.move_to(p.x, p.y);
                for p in &ps[1..] {
                    ctx.line_to(p.x, p.y)
                }
            }
        }
        LayerKind::Arrow(a, b) => {
            ctx.move_to(a.x, a.y);
            ctx.line_to(b.x, b.y)
        }
        LayerKind::Line(a, b) => {
            ctx.move_to(a.x, a.y);
            ctx.line_to(b.x, b.y)
        }
        LayerKind::Rectangle => ctx.rectangle(0., 0., l.frame.w, l.frame.h),
        LayerKind::Ellipse => {
            let _ = ctx.save();
            ctx.translate(l.frame.w / 2., l.frame.h / 2.);
            ctx.scale((l.frame.w / 2.).max(1.), (l.frame.h / 2.).max(1.));
            ctx.arc(0., 0., 1., 0., 2. * PI);
            let _ = ctx.restore();
        }
        LayerKind::Triangle => {
            ctx.move_to(l.frame.w / 2., 0.);
            ctx.line_to(l.frame.w, l.frame.h);
            ctx.line_to(0., l.frame.h);
            ctx.close_path();
        }
        LayerKind::Diamond => {
            ctx.move_to(l.frame.w / 2., 0.);
            ctx.line_to(l.frame.w, l.frame.h / 2.);
            ctx.line_to(l.frame.w / 2., l.frame.h);
            ctx.line_to(0., l.frame.h / 2.);
            ctx.close_path();
        }
        LayerKind::Star => {
            for index in 0..10 {
                let angle = -PI / 2. + index as f64 * PI / 5.;
                let radius = if index % 2 == 0 { 1. } else { 0.39 };
                let point = Point {
                    x: l.frame.w / 2. + angle.cos() * l.frame.w / 2. * radius,
                    y: l.frame.h / 2. + angle.sin() * l.frame.h / 2. * radius,
                };
                if index == 0 {
                    ctx.move_to(point.x, point.y)
                } else {
                    ctx.line_to(point.x, point.y)
                }
            }
            ctx.close_path();
        }
        _ => {}
    }
}
pub fn draw_layer(ctx: &cairo::Context, l: &Layer) -> Result<(), String> {
    if !l.visible {
        return Ok(());
    }
    let _ = ctx.save();
    ctx.set_operator(l.blend.operator());
    ctx.translate(l.frame.x + l.frame.w / 2., l.frame.y + l.frame.h / 2.);
    ctx.rotate(l.rotation);
    ctx.translate(-l.frame.w / 2., -l.frame.h / 2.);
    match &l.kind {
        LayerKind::Image { png, .. } => {
            source_image(ctx, &decode(png)?, 0., 0., l.frame.w, l.frame.h)
        }
        LayerKind::Text(text) => {
            ctx.set_source_rgba(l.color.0, l.color.1, l.color.2, l.color.3 * l.opacity);
            ctx.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            ctx.set_font_size((l.frame.h * 0.72).max(8.));
            ctx.move_to(0., l.frame.h * 0.78);
            let _ = ctx.show_text(text);
        }
        _ => {
            path(ctx, l);
            if let Some(c) = l.fill {
                ctx.set_source_rgba(c.0, c.1, c.2, c.3 * l.opacity);
                let _ = ctx.fill_preserve();
            }
            ctx.set_source_rgba(l.color.0, l.color.1, l.color.2, l.color.3 * l.opacity);
            ctx.set_line_width(l.stroke);
            ctx.set_line_cap(cairo::LineCap::Round);
            ctx.set_line_join(cairo::LineJoin::Round);
            let _ = ctx.stroke();
            if let LayerKind::Arrow(a, b) = &l.kind {
                let angle = (b.y - a.y).atan2(b.x - a.x);
                let size = (l.stroke * 4.).max(12.);
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
        }
    }
    let _ = ctx.restore();
    Ok(())
}
pub fn paint(ctx: &cairo::Context, doc: &Document) -> Result<(), String> {
    ctx.set_operator(cairo::Operator::Source);
    let background = doc.background.unwrap_or(Color(0., 0., 0., 0.));
    ctx.set_source_rgba(background.0, background.1, background.2, background.3);
    let _ = ctx.paint();
    ctx.set_operator(cairo::Operator::Over);
    for l in &doc.layers {
        draw_layer(ctx, l)?
    }
    Ok(())
}
/// Rasterize only this layer at sidebar size. Canvas coordinates and clipping
/// stay the same, but no full-resolution document copy or RGBA readback is needed.
pub fn render_layer_thumbnail(
    doc: &Document,
    layer: &Layer,
    width: i32,
    height: i32,
) -> Result<cairo::ImageSurface, String> {
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|e| e.to_string())?;
    let context = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    context.scale(
        width as f64 / doc.width as f64,
        height as f64 / doc.height as f64,
    );
    draw_layer(&context, layer)?;
    Ok(surface)
}

pub fn render(doc: &Document) -> Result<RgbaImage, String> {
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, doc.width as i32, doc.height as i32)
            .map_err(|e| e.to_string())?;
    {
        let c = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        paint(&c, doc)?
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|e| e.to_string())?;
    let mut out = RgbaImage::new(doc.width, doc.height);
    for y in 0..doc.height as usize {
        for x in 0..doc.width as usize {
            let i = y * stride + x * 4;
            let mut px = [data[i + 2], data[i + 1], data[i], data[i + 3]];
            let a = px[3];
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
                }
            }
            out.put_pixel(x as u32, y as u32, image::Rgba(px))
        }
    }
    Ok(out)
}

pub fn local_point(l: &Layer, p: Point) -> Point {
    let cx = l.frame.x + l.frame.w / 2.;
    let cy = l.frame.y + l.frame.h / 2.;
    let dx = p.x - cx;
    let dy = p.y - cy;
    let c = (-l.rotation).cos();
    let s = (-l.rotation).sin();
    Point {
        x: dx * c - dy * s + l.frame.w / 2.,
        y: dx * s + dy * c + l.frame.h / 2.,
    }
}
pub fn hit(l: &Layer, p: Point) -> bool {
    l.visible
        && l.frame.w > 0.
        && l.frame.h > 0.
        && Rect {
            x: 0.,
            y: 0.,
            w: l.frame.w,
            h: l.frame.h,
        }
        .contains(local_point(l, p))
}
pub fn rotate_handle(l: &Layer) -> Point {
    let p = Point {
        x: l.frame.w / 2.,
        y: -28.,
    };
    let c = l.rotation.cos();
    let s = l.rotation.sin();
    let dx = p.x - l.frame.w / 2.;
    let dy = p.y - l.frame.h / 2.;
    Point {
        x: l.frame.x + l.frame.w / 2. + dx * c - dy * s,
        y: l.frame.y + l.frame.h / 2. + dx * s + dy * c,
    }
}
pub fn resize_layer(l: &mut Layer, start: Rect, handle: usize, p: Point, keep_aspect: bool) {
    let min = 4.;
    let mut left = start.x;
    let mut top = start.y;
    let mut right = start.x + start.w;
    let mut bottom = start.y + start.h;
    if handle.is_multiple_of(3) {
        left = p.x.min(right - min)
    }
    if handle % 3 == 2 {
        right = p.x.max(left + min)
    }
    if handle < 3 {
        top = p.y.min(bottom - min)
    }
    if handle > 5 {
        bottom = p.y.max(top + min)
    }
    let mut w = right - left;
    let mut h = bottom - top;
    if keep_aspect {
        let ratio = start.w / start.h;
        if w / h > ratio {
            h = w / ratio
        } else {
            w = h * ratio
        }
        if handle.is_multiple_of(3) {
            left = right - w
        } else {
            right = left + w
        }
        if handle < 3 {
            top = bottom - h
        } else {
            bottom = top + h
        }
    }
    l.frame = Rect {
        x: left,
        y: top,
        w: right - left,
        h: bottom - top,
    }
}
pub fn erase_soft(
    l: &mut Layer,
    at: Point,
    radius: f64,
    restore: bool,
    softness: f64,
) -> Result<(), String> {
    let p = local_point(l, at);
    let frame = l.frame;
    let LayerKind::Image { png, original_png } = &mut l.kind else {
        return Err("Select an image layer to erase".into());
    };
    let mut image = decode(png)?;
    let original = decode(original_png)?;
    let sx = image.width() as f64 / frame.w;
    let sy = image.height() as f64 / frame.h;
    let cx = p.x * sx;
    let cy = p.y * sy;
    let rr = radius * ((sx + sy) / 2.);
    for y in ((cy - rr).floor().max(0.) as u32)
        ..=((cy + rr)
            .ceil()
            .min(image.height().saturating_sub(1) as f64) as u32)
    {
        for x in ((cx - rr).floor().max(0.) as u32)
            ..=((cx + rr).ceil().min(image.width().saturating_sub(1) as f64) as u32)
        {
            let distance = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            if distance <= rr {
                let hardness = (1. - softness.clamp(0., 1.)).max(0.01);
                let strength = if distance <= rr * hardness {
                    1.
                } else {
                    1. - (distance - rr * hardness) / (rr * (1. - hardness))
                };
                if restore {
                    let target = original.get_pixel(x, y).0;
                    let current = &mut image.get_pixel_mut(x, y).0;
                    for channel in 0..4 {
                        current[channel] = (current[channel] as f64
                            + (target[channel] as f64 - current[channel] as f64) * strength)
                            .round() as u8;
                    }
                } else {
                    let alpha = &mut image.get_pixel_mut(x, y).0[3];
                    *alpha = (*alpha as f64 * (1. - strength)).round() as u8;
                }
            }
        }
    }
    *png = encode(&image);
    Ok(())
}

pub fn remove_color(
    l: &mut Layer,
    at: Point,
    tolerance: u8,
    contiguous: bool,
) -> Result<(), String> {
    let p = local_point(l, at);
    let frame = l.frame;
    let LayerKind::Image { png, .. } = &mut l.kind else {
        return Err("Select an image layer for the wand".into());
    };
    let mut image = decode(png)?;
    let x = (p.x * image.width() as f64 / frame.w)
        .floor()
        .clamp(0., image.width().saturating_sub(1) as f64) as u32;
    let y = (p.y * image.height() as f64 / frame.h)
        .floor()
        .clamp(0., image.height().saturating_sub(1) as f64) as u32;
    let sample = image.get_pixel(x, y).0;
    let matches = |pixel: [u8; 4]| -> bool {
        pixel[3] != 0
            && pixel[..3]
                .iter()
                .zip(sample[..3].iter())
                .map(|(a, b)| (i16::from(*a) - i16::from(*b)).unsigned_abs())
                .max()
                .unwrap_or(0)
                <= u16::from(tolerance)
    };
    if contiguous {
        let mut queue = std::collections::VecDeque::from([(x, y)]);
        let mut seen = vec![false; image.width() as usize * image.height() as usize];
        while let Some((x, y)) = queue.pop_front() {
            let index = y as usize * image.width() as usize + x as usize;
            if seen[index] {
                continue;
            }
            seen[index] = true;
            if !matches(image.get_pixel(x, y).0) {
                continue;
            }
            image.get_pixel_mut(x, y).0 = [0, 0, 0, 0];
            if x > 0 {
                queue.push_back((x - 1, y));
            }
            if x + 1 < image.width() {
                queue.push_back((x + 1, y));
            }
            if y > 0 {
                queue.push_back((x, y - 1));
            }
            if y + 1 < image.height() {
                queue.push_back((x, y + 1));
            }
        }
    } else {
        for pixel in image.pixels_mut() {
            if matches(pixel.0) {
                pixel.0 = [0, 0, 0, 0];
            }
        }
    }
    *png = encode(&image);
    Ok(())
}

pub fn content_bounds(doc: &Document) -> Option<Rect> {
    doc.layers
        .iter()
        .filter(|l| l.visible)
        .map(layer_bounds)
        .reduce(|a, b| {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            Rect {
                x,
                y,
                w: (a.x + a.w).max(b.x + b.w) - x,
                h: (a.y + a.h).max(b.y + b.h) - y,
            }
        })
}
pub fn trim_to_content(doc: &mut Document, padding: f64) {
    let Some(r) = content_bounds(doc) else { return };
    let padding = padding.max(0.).round();
    let x = r.x.floor() - padding;
    let y = r.y.floor() - padding;
    let right = (r.x + r.w).ceil() + padding;
    let bottom = (r.y + r.h).ceil() + padding;
    let width = (right - x).max(1.) as u32;
    let height = (bottom - y).max(1.) as u32;
    if x == 0. && y == 0. && width == doc.width && height == doc.height {
        return;
    }
    doc.width = width;
    doc.height = height;
    for layer in &mut doc.layers {
        layer.frame.x -= x;
        layer.frame.y -= y;
    }
}
pub fn expand_to_content(doc: &mut Document, padding: f64) {
    let Some(r) = content_bounds(doc) else { return };
    let left = (padding - r.x).max(0.);
    let top = (padding - r.y).max(0.);
    let right = (r.x + r.w + padding - doc.width as f64).max(0.);
    let bottom = (r.y + r.h + padding - doc.height as f64).max(0.);
    if left + top + right + bottom == 0. {
        return;
    }
    for l in &mut doc.layers {
        l.frame.x += left;
        l.frame.y += top
    }
    doc.width = (doc.width as f64 + left + right).ceil() as u32;
    doc.height = (doc.height as f64 + top + bottom).ceil() as u32
}

pub fn snap_translation(doc: &Document, index: usize, frame: Rect, threshold: f64) -> Rect {
    let mut x = frame.x;
    let mut y = frame.y;
    let mut xs = vec![0., doc.width as f64];
    let mut ys = vec![0., doc.height as f64];
    for (i, l) in doc.layers.iter().enumerate() {
        if i != index && l.visible {
            let r = layer_bounds(l);
            xs.extend([r.x, r.x + r.w]);
            ys.extend([r.y, r.y + r.h]);
        }
    }
    for edge in [frame.x, frame.x + frame.w] {
        if let Some(target) = xs
            .iter()
            .copied()
            .filter(|v| (edge - v).abs() <= threshold)
            .min_by(|a, b| (edge - a).abs().total_cmp(&(edge - b).abs()))
        {
            x += target - edge;
            break;
        }
    }
    for edge in [frame.y, frame.y + frame.h] {
        if let Some(target) = ys
            .iter()
            .copied()
            .filter(|v| (edge - v).abs() <= threshold)
            .min_by(|a, b| (edge - a).abs().total_cmp(&(edge - b).abs()))
        {
            y += target - edge;
            break;
        }
    }
    Rect { x, y, ..frame }
}

pub fn paste_layer(doc: &mut Document, source: &Layer, after: Option<usize>, offset: f64) -> usize {
    let mut layer = source.clone();
    layer.id = doc.next_id;
    doc.next_id += 1;
    layer.name = format!("{} copy", source.name);
    layer.locked = false;
    layer.visible = true;
    layer.frame.x += offset;
    layer.frame.y += offset;
    let index = after.map_or(doc.layers.len(), |index| (index + 1).min(doc.layers.len()));
    doc.layers.insert(index, layer);
    index
}
pub fn transform_image(l: &mut Layer, clockwise: bool) {
    if !matches!(l.kind, LayerKind::Image { .. }) {
        return;
    }
    l.rotation += if clockwise { PI / 2. } else { -PI / 2. };
    std::mem::swap(&mut l.frame.w, &mut l.frame.h);
    l.rotation = 0.;
    if let LayerKind::Image { png, original_png } = &mut l.kind
        && let (Ok(i), Ok(o)) = (decode(png), decode(original_png))
    {
        let (i, o) = if clockwise {
            (image::imageops::rotate90(&i), image::imageops::rotate90(&o))
        } else {
            (
                image::imageops::rotate270(&i),
                image::imageops::rotate270(&o),
            )
        };
        *png = encode(&i);
        *original_png = encode(&o)
    }
}
pub fn flip_image(l: &mut Layer, horizontal: bool) {
    if let LayerKind::Image { png, original_png } = &mut l.kind
        && let (Ok(i), Ok(o)) = (decode(png), decode(original_png))
    {
        let (i, o) = if horizontal {
            (
                image::imageops::flip_horizontal(&i),
                image::imageops::flip_horizontal(&o),
            )
        } else {
            (
                image::imageops::flip_vertical(&i),
                image::imageops::flip_vertical(&o),
            )
        };
        *png = encode(&i);
        *original_png = encode(&o)
    }
}
pub fn merge_indices(doc: &mut Document, indices: &[usize], name: &str) -> Result<usize, String> {
    if indices.is_empty() {
        return Err("No layers to merge".into());
    }
    let mut subset = doc.clone();
    subset.layers = indices
        .iter()
        .filter_map(|i| doc.layers.get(*i).cloned())
        .collect();
    let image = render(&subset)?;
    let first = *indices.iter().min().unwrap();
    for i in indices.iter().copied().rev() {
        doc.layers.remove(i);
    }
    let png = encode(&image);
    doc.layers.insert(
        first,
        Layer {
            id: doc.next_id,
            name: name.into(),
            kind: LayerKind::Image {
                png: png.clone(),
                original_png: png,
            },
            frame: Rect {
                x: 0.,
                y: 0.,
                w: doc.width as f64,
                h: doc.height as f64,
            },
            rotation: 0.,
            visible: true,
            locked: false,
            opacity: 1.,
            blend: Blend::Normal,
            color: Color::default(),
            fill: None,
            stroke: 1.,
        },
    );
    doc.next_id += 1;
    Ok(first)
}
pub fn layer_bounds(l: &Layer) -> Rect {
    let c = l.rotation.cos().abs();
    let s = l.rotation.sin().abs();
    let w = l.frame.w * c + l.frame.h * s;
    let h = l.frame.w * s + l.frame.h * c;
    Rect {
        x: l.frame.x + (l.frame.w - w) / 2.,
        y: l.frame.y + (l.frame.h - h) / 2.,
        w,
        h,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_alpha_is_preserved_until_background_is_explicitly_enabled() {
        let image = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([200, 100, 50, 0])
            } else {
                image::Rgba([20, 40, 60, 255])
            }
        });
        let mut document = Document::new(image);
        assert_eq!(document.background, None);
        assert_eq!(render(&document).unwrap().get_pixel(0, 0).0[3], 0);

        document.background = Some(Color(1., 1., 1., 1.));
        assert_eq!(render(&document).unwrap().get_pixel(0, 0).0, [255; 4]);
    }

    fn thumbnail_pixel(doc: &Document, layer: &Layer, x: usize, y: usize) -> u32 {
        let mut surface = render_layer_thumbnail(doc, layer, 38, 30).unwrap();
        assert_eq!((surface.width(), surface.height()), (38, 30));
        let offset = y * surface.stride() as usize + x * 4;
        let data = surface.data().unwrap();
        u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn thumbnails_preserve_canvas_coordinates_alpha_rotation_and_visibility() {
        let mut doc = Document::transparent(380, 300);
        let index = doc.add(
            LayerKind::Rectangle,
            Rect {
                x: 40.,
                y: 60.,
                w: 100.,
                h: 80.,
            },
            Color(1., 0., 0., 1.),
            0.,
        );
        doc.layers[index].fill = Some(Color(1., 0., 0., 1.));
        doc.layers[index].opacity = 0.5;
        let layer = doc.layers[index].clone();
        // A second layer must not leak into this layer's thumbnail.
        doc.layers[index].frame = Rect {
            x: 0.,
            y: 0.,
            w: 380.,
            h: 300.,
        };
        assert_eq!(thumbnail_pixel(&doc, &layer, 9, 10), 0x80800000);
        assert_eq!(thumbnail_pixel(&doc, &layer, 1, 1), 0);
        assert_eq!(thumbnail_pixel(&doc, &layer, 6, 14), 0);
        let mut rotated = layer.clone();
        rotated.rotation = PI / 2.;
        assert_eq!(thumbnail_pixel(&doc, &rotated, 6, 14), 0x80800000);
        rotated.visible = false;
        assert_eq!(thumbnail_pixel(&doc, &rotated, 6, 14), 0);
        let mut clipped = layer;
        clipped.frame.x = -50.;
        assert_eq!(thumbnail_pixel(&doc, &clipped, 0, 10), 0x80800000);
        assert_eq!(thumbnail_pixel(&doc, &clipped, 6, 10), 0);
    }

    #[test]
    fn thumbnail_image_keeps_pixel_orientation_and_transparency() {
        let mut doc = Document::new(image());
        doc.width = 380;
        doc.height = 300;
        // Each source pixel covers one thumbnail pixel, at a nonzero origin.
        doc.layers[0].frame = Rect {
            x: 30.,
            y: 60.,
            w: 100.,
            h: 80.,
        };
        assert_eq!(thumbnail_pixel(&doc, &doc.layers[0], 3, 6), 0xff14508c);
        assert_eq!(thumbnail_pixel(&doc, &doc.layers[0], 5, 9), 0x805f0f23);
        assert_eq!(thumbnail_pixel(&doc, &doc.layers[0], 2, 9), 0);
    }

    #[test]
    #[ignore = "manual release-profile thumbnail pipeline benchmark"]
    fn benchmark_layer_thumbnails() {
        use std::{hint::black_box, time::Instant};
        for (width, height) in [(1920, 1080), (3840, 2160)] {
            let mut doc = Document::new(RgbaImage::from_fn(width, height, |x, y| {
                image::Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 233) as u8, 255])
            }));
            for i in 0..12 {
                doc.add(
                    LayerKind::Rectangle,
                    Rect {
                        x: (i * 70) as f64,
                        y: (i * 35) as f64,
                        w: 300.,
                        h: 160.,
                    },
                    Color::default(),
                    4.,
                );
            }
            let old = || {
                for layer in &doc.layers {
                    let mut single = doc.clone();
                    single.layers = vec![layer.clone()];
                    let image = render(&single).unwrap();
                    black_box(
                        crate::ui::pixbuf(&image)
                            .scale_simple(38, 30, gtk::gdk_pixbuf::InterpType::Bilinear)
                            .unwrap(),
                    );
                }
            };
            let new = || {
                for layer in &doc.layers {
                    black_box(render_layer_thumbnail(&doc, layer, 38, 30).unwrap());
                }
            };
            old();
            new();
            let mut old_ms = Vec::new();
            let mut new_ms = Vec::new();
            for sample in 0..5 {
                for baseline in [sample % 2 == 0, sample % 2 != 0] {
                    let start = Instant::now();
                    if baseline {
                        old();
                    } else {
                        new();
                    }
                    let elapsed = start.elapsed().as_secs_f64() * 1000.;
                    if baseline {
                        old_ms.push(elapsed);
                    } else {
                        new_ms.push(elapsed);
                    }
                }
            }
            println!(
                "{}",
                serde_json::json!({
                    "width": width, "height": height, "layers": doc.layers.len(),
                    "old_ms": old_ms, "new_ms": new_ms,
                })
            );
        }
    }

    fn image() -> RgbaImage {
        let mut i = RgbaImage::from_pixel(10, 8, image::Rgba([20, 80, 140, 255]));
        i.put_pixel(2, 3, image::Rgba([190, 30, 70, 128]));
        i
    }
    #[test]
    fn alpha_and_crop_clip_without_flattening() {
        let mut d = Document::new(image());
        d.layers[0].locked = false;
        d.add(
            LayerKind::Rectangle,
            Rect {
                x: 6.,
                y: 1.,
                w: 2.,
                h: 2.,
            },
            Color(1., 0., 0., 1.),
            2.,
        );
        d.crop(Rect {
            x: 2.,
            y: 1.,
            w: 6.,
            h: 5.,
        });
        assert_eq!((d.width, d.height, d.layers.len()), (6, 5, 2));
        assert_eq!(d.layers[1].frame.x, 4.);
        let p = render(&d).unwrap().get_pixel(0, 2).0;
        assert_eq!(p[3], 128);
    }
    #[test]
    fn rotated_hit_and_resize_are_in_layer_space() {
        let mut d = Document::transparent(200, 200);
        let i = d.add(
            LayerKind::Rectangle,
            Rect {
                x: 20.,
                y: 30.,
                w: 80.,
                h: 40.,
            },
            Color::default(),
            2.,
        );
        d.layers[i].rotation = PI / 2.;
        assert!(hit(&d.layers[i], Point { x: 60., y: 50. }));
        assert!(!hit(&d.layers[i], Point { x: 100., y: 31. }));
        let start = d.layers[i].frame;
        resize_layer(&mut d.layers[i], start, 8, Point { x: 180., y: 110. }, true);
        assert!((d.layers[i].frame.w / d.layers[i].frame.h - 2.).abs() < 1e-6);
    }
    #[test]
    fn erase_and_restore_preserve_rgb_and_alpha() {
        let mut d = Document::new(image());
        let p = Point { x: 2., y: 3. };
        erase_soft(&mut d.layers[0], p, 1., false, 0.).unwrap();
        assert_eq!(render(&d).unwrap().get_pixel(2, 3).0[3], 0);
        erase_soft(&mut d.layers[0], p, 1., true, 0.).unwrap();
        let out = render(&d).unwrap().get_pixel(2, 3).0;
        assert_eq!(out[3], 128);
        assert!((out[0] as i16 - 190).abs() <= 1);
    }
    #[test]
    fn serde_round_trip_keeps_pixels_and_transforms() {
        let mut d = Document::new(image());
        d.layers[0].rotation = 0.37;
        d.layers[0].opacity = 0.4;
        let json = serde_json::to_vec(&d).unwrap();
        let restored: Document = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored.layers[0].rotation, 0.37);
        assert_eq!(render(&restored).unwrap(), render(&d).unwrap());
    }
    #[test]
    fn reorder_duplicate_and_snapshot_undo_are_distinct() {
        let mut d = Document::new(image());
        let before = d.clone();
        let i = d.duplicate(0).unwrap();
        assert!(!d.layers[i].locked);
        let moved = d.reorder(i, 0).unwrap();
        assert_eq!(moved, 1);
        assert_eq!(d.layers[0].id, before.layers[0].id);
        let top = d.duplicate(0).unwrap();
        assert_eq!(d.reorder(top, 1), Some(1));
        assert_eq!(d.layers[1].id, 3);
        d = before;
        assert_eq!(d.layers.len(), 1);
    }

    #[test]
    fn wand_uses_max_channel_tolerance_and_respects_contiguity() {
        let mut pixels = RgbaImage::from_pixel(5, 1, image::Rgba([20, 30, 40, 255]));
        pixels.put_pixel(1, 0, image::Rgba([56, 30, 40, 255]));
        pixels.put_pixel(2, 0, image::Rgba([90, 30, 40, 255]));
        pixels.put_pixel(3, 0, image::Rgba([20, 30, 40, 255]));
        let mut contiguous = Document::new(pixels.clone());
        remove_color(&mut contiguous.layers[0], Point { x: 0., y: 0. }, 36, true).unwrap();
        let contiguous_pixels = decode(match &contiguous.layers[0].kind {
            LayerKind::Image { png, .. } => png,
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(contiguous_pixels.get_pixel(1, 0).0, [0, 0, 0, 0]);
        assert_eq!(contiguous_pixels.get_pixel(2, 0).0[3], 255);
        assert_eq!(contiguous_pixels.get_pixel(3, 0).0[3], 255);

        let mut global = Document::new(pixels);
        remove_color(&mut global.layers[0], Point { x: 0., y: 0. }, 36, false).unwrap();
        let global_pixels = decode(match &global.layers[0].kind {
            LayerKind::Image { png, .. } => png,
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(global_pixels.get_pixel(3, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn soft_erase_has_opaque_edge_and_partial_falloff() {
        let pixels = RgbaImage::from_pixel(9, 9, image::Rgba([80, 90, 100, 255]));
        let mut document = Document::new(pixels);
        erase_soft(
            &mut document.layers[0],
            Point { x: 4., y: 4. },
            4.,
            false,
            1.,
        )
        .unwrap();
        let output = decode(match &document.layers[0].kind {
            LayerKind::Image { png, .. } => png,
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(output.get_pixel(4, 4).0[3], 0);
        assert!(matches!(output.get_pixel(7, 4).0[3], 1..=254));
        assert_eq!(output.get_pixel(8, 4).0[3], 255);
    }

    #[test]
    fn added_closed_shapes_paint_inside_their_distinct_geometry() {
        for kind in [LayerKind::Triangle, LayerKind::Diamond, LayerKind::Star] {
            let mut document = Document::transparent(40, 40);
            let index = document.add(
                kind,
                Rect {
                    x: 5.,
                    y: 5.,
                    w: 30.,
                    h: 30.,
                },
                Color(1., 0., 0., 1.),
                2.,
            );
            document.layers[index].fill = Some(Color(1., 0., 0., 1.));
            let output = render(&document).unwrap();
            assert!(output.get_pixel(20, 20).0[3] > 200);
            assert_eq!(output.get_pixel(6, 6).0[3], 0);
        }
    }

    #[test]
    fn trim_keeps_overhanging_content_and_expand_only_grows() {
        let mut document = Document::transparent(100, 80);
        document.add(
            LayerKind::Rectangle,
            Rect {
                x: -10.2,
                y: 12.4,
                w: 40.,
                h: 20.,
            },
            Color::default(),
            1.,
        );
        trim_to_content(&mut document, 2.);
        assert_eq!((document.width, document.height), (45, 25));
        assert!((document.layers[0].frame.x - 2.8).abs() < 1e-9);

        document.layers[0].frame.x = -4.;
        document.layers[0].frame.y = -3.;
        expand_to_content(&mut document, 1.);
        assert_eq!((document.width, document.height), (50, 29));
        assert_eq!(document.layers[0].frame.x, 1.);
        assert_eq!(document.layers[0].frame.y, 1.);
    }

    #[test]
    fn snap_chooses_closest_edge_and_clipboard_inserts_after_selection() {
        let mut document = Document::transparent(200, 100);
        document.add(
            LayerKind::Rectangle,
            Rect {
                x: 80.,
                y: 10.,
                w: 20.,
                h: 20.,
            },
            Color::default(),
            1.,
        );
        let moving = document.add(
            LayerKind::Rectangle,
            Rect {
                x: 57.,
                y: 32.,
                w: 20.,
                h: 20.,
            },
            Color::default(),
            1.,
        );
        let snapped = snap_translation(&document, moving, document.layers[moving].frame, 4.);
        assert_eq!((snapped.x, snapped.y), (60., 30.));

        let copied = document.layers[0].clone();
        let pasted = paste_layer(&mut document, &copied, Some(0), 24.);
        assert_eq!(pasted, 1);
        assert_eq!(document.layers[pasted].frame.x, 104.);
        assert!(!document.layers[pasted].locked);
        assert_ne!(document.layers[pasted].id, copied.id);
    }
}
