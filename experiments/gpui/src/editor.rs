//! Screenshot editor GPUI surface.
//!
//! The chrome deliberately follows the shared React editor rather than the
//! Windows experiment.  Pixel/document behavior is delegated to the portable,
//! tested `captures-windows-native` model.

use captures_windows_native::{
    draft::{DraftIdentity, DraftStore},
    editor::{BlendMode, Document, Shape, Tool, resize_from_corner},
    encoder::{
        encode_jpeg, encode_jpeg_with_limit, encode_png, encode_png_with_limit, encode_webp,
        encode_webp_with_limit,
    },
    geometry::{Point, Rect},
};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    Launch,
    theme::{Theme, font},
};

const ACCENT: [u8; 4] = [255, 202, 40, 255];

#[derive(Clone, Copy)]
enum EditorIcon {
    Select,
    Crop,
    Text,
    Pen,
    Arrow,
    Rectangle,
    Ellipse,
    Line,
    Triangle,
    Diamond,
    Star,
    Eraser,
}

fn editor_icon(icon: EditorIcon, color: &'static str) -> Img {
    let body = match icon {
        EditorIcon::Select => r#"<path d="m5 3 12 9-6 1-3 6Z"/>"#,
        EditorIcon::Crop => r#"<path d="M7 3v14a2 2 0 0 0 2 2h12M3 7h14a2 2 0 0 1 2 2v12"/>"#,
        EditorIcon::Text => r#"<path d="M5 5h14M12 5v14M8 19h8"/>"#,
        EditorIcon::Pen => r#"<path d="M4 17c4-7 6-8 8-5s3 3 8-5M4 20h16"/>"#,
        EditorIcon::Arrow => r#"<path d="M5 19 19 5M10 5h9v9"/>"#,
        EditorIcon::Rectangle => r#"<rect x="4" y="5" width="16" height="14" rx="2"/>"#,
        EditorIcon::Ellipse => r#"<ellipse cx="12" cy="12" rx="8" ry="6"/>"#,
        EditorIcon::Line => r#"<path d="m5 19 14-14"/>"#,
        EditorIcon::Triangle => r#"<path d="m12 4 9 16H3Z"/>"#,
        EditorIcon::Diamond => r#"<path d="m12 3 9 9-9 9-9-9Z"/>"#,
        EditorIcon::Star => {
            r#"<path d="m12 3 2.7 5.6 6.3.9-4.6 4.4 1.1 6.1-5.5-2.9L6.5 20l1.1-6.1L3 9.5l6.3-.9Z"/>"#
        }
        EditorIcon::Eraser => r#"<path d="m14 4 6 6-9 9H5l-2-2Z"/><path d="m8 12 6 6M11 19h10"/>"#,
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(18.))
    .h(px(18.))
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Webp,
}

#[derive(Clone, Copy)]
struct ExportSpec {
    format: Format,
    quality: u8,
    max_bytes: Option<u64>,
    width: u32,
    height: u32,
}

struct CompressionPreview {
    before: Arc<RenderImage>,
    after: Arc<RenderImage>,
    before_bytes: usize,
    after_bytes: usize,
}

impl Format {
    fn from_preference(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Self::Jpeg,
            "webp" => Self::Webp,
            _ => Self::Png,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
}

pub struct ScreenshotEditor {
    launch: Launch,
    theme: Theme,
    focus: Option<FocusHandle>,
    text: Option<Entity<crate::preferences::input::TextInput>>,
    export_width_input: Option<Entity<crate::preferences::input::TextInput>>,
    export_height_input: Option<Entity<crate::preferences::input::TextInput>>,
    quality_input: Option<Entity<crate::preferences::input::TextInput>>,
    max_size_input: Option<Entity<crate::preferences::input::TextInput>>,
    canvas_width_input: Option<Entity<crate::preferences::input::TextInput>>,
    canvas_height_input: Option<Entity<crate::preferences::input::TextInput>>,
    stroke_input: Option<Entity<crate::preferences::input::TextInput>>,
    stroke_color_input: Option<Entity<crate::preferences::input::TextInput>>,
    fill_color_input: Option<Entity<crate::preferences::input::TextInput>>,
    text_size_input: Option<Entity<crate::preferences::input::TextInput>>,
    text_wrap_input: Option<Entity<crate::preferences::input::TextInput>>,
    shadow_blur_input: Option<Entity<crate::preferences::input::TextInput>>,
    shadow_x_input: Option<Entity<crate::preferences::input::TextInput>>,
    shadow_y_input: Option<Entity<crate::preferences::input::TextInput>>,
    shadow_color_input: Option<Entity<crate::preferences::input::TextInput>>,
    shadow_opacity_input: Option<Entity<crate::preferences::input::TextInput>>,
    synced_text_layer: Option<(u64, u64)>,
    text_font_family: &'static str,
    text_font: Option<Arc<[u8]>>,
    document: Document,
    identity: DraftIdentity,
    source_path: Option<PathBuf>,
    rendered: Arc<RenderImage>,
    canvas_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    tool: Tool,
    selected: Option<u64>,
    zoom: u16,
    fit: bool,
    format: Format,
    quality: u8,
    export_open: bool,
    export_scale: u8,
    export_width: u32,
    export_height: u32,
    export_max_bytes: Option<u64>,
    export_aspect_locked: bool,
    destination: PathBuf,
    compression_preview: Option<CompressionPreview>,
    compression_preview_pending: bool,
    compression_preview_error: Option<String>,
    compression_preview_request: u64,
    compression_preview_revision: u64,
    document_revision: u64,
    compression_split: u8,
    pan: gpui::Point<Pixels>,
    last_canvas_point: Option<Point>,
    space_down: bool,
    style_color: [u8; 4],
    style_fill: Option<[u8; 4]>,
    style_stroke: f32,
    style_font_size: f32,
    status: String,
}

#[derive(Clone)]
enum Drag {
    Draw {
        start: Point,
        points: Vec<Point>,
    },
    Move {
        start: Point,
        original: captures_windows_native::editor::Layer,
    },
    Resize {
        corner: usize,
        original: captures_windows_native::editor::Layer,
    },
    Rotate {
        center: Point,
        pointer_offset: f32,
        original: captures_windows_native::editor::Layer,
    },
    Pan {
        start: gpui::Point<Pixels>,
        original: gpui::Point<Pixels>,
    },
    Erase {
        target: captures_windows_native::editor::ImageTarget,
        points: Vec<Point>,
    },
}

pub fn open(launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    if !matches!(launch.view.as_str(), "screenshot-editor" | "viewer") {
        anyhow::bail!("editor cannot open view {:?}", launch.view);
    }
    crate::preferences::input::bind_keys(cx);
    let editor = ScreenshotEditor::load(launch)?;
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Captures Screenshot editor".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let mut editor = editor;
                let focus = cx.focus_handle();
                focus.focus(window);
                editor.focus = Some(focus);
                editor
            })
        },
    )?;
    Ok(())
}

impl ScreenshotEditor {
    fn load(launch: Launch) -> anyhow::Result<Self> {
        fs::create_dir_all(launch.profile.join("drafts"))?;
        fs::create_dir_all(launch.profile.join("captures"))?;
        let (pixels, source_path, identity) = if let Some(path) = launch.path.as_ref() {
            let pixels = decode(path)?;
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            (
                pixels,
                Some(canonical.clone()),
                DraftIdentity::imported(canonical),
            )
        } else if launch.mock {
            (mock_artwork(), None, DraftIdentity::new_capture())
        } else {
            anyhow::bail!("Screenshot editor requires --open FILE (or --mock)");
        };
        let store = DraftStore::new(&launch.profile.join("drafts"));
        let document = store
            .load(&identity, source_path.as_deref())
            .map_err(anyhow::Error::msg)?
            .unwrap_or_else(|| Document::new(pixels));
        let rendered = render_image(&document.render().map_err(anyhow::Error::msg)?);
        let settings = crate::preferences::settings::load(&launch.profile).unwrap_or_default();
        let format = Format::from_preference(&settings.screenshot_format);
        let destination = PathBuf::from(&settings.output_directory)
            .join(format!("Capture-edited.{}", format.extension()));
        let (export_width, export_height) = (document.canvas_width, document.canvas_height);
        Ok(Self {
            theme: Theme::new(launch.light),
            focus: None,
            text: None,
            export_width_input: None,
            export_height_input: None,
            quality_input: None,
            max_size_input: None,
            canvas_width_input: None,
            canvas_height_input: None,
            stroke_input: None,
            stroke_color_input: None,
            fill_color_input: None,
            text_size_input: None,
            text_wrap_input: None,
            shadow_blur_input: None,
            shadow_x_input: None,
            shadow_y_input: None,
            shadow_color_input: None,
            shadow_opacity_input: None,
            synced_text_layer: None,
            text_font_family: "system",
            text_font: None,
            launch,
            document,
            identity,
            source_path,
            rendered,
            canvas_bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            tool: Tool::Select,
            selected: None,
            zoom: 100,
            fit: true,
            format,
            quality: 92,
            export_open: false,
            export_scale: 100,
            export_width,
            export_height,
            export_max_bytes: None,
            export_aspect_locked: true,
            destination,
            compression_preview: None,
            compression_preview_pending: false,
            compression_preview_error: None,
            compression_preview_request: 0,
            compression_preview_revision: u64::MAX,
            document_revision: 0,
            compression_split: 50,
            pan: point(px(0.), px(0.)),
            last_canvas_point: None,
            space_down: false,
            style_color: ACCENT,
            style_fill: None,
            style_stroke: 6.,
            style_font_size: 32.,
            status: "Ready".into(),
        })
    }

    fn refresh(&mut self) {
        self.document_revision = self.document_revision.wrapping_add(1);
        self.compression_preview_request = self.compression_preview_request.wrapping_add(1);
        self.compression_preview_pending = false;
        match self.document.render() {
            Ok(image) => {
                self.rendered = render_image(&image);
                let store = DraftStore::new(&self.launch.profile.join("drafts"));
                self.status =
                    match store.save(&self.identity, self.source_path.as_deref(), &self.document) {
                        Ok(()) => "Draft autosaved".into(),
                        Err(error) => format!("Draft autosave failed: {error}"),
                    };
            }
            Err(error) => self.status = format!("Preview failed: {error}"),
        }
    }

    fn repaint_preview(&mut self) {
        if let Ok(image) = self.document.render() {
            self.rendered = render_image(&image);
        }
    }

    fn choose_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        cx.notify();
    }

    fn document_point(&self, p: gpui::Point<Pixels>) -> Option<Point> {
        let b = self.canvas_bounds.get();
        if !b.contains(&p) {
            return None;
        }
        Some(Point {
            x: (p.x - b.origin.x) / b.size.width * self.document.canvas_width as f32
                + self.document.crop.x,
            y: (p.y - b.origin.y) / b.size.height * self.document.canvas_height as f32
                + self.document.crop.y,
        })
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = &self.focus {
            focus.focus(window);
        }
        if self.space_down || ev.button == MouseButton::Middle {
            self.fit = false;
            self.drag = Some(Drag::Pan {
                start: ev.position,
                original: self.pan,
            });
            cx.notify();
            return;
        }
        let Some(p) = self.document_point(ev.position) else {
            return;
        };
        if self.tool == Tool::Text {
            let result = (|| -> anyhow::Result<()> {
                if self.text_font.is_none() {
                    use font_kit::{
                        family_name::FamilyName, properties::Properties, source::SystemSource,
                    };
                    let font = SystemSource::new()
                        .select_best_match(&[FamilyName::SansSerif], &Properties::new())?
                        .load()?;
                    let bytes = font.copy_font_data().ok_or_else(|| {
                        anyhow::anyhow!("System font cannot be embedded in the draft")
                    })?;
                    self.text_font = Some(Arc::from(bytes.as_slice()));
                }
                let value = self
                    .text
                    .as_ref()
                    .map(|text| text.read(cx).value())
                    .unwrap_or_else(|| "Text".into());
                if !value.trim().is_empty() {
                    let id = self.document.add(
                        Shape::Text {
                            origin: p,
                            value,
                            font_size: self.style_font_size,
                            font_data: self.text_font.clone().unwrap(),
                            style: captures_image::TextStyleSettings::default(),
                        },
                        self.style_color,
                        0.,
                    );
                    self.selected = Some(id);
                    self.refresh();
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.status = error.to_string();
            }
            cx.notify();
            return;
        }
        self.drag = match self.tool {
            Tool::Select => {
                if let Some(selected) = self
                    .selected
                    .and_then(|id| self.document.layers.iter().find(|l| l.id == id))
                {
                    let threshold = 9. * self.document.canvas_width as f32
                        / (self.canvas_bounds.get().size.width / px(1.)).max(1.);
                    if let Some(rotation) = rotation_handle(selected)
                        && (rotation.x - p.x).hypot(rotation.y - p.y) <= threshold
                        && let Some(bounds) = selected.geometry_bounds()
                    {
                        let center = Point {
                            x: bounds.x + bounds.width / 2.,
                            y: bounds.y + bounds.height / 2.,
                        };
                        let pointer_angle = (p.y - center.y).atan2(p.x - center.x).to_degrees();
                        self.drag = Some(Drag::Rotate {
                            center,
                            pointer_offset: pointer_angle - selected.rotation_degrees,
                            original: selected.clone(),
                        });
                        cx.notify();
                        return;
                    }
                    if let Some((corner, _)) = selected
                        .resize_handles()
                        .into_iter()
                        .find(|(_, h)| (h.x - p.x).hypot(h.y - p.y) <= threshold)
                    {
                        self.drag = Some(Drag::Resize {
                            corner,
                            original: selected.clone(),
                        });
                        cx.notify();
                        return;
                    }
                }
                self.document.hit_test(p, 8.).and_then(|id| {
                    self.document
                        .layers
                        .iter()
                        .find(|l| l.id == id && !l.locked)
                        .cloned()
                })
            }
            .map(|original| {
                self.selected = Some(original.id);
                Drag::Move { start: p, original }
            }),
            Tool::Eraser => self.document.hit_test_image(p).map(|target| Drag::Erase {
                target,
                points: vec![p],
            }),
            _ => Some(Drag::Draw {
                start: p,
                points: vec![p],
            }),
        };
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Drag::Pan { start, original }) = &self.drag {
            self.pan = point(
                original.x + ev.position.x - start.x,
                original.y + ev.position.y - start.y,
            );
            cx.notify();
            return;
        }
        let Some(p) = self.document_point(ev.position) else {
            return;
        };
        self.last_canvas_point = Some(p);
        match &mut self.drag {
            Some(Drag::Draw { start, points }) => {
                points.push(p);
                if let Some(shape) =
                    gesture_shape(self.tool, *start, p, points.clone(), ev.modifiers.shift)
                {
                    let mut preview = self.document.clone();
                    let id = preview.add(shape, self.style_color, self.style_stroke);
                    if let Some(fill) = self.style_fill {
                        preview.set_layer_fill(id, Some(fill));
                    }
                    if let Ok(image) = preview.render() {
                        self.rendered = render_image(&image);
                    }
                }
            }
            Some(Drag::Erase { points, .. }) => points.push(p),
            Some(Drag::Move { start, original }) => {
                self.document.preview_layer(original.translated(Point {
                    x: p.x - start.x,
                    y: p.y - start.y,
                }));
                self.repaint_preview();
            }
            Some(Drag::Resize { corner, original }) => {
                if let Some(layer) = resize_from_corner(original, *corner, p) {
                    self.document.preview_layer(layer);
                    self.repaint_preview();
                }
            }
            Some(Drag::Rotate {
                center,
                pointer_offset,
                original,
            }) => {
                let angle = (p.y - center.y).atan2(p.x - center.x).to_degrees() - *pointer_offset;
                let snapped = if ev.modifiers.shift {
                    (angle / 15.).round() * 15.
                } else {
                    angle
                };
                let mut layer = original.clone();
                layer.rotation_degrees = snapped;
                self.document.preview_layer(layer);
                self.repaint_preview();
            }
            Some(Drag::Pan { .. }) => unreachable!(),
            None => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.document_point(ev.position);
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Pan { .. } => {
                cx.notify();
                return;
            }
            Drag::Move { original, .. } => {
                self.document.commit_layer_preview(original);
            }
            Drag::Resize { original, .. } => {
                self.document.commit_layer_preview(original);
            }
            Drag::Rotate { original, .. } => {
                self.document.commit_layer_preview(original);
            }
            Drag::Erase { target, mut points } => {
                if let Some(p) = end {
                    points.push(p);
                }
                let _ = self
                    .document
                    .remove_background_stroke(target, &points, 24., 20., false);
            }
            Drag::Draw { start, mut points } => {
                if let Some(end) = end {
                    points.push(end);
                    if self.tool == Tool::Crop {
                        self.document.set_crop(normalized_rect(start, end, false));
                    } else if let Some(shape) =
                        gesture_shape(self.tool, start, end, points, ev.modifiers.shift)
                    {
                        let id = self
                            .document
                            .add(shape, self.style_color, self.style_stroke);
                        if self.style_fill.is_some() {
                            self.document.set_layer_fill(id, self.style_fill);
                        }
                        self.selected = Some(id);
                    }
                }
            }
        }
        self.refresh();
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.document.undo() {
            self.refresh();
            cx.notify();
        }
    }
    fn redo(&mut self, cx: &mut Context<Self>) {
        if self.document.redo() {
            self.refresh();
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .focus
            .as_ref()
            .is_none_or(|focus| !focus.is_focused(window))
        {
            return;
        }
        let key = event.keystroke.key.as_str();
        let command = if cfg!(target_os = "macos") {
            event.keystroke.modifiers.platform
        } else {
            event.keystroke.modifiers.control
        };
        match key {
            "z" if command && event.keystroke.modifiers.shift => self.redo(cx),
            "z" if command => self.undo(cx),
            "y" if command => self.redo(cx),
            "s" if command => self.export(cx),
            "c" if command => self.copy_image(cx),
            "v" if command => self.paste_image(cx),
            "space" => {
                self.space_down = true;
                cx.notify();
            }
            "backspace" | "delete" => {
                if let Some(id) = self.selected.take() {
                    self.document.delete(id);
                    self.refresh();
                    cx.notify();
                }
            }
            "escape" => {
                if let Some(Drag::Move { original, .. }) = self.drag.take() {
                    self.document.preview_layer(original);
                }
                self.selected = None;
                self.repaint_preview();
                cx.notify();
            }
            "v" => self.choose_tool(Tool::Select, cx),
            "c" => self.choose_tool(Tool::Crop, cx),
            "t" => self.choose_tool(Tool::Text, cx),
            "p" => self.choose_tool(Tool::Pen, cx),
            "a" => self.choose_tool(Tool::Arrow, cx),
            "r" => self.choose_tool(Tool::Rectangle, cx),
            "e" => self.choose_tool(Tool::Ellipse, cx),
            _ => {}
        }
    }

    fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "space" {
            self.space_down = false;
            cx.notify();
        }
    }

    fn paste_image(&mut self, cx: &mut Context<Self>) {
        let image = cx.read_from_clipboard().and_then(|item| {
            item.into_entries().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => image::load_from_memory(&image.bytes).ok(),
                ClipboardEntry::String(_) => None,
            })
        });
        match image {
            Some(image) => {
                self.import_decoded_at(vec![(image.to_rgba8(), "Pasted image".into())], None, cx)
            }
            None => {
                self.status = "Clipboard does not contain an image".into();
                cx.notify();
            }
        }
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let store = DraftStore::new(&self.launch.profile.join("drafts"));
        self.status = match store.save(&self.identity, self.source_path.as_deref(), &self.document)
        {
            Ok(()) => "Draft saved".into(),
            Err(e) => format!("Draft failed: {e}"),
        };
        cx.notify();
    }

    fn apply_export_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let width = parse_dimension("Export width", input_value(&self.export_width_input, cx))?;
        let entered_height =
            parse_dimension("Export height", input_value(&self.export_height_input, cx))?;
        let height = if self.export_aspect_locked {
            proportional_height(
                width,
                self.document.canvas_width,
                self.document.canvas_height,
            )?
        } else {
            entered_height
        };
        let quality = parse_bounded_u8("Quality", input_value(&self.quality_input, cx), 1, 100)?;
        let max_bytes = parse_max_size(input_value(&self.max_size_input, cx))?;
        self.export_width = width;
        self.export_height = height;
        self.quality = quality;
        self.export_max_bytes = max_bytes;
        if self.export_aspect_locked {
            replace_input(&mut self.export_height_input, height.to_string(), cx);
        }
        Ok(())
    }

    fn apply_canvas_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let width = parse_dimension("Canvas width", input_value(&self.canvas_width_input, cx))?;
        let height = parse_dimension("Canvas height", input_value(&self.canvas_height_input, cx))?;
        self.document.set_canvas_size(width, height)?;
        self.refresh();
        Ok(())
    }

    fn apply_style_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.style_stroke =
            parse_f32("Line width", input_value(&self.stroke_input, cx), 0.1, 200.)?;
        self.style_color = parse_color("Stroke", input_value(&self.stroke_color_input, cx))?;
        let fill = input_value(&self.fill_color_input, cx);
        self.style_fill = if fill.trim().is_empty() || fill.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(parse_color("Fill", fill)?)
        };
        self.style_font_size = parse_f32(
            "Text size",
            input_value(&self.text_size_input, cx),
            8.,
            240.,
        )?;
        if self.selected.is_some() {
            let color = self.style_color;
            let stroke = self.style_stroke;
            let font_size = self.style_font_size;
            let fill = self.style_fill;
            self.edit_selected(
                |layer| {
                    layer.color = color;
                    layer.stroke = stroke;
                    layer.fill = fill;
                    if let Shape::Text {
                        font_size: size,
                        style,
                        ..
                    } = &mut layer.shape
                    {
                        *size = font_size;
                        style.background = fill;
                    }
                },
                cx,
            );
        }
        Ok(())
    }

    fn apply_text_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let value = input_value(&self.text, cx);
        let font_size = parse_f32(
            "Text size",
            input_value(&self.text_size_input, cx),
            8.,
            240.,
        )?;
        let width = parse_f32(
            "Wrap width",
            input_value(&self.text_wrap_input, cx),
            20.,
            4000.,
        )?;
        let blur = parse_f32(
            "Shadow blur",
            input_value(&self.shadow_blur_input, cx),
            0.,
            200.,
        )?;
        let offset_x = parse_f32(
            "Shadow X",
            input_value(&self.shadow_x_input, cx),
            -500.,
            500.,
        )?;
        let offset_y = parse_f32(
            "Shadow Y",
            input_value(&self.shadow_y_input, cx),
            -500.,
            500.,
        )?;
        let opacity = parse_f32(
            "Shadow opacity",
            input_value(&self.shadow_opacity_input, cx),
            0.,
            100.,
        )?;
        let mut shadow_color =
            parse_color("Shadow color", input_value(&self.shadow_color_input, cx))?;
        shadow_color[3] = (opacity * 2.55).round() as u8;
        self.style_font_size = font_size;
        self.edit_selected(
            |layer| {
                if let Shape::Text {
                    value: text,
                    font_size: size,
                    style,
                    ..
                } = &mut layer.shape
                {
                    *text = value;
                    *size = font_size;
                    if let Some(shadow) = &mut style.shadow {
                        shadow.blur = blur;
                        shadow.offset = captures_image::Point {
                            x: offset_x,
                            y: offset_y,
                        };
                        shadow.color = shadow_color;
                    }
                    if style.width.is_some() {
                        style.width = Some(width);
                    }
                }
            },
            cx,
        );
        Ok(())
    }

    fn choose_text_font(&mut self, family: &'static str, cx: &mut Context<Self>) {
        use font_kit::{family_name::FamilyName, properties::Properties, source::SystemSource};
        if self
            .selected
            .and_then(|id| self.document.layers.iter().find(|layer| layer.id == id))
            .is_some_and(|layer| layer.locked)
        {
            self.status = "Unlock this layer before editing it".into();
            cx.notify();
            return;
        }
        let requested = match family {
            "serif" => FamilyName::Serif,
            "mono" => FamilyName::Monospace,
            "rounded" => FamilyName::Title("Arial Rounded MT Bold".into()),
            _ => FamilyName::SansSerif,
        };
        let result = (|| -> anyhow::Result<Arc<[u8]>> {
            let font = SystemSource::new()
                .select_best_match(&[requested, FamilyName::SansSerif], &Properties::new())?
                .load()?;
            let bytes = font
                .copy_font_data()
                .ok_or_else(|| anyhow::anyhow!("font data is not available"))?;
            Ok(Arc::from(bytes.as_slice()))
        })();
        match result {
            Ok(bytes) => {
                self.text_font = Some(bytes.clone());
                self.text_font_family = family;
                self.edit_selected(
                    |layer| {
                        if let Shape::Text { font_data, .. } = &mut layer.shape {
                            *font_data = bytes;
                        }
                    },
                    cx,
                );
            }
            Err(error) => {
                self.status = format!("Could not load {family} font: {error}");
                cx.notify();
            }
        }
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.apply_export_fields(cx) {
            self.status = error;
            cx.notify();
            return;
        }
        let image = match self.document.render() {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Export failed: {e}");
                cx.notify();
                return;
            }
        };
        let target_width = if self.export_width > 0 {
            self.export_width
        } else {
            (image.width() * u32::from(self.export_scale) / 100).max(1)
        };
        let target_height = if self.export_height > 0 {
            self.export_height
        } else {
            (image.height() * u32::from(self.export_scale) / 100).max(1)
        };
        let image = if image.width() == target_width && image.height() == target_height {
            image
        } else {
            image::imageops::resize(
                &image,
                target_width,
                target_height,
                image::imageops::FilterType::Lanczos3,
            )
        };
        let bytes = match (self.format, self.export_max_bytes) {
            (Format::Png, Some(limit)) => encode_png_with_limit(&image, limit),
            (Format::Jpeg, Some(limit)) => encode_jpeg_with_limit(&image, limit),
            (Format::Webp, Some(limit)) => encode_webp_with_limit(&image, limit),
            (Format::Png, None) => encode_png(&image, Some(self.quality)),
            (Format::Jpeg, None) => encode_jpeg(&image, self.quality),
            (Format::Webp, None) => encode_webp(&image, Some(self.quality)),
        };
        let bytes = match bytes {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Export failed: {e}");
                cx.notify();
                return;
            }
        };
        // Export is always a new capture. The launch source is never overwritten.
        let mut path = self.destination.clone();
        path.set_extension(self.format.extension());
        if self
            .source_path
            .as_deref()
            .is_some_and(|source| paths_refer_to_same_file(source, &path))
        {
            self.status = "Choose a new file name; the source is never overwritten".into();
            cx.notify();
            return;
        }
        self.status = match atomic_write(&path, &bytes) {
            Ok(()) => format!("Saved {} ({} KB)", path.display(), bytes.len() / 1024),
            Err(e) => format!("Save failed: {e}"),
        };
        cx.notify();
    }

    /// Render on the UI thread (the document contains GPUI-independent image data), then move
    /// resizing and both encodes to the background executor. Request and source revision checks
    /// prevent a slow result from replacing a newer document/settings preview.
    fn start_compression_preview(&mut self, cx: &mut Context<Self>) {
        let Ok(source) = self.document.render() else {
            self.compression_preview_error = Some("Could not render compression preview".into());
            return;
        };
        let spec = ExportSpec {
            format: self.format,
            quality: self.quality,
            max_bytes: self.export_max_bytes,
            width: self.export_width,
            height: self.export_height,
        };
        self.compression_preview_request = self.compression_preview_request.wrapping_add(1);
        let request = self.compression_preview_request;
        let revision = self.document_revision;
        self.compression_preview_pending = true;
        self.compression_preview_error = None;
        let task = cx
            .background_executor()
            .spawn(async move { encode_compression_preview(source, spec) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |editor, cx| {
                if !preview_result_matches(
                    editor.compression_preview_request,
                    request,
                    editor.document_revision,
                    revision,
                ) {
                    return;
                }
                editor.compression_preview_pending = false;
                editor.compression_preview_revision = revision;
                match result {
                    Ok(preview) => {
                        editor.compression_preview = Some(preview);
                        editor.compression_preview_error = None;
                    }
                    Err(error) => editor.compression_preview_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn invalidate_compression_preview(&mut self) {
        self.compression_preview_request = self.compression_preview_request.wrapping_add(1);
        self.compression_preview_pending = false;
        self.compression_preview_revision = u64::MAX;
    }

    fn copy_image(&mut self, cx: &mut Context<Self>) {
        match self
            .document
            .render()
            .and_then(|image| encode_png(&image, None))
        {
            Ok(bytes) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    bytes,
                )));
                self.status = "Copied image".into();
            }
            Err(error) => self.status = format!("Copy failed: {error}"),
        }
        cx.notify();
    }

    fn import_images(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut images = Vec::new();
        for path in paths {
            match decode(&path) {
                Ok(image) => images.push((
                    image,
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )),
                Err(error) => self.status = format!("Could not import {}: {error}", path.display()),
            }
        }
        self.import_decoded_at(images, None, cx);
    }

    fn import_decoded_at(
        &mut self,
        images: Vec<(RgbaImage, String)>,
        point: Option<Point>,
        cx: &mut Context<Self>,
    ) {
        let count = images.len();
        let ids = self.document.add_images(images, self.document.layers.len());
        if let (Some(point), Some(id)) = (point, ids.last().copied())
            && let Some(layer) = self
                .document
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .cloned()
            && let Some(bounds) = layer.geometry_bounds()
        {
            let original = layer.clone();
            self.document.preview_layer(layer.translated(Point {
                x: point.x - bounds.x - bounds.width / 2.,
                y: point.y - bounds.y - bounds.height / 2.,
            }));
            self.document.commit_layer_preview(original);
        }
        self.selected = ids.last().copied();
        if count > 0 {
            self.refresh();
            self.status = format!(
                "Imported {count} image{}",
                if count == 1 { "" } else { "s" }
            );
        }
        cx.notify();
    }

    fn edit_selected(
        &mut self,
        edit: impl FnOnce(&mut captures_windows_native::editor::Layer),
        cx: &mut Context<Self>,
    ) {
        let Some(mut layer) = self
            .selected
            .and_then(|id| self.document.layers.iter().find(|l| l.id == id).cloned())
        else {
            return;
        };
        if layer.locked {
            self.status = "Unlock this layer before editing it".into();
            cx.notify();
            return;
        }
        let original = layer.clone();
        edit(&mut layer);
        self.document.preview_layer(layer);
        self.document.commit_layer_preview(original);
        self.refresh();
        cx.notify();
    }

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        active: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .border_1()
            .border_color(if active {
                self.theme.accent
            } else {
                self.theme.border
            })
            .bg(if active {
                self.theme.hover
            } else {
                self.theme.field
            })
            .text_color(self.theme.text)
            .text_sm()
            .cursor_pointer()
            .child(label.into())
    }

    fn tool_button(
        &self,
        name: &'static str,
        icon: EditorIcon,
        tool: Tool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.tool == tool;
        div()
            .id(name)
            .w(px(38.))
            .h(px(38.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .bg(if active {
                self.theme.accent
            } else {
                rgba(0x00000000)
            })
            .cursor_pointer()
            .child(editor_icon(
                icon,
                if active { "#17140a" } else { "#8b8b94" },
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_tool(tool, cx)))
    }
}

impl Render for ScreenshotEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = Theme::for_window(&self.launch, window, cx);
        if self.export_open
            && !self.compression_preview_pending
            && self.compression_preview_revision != self.document_revision
        {
            self.start_compression_preview(cx);
        }
        let t = self.theme;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let _text_input = self
            .text
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    crate::preferences::input::TextInput::new("Text", "Annotation text", cx)
                })
            })
            .clone();
        let selected_text = self.selected.and_then(|id| {
            self.document
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(|layer| {
                    if let Shape::Text {
                        value,
                        font_size,
                        style,
                        ..
                    } = &layer.shape
                    {
                        Some((id, value.clone(), *font_size, style.clone()))
                    } else {
                        None
                    }
                })
        });
        if let Some((id, value, font_size, style)) = &selected_text
            && self.synced_text_layer != Some((*id, self.document_revision))
        {
            replace_input(&mut self.text, value.clone(), cx);
            replace_input(&mut self.text_size_input, font_size.to_string(), cx);
            replace_input(
                &mut self.text_wrap_input,
                style.width.unwrap_or(240.).round().to_string(),
                cx,
            );
            let shadow = style.shadow.clone().unwrap_or(captures_image::TextShadow {
                color: [0, 0, 0, 115],
                blur: 6.,
                offset: captures_image::Point { x: 0., y: 3. },
            });
            replace_input(&mut self.shadow_blur_input, shadow.blur.to_string(), cx);
            replace_input(&mut self.shadow_x_input, shadow.offset.x.to_string(), cx);
            replace_input(&mut self.shadow_y_input, shadow.offset.y.to_string(), cx);
            replace_input(&mut self.shadow_color_input, color_hex(shadow.color), cx);
            replace_input(
                &mut self.shadow_opacity_input,
                ((f32::from(shadow.color[3]) / 255.) * 100.)
                    .round()
                    .to_string(),
                cx,
            );
            self.style_font_size = *font_size;
            self.synced_text_layer = Some((*id, self.document_revision));
        } else if selected_text.is_none() {
            self.synced_text_layer = None;
        }
        let text_input = self.text.as_ref().unwrap().clone();
        let export_width_input =
            ensure_input(&mut self.export_width_input, self.export_width, "Width", cx);
        let export_height_input = ensure_input(
            &mut self.export_height_input,
            self.export_height,
            "Height",
            cx,
        );
        let quality_input = ensure_input(&mut self.quality_input, self.quality, "1–100", cx);
        let max_size_input = ensure_input(&mut self.max_size_input, "", "Optional bytes (1MB)", cx);
        let canvas_width_input = ensure_input(
            &mut self.canvas_width_input,
            self.document.canvas_width,
            "Width",
            cx,
        );
        let canvas_height_input = ensure_input(
            &mut self.canvas_height_input,
            self.document.canvas_height,
            "Height",
            cx,
        );
        let stroke_input =
            ensure_input(&mut self.stroke_input, self.style_stroke, "Line width", cx);
        let stroke_color_input = ensure_input(
            &mut self.stroke_color_input,
            color_hex(self.style_color),
            "#RRGGBBAA",
            cx,
        );
        let fill_color_input =
            ensure_input(&mut self.fill_color_input, "", "none or #RRGGBBAA", cx);
        let text_size_input =
            ensure_input(&mut self.text_size_input, self.style_font_size, "8–240", cx);
        let text_wrap_input = ensure_input(&mut self.text_wrap_input, 240, "20–4000", cx);
        let shadow_blur_input = ensure_input(&mut self.shadow_blur_input, 6, "0–200", cx);
        let shadow_x_input = ensure_input(&mut self.shadow_x_input, 0, "−500–500", cx);
        let shadow_y_input = ensure_input(&mut self.shadow_y_input, 3, "−500–500", cx);
        let shadow_color_input =
            ensure_input(&mut self.shadow_color_input, "#000000", "#RRGGBB", cx);
        let shadow_opacity_input = ensure_input(&mut self.shadow_opacity_input, 45, "0–100", cx);
        let rendered = self.rendered.clone();
        let recorded_bounds = self.canvas_bounds.clone();
        let paint_bounds = recorded_bounds.clone();
        let image_size = size(
            self.document.canvas_width as f32,
            self.document.canvas_height as f32,
        );
        let fit = self.fit;
        let zoom = self.zoom;
        let pan = self.pan;
        let crop = self.document.crop;
        let selected_handles = self
            .selected
            .and_then(|id| {
                self.document
                    .layers
                    .iter()
                    .find(|layer| layer.id == id)
                    .map(|layer| layer.resize_handles())
            })
            .unwrap_or_default();
        let rotation_handle_point = self.selected.and_then(|id| {
            self.document
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(rotation_handle)
        });
        let tools = [
            ("Select", EditorIcon::Select, Tool::Select),
            ("Crop", EditorIcon::Crop, Tool::Crop),
            ("Text", EditorIcon::Text, Tool::Text),
            ("Pen", EditorIcon::Pen, Tool::Pen),
            ("Arrow", EditorIcon::Arrow, Tool::Arrow),
            ("Rectangle", EditorIcon::Rectangle, Tool::Rectangle),
            ("Ellipse", EditorIcon::Ellipse, Tool::Ellipse),
            ("Line", EditorIcon::Line, Tool::Line),
            ("Triangle", EditorIcon::Triangle, Tool::Triangle),
            ("Diamond", EditorIcon::Diamond, Tool::Diamond),
            ("Star", EditorIcon::Star, Tool::Star),
            ("Eraser", EditorIcon::Eraser, Tool::Eraser),
        ];
        let layers = self
            .document
            .layers
            .iter()
            .rev()
            .map(|layer| {
                let id = layer.id;
                let active = self.selected == Some(id);
                div()
                    .id(("layer", id as usize))
                    .h(px(42.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(if active { t.hover } else { t.raised })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id);
                        cx.notify()
                    }))
                    .child(if layer.visible { "◉" } else { "○" })
                    .child(div().flex_1().overflow_hidden().child(layer.name.clone()))
                    .child(if layer.locked { "⌑" } else { "" })
            })
            .collect::<Vec<_>>();
        let selected = self.selected;
        let compression_before = self.compression_preview.as_ref().map(|p| p.before.clone());
        let compression_after = self.compression_preview.as_ref().map(|p| p.after.clone());
        let compression_label = self.compression_preview.as_ref().map(|p| {
            format!(
                "Original {}  •  Export {}  •  {}%",
                human_bytes(p.before_bytes),
                human_bytes(p.after_bytes),
                p.after_bytes.saturating_mul(100) / p.before_bytes.max(1)
            )
        });
        div()
            .track_focus(&focus)
            .key_context("ScreenshotEditor")
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .font_family(font())
            .text_size(px(13.))
            .size_full()
            .min_w(px(900.))
            .bg(t.canvas)
            .text_color(t.text)
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .child(
                        div()
                            .h(px(34.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(9.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.canvas)
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("Canvas")
                            .child("W")
                            .child(
                                div()
                                    .text_color(t.text)
                                    .child(self.document.canvas_width.to_string()),
                            )
                            .child("× H")
                            .child(
                                div()
                                    .text_color(t.text)
                                    .child(self.document.canvas_height.to_string()),
                            )
                            .child(div().w(px(1.)).h(px(16.)).mx_1().bg(t.border))
                            .child("Trim edges"),
                    )
                    .child(div().flex_1())
                    .child(
                        self.button("import", "▧  Add images", false)
                            .on_click(cx.listener(|_this, _, window, cx| {
                                let receiver = cx.prompt_for_paths(PathPromptOptions {
                                    files: true,
                                    directories: false,
                                    multiple: true,
                                    prompt: Some("Import images".into()),
                                });
                                cx.spawn_in(window, async move |this, cx| {
                                    if let Ok(Ok(Some(paths))) = receiver.await {
                                        let _ = this.update(cx, |editor, cx| {
                                            editor.import_images(paths, cx)
                                        });
                                    }
                                })
                                .detach();
                            })),
                    )
                    .child(
                        self.button("undo", "↶", false)
                            .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                    )
                    .child(
                        self.button("redo", "↷", false)
                            .on_click(cx.listener(|this, _, _, cx| this.redo(cx))),
                    )
                    .child(self.button("zoom-out", "−", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.fit = false;
                            this.zoom = this.zoom.saturating_sub(10).max(10);
                            cx.notify()
                        },
                    )))
                    .child(div().w(px(58.)).text_center().text_sm().child(if self.fit {
                        "Fit".into()
                    } else {
                        format!("{}%", self.zoom)
                    }))
                    .child(self.button("zoom-in", "+", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.fit = false;
                            this.zoom = (this.zoom + 10).min(400);
                            cx.notify()
                        },
                    )))
                    .child(self.button("pan-left", "←", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.fit = false;
                            s.pan.x += px(32.);
                            cx.notify()
                        },
                    )))
                    .child(self.button("pan-up", "↑", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.fit = false;
                            s.pan.y += px(32.);
                            cx.notify()
                        },
                    )))
                    .child(self.button("pan-down", "↓", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.fit = false;
                            s.pan.y -= px(32.);
                            cx.notify()
                        },
                    )))
                    .child(self.button("pan-right", "→", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.fit = false;
                            s.pan.x -= px(32.);
                            cx.notify()
                        },
                    )))
                    .child(
                        self.button("recenter", "Fit", self.fit)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.pan = point(px(0.), px(0.));
                                s.fit = true;
                                cx.notify()
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(56.))
                            .py_2()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .border_r_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .children(
                                tools
                                    .into_iter()
                                    .map(|(n, g, v)| self.tool_button(n, g, v, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.canvas)
                            .child(
                                div()
                                    .id("canvas")
                                    .size_full()
                                    .cursor_crosshair()
                                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                                    .on_mouse_down(
                                        MouseButton::Middle,
                                        cx.listener(Self::mouse_down),
                                    )
                                    .on_mouse_move(cx.listener(Self::mouse_move))
                                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
                                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
                                    .on_mouse_up_out(
                                        MouseButton::Middle,
                                        cx.listener(Self::mouse_up),
                                    )
                                    .can_drop(|value, _, _| value.is::<ExternalPaths>())
                                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                        let paths = paths.paths().to_vec();
                                        let mut images = Vec::new();
                                        for path in paths {
                                            if let Ok(image) = decode(&path) {
                                                images.push((
                                                    image,
                                                    path.file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .into_owned(),
                                                ));
                                            }
                                        }
                                        let point = this.last_canvas_point.unwrap_or(Point {
                                            x: this.document.crop.x
                                                + this.document.canvas_width as f32 / 2.,
                                            y: this.document.crop.y
                                                + this.document.canvas_height as f32 / 2.,
                                        });
                                        this.import_decoded_at(images, Some(point), cx);
                                    }))
                                    .child(
                                        canvas(
                                            move |bounds, _, _| {
                                                let available_w =
                                                    (bounds.size.width / px(1.)).max(1.);
                                                let available_h =
                                                    (bounds.size.height / px(1.)).max(1.);
                                                let scale = if fit {
                                                    (available_w / image_size.width)
                                                        .min(available_h / image_size.height)
                                                } else {
                                                    zoom as f32 / 100.
                                                };
                                                let size = size(
                                                    px(image_size.width * scale),
                                                    px(image_size.height * scale),
                                                );
                                                recorded_bounds.set(Bounds {
                                                    origin: point(
                                                        bounds.origin.x
                                                            + (bounds.size.width - size.width) / 2.
                                                            + pan.x,
                                                        bounds.origin.y
                                                            + (bounds.size.height - size.height)
                                                                / 2.
                                                            + pan.y,
                                                    ),
                                                    size,
                                                });
                                            },
                                            move |_, _, window, _| {
                                                let _ = window.paint_image(
                                                    paint_bounds.get(),
                                                    Corners::default(),
                                                    rendered.clone(),
                                                    0,
                                                    false,
                                                );
                                                let image = paint_bounds.get();
                                                for (_, handle) in &selected_handles {
                                                    let center = point(
                                                        image.origin.x
                                                            + px((handle.x - crop.x)
                                                                / image_size.width
                                                                * (image.size.width / px(1.))),
                                                        image.origin.y
                                                            + px((handle.y - crop.y)
                                                                / image_size.height
                                                                * (image.size.height / px(1.))),
                                                    );
                                                    window.paint_quad(quad(
                                                        Bounds {
                                                            origin: point(
                                                                center.x - px(4.5),
                                                                center.y - px(4.5),
                                                            ),
                                                            size: size(px(9.), px(9.)),
                                                        },
                                                        Corners::all(px(3.)),
                                                        rgb(0xffffff),
                                                        Edges::all(px(2.)),
                                                        rgb(0xffca28),
                                                        BorderStyle::Solid,
                                                    ));
                                                }
                                                if let Some(handle) = rotation_handle_point {
                                                    let center = point(
                                                        image.origin.x
                                                            + px((handle.x - crop.x)
                                                                / image_size.width
                                                                * (image.size.width / px(1.))),
                                                        image.origin.y
                                                            + px((handle.y - crop.y)
                                                                / image_size.height
                                                                * (image.size.height / px(1.))),
                                                    );
                                                    window.paint_quad(quad(
                                                        Bounds {
                                                            origin: point(
                                                                center.x - px(5.),
                                                                center.y - px(5.),
                                                            ),
                                                            size: size(px(10.), px(10.)),
                                                        },
                                                        Corners::all(px(5.)),
                                                        rgb(0xffca28),
                                                        Edges::all(px(2.)),
                                                        rgb(0xffffff),
                                                        BorderStyle::Solid,
                                                    ));
                                                }
                                            },
                                        )
                                        .size_full(),
                                    )
                                    .when(self.export_open, |surface| {
                                        surface.child(
                                            div()
                                                .absolute()
                                                .left_3()
                                                .right_3()
                                                .bottom_3()
                                                .h(px(190.))
                                                .p_2()
                                                .rounded(px(10.))
                                                .border_1()
                                                .border_color(t.border)
                                                .bg(t.raised)
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(
                                                    div().flex().items_center().gap_2()
                                                        .child("Compression preview")
                                                        .child(div().flex_1())
                                                        .child(
                                                            compression_label.clone().unwrap_or_else(|| {
                                                                if self.compression_preview_pending {
                                                                    "Encoding…".into()
                                                                } else {
                                                                    self.compression_preview_error.clone().unwrap_or_else(|| "Waiting…".into())
                                                                }
                                                            })
                                                        ),
                                                )
                                                .child(
                                                    div().flex_1().min_h_0().flex().gap_1()
                                                        .when_some(compression_before.clone(), |row, image| {
                                                            row.child(compression_image(image, self.compression_split, true, t))
                                                        })
                                                        .when_some(compression_after.clone(), |row, image| {
                                                            row.child(compression_image(image, self.compression_split, false, t))
                                                        }),
                                                )
                                                .child(
                                                    div().flex().items_center().gap_2()
                                                        .child("Before")
                                                        .child(self.button("compare-less", "◀", false).on_click(cx.listener(|s, _, _, cx| {
                                                            s.compression_split = s.compression_split.saturating_sub(5).max(10);
                                                            cx.notify();
                                                        })))
                                                        .child(format!("{}%", self.compression_split))
                                                        .child(self.button("compare-more", "▶", false).on_click(cx.listener(|s, _, _, cx| {
                                                            s.compression_split = s.compression_split.saturating_add(5).min(90);
                                                            cx.notify();
                                                        })))
                                                        .child("After"),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("screenshot-inspector")
                            .w(px(320.))
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .when(self.tool != Tool::Select || selected.is_some(), |sidebar| {
                                sidebar.child(
                                    section("Inspector", t)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(field("LINE WIDTH", stroke_input, t))
                                                .child(field(
                                                    "STROKE · HEX OR RGBA",
                                                    stroke_color_input,
                                                    t,
                                                ))
                                                .child(field(
                                                    "FILL · NONE, HEX OR RGBA",
                                                    fill_color_input,
                                                    t,
                                                ))
                                                .child(
                                                    self.button("apply-style", "Apply style", true)
                                                        .on_click(cx.listener(|s, _, _, cx| {
                                                            s.status = match s
                                                                .apply_style_fields(cx)
                                                            {
                                                                Ok(()) => "Style applied".into(),
                                                                Err(error) => error,
                                                            };
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .when(self.tool == Tool::Text || selected_text.is_some(), |panel| {
                                            let style = selected_text
                                                .as_ref()
                                                .map(|(_, _, _, style)| style.clone())
                                                .unwrap_or_default();
                                            panel
                                            .child(text_input)
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .gap_2()
                                                    .child(field("SIZE", text_size_input, t))
                                                    .child(self.button("font-system", "System", self.text_font_family == "system").on_click(cx.listener(|s, _, _, cx| s.choose_text_font("system", cx))))
                                                    .child(self.button("font-serif", "Serif", self.text_font_family == "serif").on_click(cx.listener(|s, _, _, cx| s.choose_text_font("serif", cx))))
                                                    .child(self.button("font-mono", "Mono", self.text_font_family == "mono").on_click(cx.listener(|s, _, _, cx| s.choose_text_font("mono", cx))))
                                                    .child(self.button("font-rounded", "Rounded", self.text_font_family == "rounded").on_click(cx.listener(|s, _, _, cx| s.choose_text_font("rounded", cx)))),
                                            )
                                            .child(
                                                div().flex().flex_wrap().gap_2()
                                                    .child(self.button("text-bold", "Bold", style.bold).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.bold = !style.bold }, cx))))
                                                    .child(self.button("text-italic", "Italic", style.italic).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.italic = !style.italic }, cx))))
                                                    .child(self.button("text-outline", "Outline", style.outlined).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.outlined = !style.outlined }, cx)))),
                                            )
                                            .child(
                                                div().flex().flex_wrap().gap_2()
                                                    .child(self.button("text-left", "Left", style.align == captures_image::TextAlign::Left).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.align = captures_image::TextAlign::Left }, cx))))
                                                    .child(self.button("text-center", "Center", style.align == captures_image::TextAlign::Center).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.align = captures_image::TextAlign::Center }, cx))))
                                                    .child(self.button("text-right", "Right", style.align == captures_image::TextAlign::Right).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.align = captures_image::TextAlign::Right }, cx))))
                                                    .child(self.button("text-auto", "Auto width", style.width.is_none()).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.width = None }, cx))))
                                                    .child(self.button("text-fixed", "Fixed width", style.width.is_some()).on_click(cx.listener(|s, _, _, cx| {
                                                        let width = input_value(&s.text_wrap_input, cx).trim().parse::<f32>().unwrap_or(240.).clamp(20., 4000.);
                                                        s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.width = Some(width) }, cx)
                                                    })))
                                                    .child(field("WRAP WIDTH", text_wrap_input, t)),
                                            )
                                            .child(
                                                div().flex().flex_wrap().gap_2()
                                                    .child(self.button("text-plate", "Plate", style.background.is_some() && !style.rounded_background).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.background = if style.background.is_some() && !style.rounded_background { None } else { Some([17, 19, 24, 230]) }; style.rounded_background = false }, cx))))
                                                    .child(self.button("text-rounded", "Rounded", style.background.is_some() && style.rounded_background).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { let active = style.background.is_some() && style.rounded_background; style.background = if active { None } else { Some([17, 19, 24, 230]) }; style.rounded_background = !active }, cx))))
                                                    .child(self.button("text-shadow", "Shadow", style.shadow.is_some()).on_click(cx.listener(|s, _, _, cx| s.edit_selected(|layer| if let Shape::Text { style, .. } = &mut layer.shape { style.shadow = if style.shadow.is_some() { None } else { Some(captures_image::TextShadow { color: [0, 0, 0, 115], blur: 6., offset: captures_image::Point { x: 0., y: 3. } }) } }, cx)))),
                                            )
                                            .when(style.shadow.is_some(), |panel| panel.child(
                                                div().flex().flex_wrap().gap_2()
                                                    .child(field("BLUR", shadow_blur_input, t))
                                                    .child(field("OFFSET X", shadow_x_input, t))
                                                    .child(field("OFFSET Y", shadow_y_input, t))
                                                    .child(field("COLOR", shadow_color_input, t))
                                                    .child(field("OPACITY %", shadow_opacity_input, t)),
                                            ))
                                            .child(self.button("apply-text", "Apply text", true).on_click(cx.listener(|s, _, _, cx| {
                                                s.status = match s.apply_text_fields(cx) { Ok(()) => "Text updated".into(), Err(error) => error };
                                                cx.notify();
                                            })))
                                            .child(
                                                div().text_size(px(12.)).text_color(t.muted).child(
                                                    if selected_text.is_some() { "Editing selected text layer." } else { "Enter text, then click the image to place it." },
                                                ),
                                            )
                                        })
                                        .child(div().text_sm().text_color(t.muted).child(
                                            match self.selected {
                                                Some(id) => format!("Layer {id} · opacity 100%"),
                                                None => "Select a layer to edit its style".into(),
                                            },
                                        ))
                                        .when_some(selected, |panel, id| {
                                            panel.child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .gap_1()
                                                    .child(
                                                        self.button("visible", "Visible", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    s.document
                                                                        .toggle_visibility(id);
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("lock", "Lock", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    s.document.toggle_locked(id);
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button(
                                                            "duplicate",
                                                            "Duplicate",
                                                            false,
                                                        )
                                                        .on_click(cx.listener(
                                                            move |s, _, _, cx| {
                                                                s.selected =
                                                                    s.document.duplicate(id);
                                                                s.refresh();
                                                                cx.notify();
                                                            },
                                                        )),
                                                    )
                                                    .child(
                                                        self.button("raise", "Raise", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    s.document.move_layer(id, 1);
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("lower", "Lower", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    s.document.move_layer(id, -1);
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("rotate", "Rotate 15°", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    let angle = s
                                                                        .document
                                                                        .layers
                                                                        .iter()
                                                                        .find(|l| l.id == id)
                                                                        .map_or(15., |l| {
                                                                            l.rotation_degrees + 15.
                                                                        });
                                                                    s.document.set_layer_rotation(
                                                                        id, angle,
                                                                    );
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("opacity", "Opacity −", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    let opacity = s
                                                                        .document
                                                                        .layers
                                                                        .iter()
                                                                        .find(|l| l.id == id)
                                                                        .map_or(255, |l| {
                                                                            l.opacity
                                                                                .saturating_sub(26)
                                                                        });
                                                                    s.document.set_layer_opacity(
                                                                        id, opacity,
                                                                    );
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("blend", "Multiply", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    s.document
                                                                        .set_layer_blend_mode(
                                                                            id,
                                                                            BlendMode::Multiply,
                                                                        );
                                                                    s.refresh();
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("delete", "Delete", false)
                                                            .on_click(cx.listener(
                                                                move |s, _, _, cx| {
                                                                    if s.document.delete(id) {
                                                                        s.selected = None;
                                                                        s.refresh();
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    ),
                                            )
                                        }),
                                )
                            })
                            .child(
                                section("Layers", t)
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h(px(42.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(if self.document.source_visible {
                                                "◉"
                                            } else {
                                                "○"
                                            })
                                            .child(self.document.source_name.clone()),
                                    )
                                    .children(layers),
                            )
                            .child(
                                section("Document", t).child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(field("WIDTH", canvas_width_input, t).w(px(118.)))
                                        .child(field("HEIGHT", canvas_height_input, t).w(px(118.)))
                                        .child(
                                            self.button("canvas-apply", "Resize canvas", true)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.status = match this.apply_canvas_fields(cx)
                                                    {
                                                        Ok(()) => "Canvas resized".into(),
                                                        Err(error) => error,
                                                    };
                                                    cx.notify();
                                                })),
                                        )
                                        .child(self.button("flatten", "Flatten", false).on_click(
                                            cx.listener(|this, _, _, cx| {
                                                match this.document.flatten_layers() {
                                                    Ok(_) => this.refresh(),
                                                    Err(e) => this.status = e,
                                                }
                                                cx.notify()
                                            }),
                                        ))
                                        .child(self.button("trim", "Trim", false).on_click(
                                            cx.listener(|this, _, _, cx| {
                                                match this.document.trim_to_visible_content() {
                                                    Ok(_) => this.refresh(),
                                                    Err(e) => this.status = e.into(),
                                                }
                                                cx.notify()
                                            }),
                                        )),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .min_h(px(72.))
                    .py_3()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .when(self.export_open, |bar| {
                        bar.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(field("WIDTH", export_width_input, t).w(px(112.)))
                                .child(field("HEIGHT", export_height_input, t).w(px(112.)))
                                .child(field("QUALITY", quality_input, t).w(px(92.)))
                                .child(field("MAX SIZE", max_size_input, t).w(px(140.)))
                                .child(
                                    self.button(
                                        "aspect-lock",
                                        "Lock aspect",
                                        self.export_aspect_locked,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            s.export_aspect_locked = !s.export_aspect_locked;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(self.button("apply-export", "Apply", true).on_click(
                                    cx.listener(|s, _, _, cx| {
                                        s.status = match s.apply_export_fields(cx) {
                                            Ok(()) => {
                                                s.invalidate_compression_preview();
                                                "Export settings applied".into()
                                            }
                                            Err(error) => error,
                                        };
                                        cx.notify();
                                    }),
                                )),
                        )
                    })
                    .child(
                        self.button("export-options", "Export settings", self.export_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_open = !this.export_open;
                                this.invalidate_compression_preview();
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("copy-image", "Copy image", false)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_image(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.subtle)
                                    .child("FILE NAME & DESTINATION"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .child(self.destination.display().to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted)
                                    .child(self.status.clone()),
                            ),
                    )
                    .child(
                        self.button("destination", "Change…", false)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let parent = this.destination.parent().unwrap_or(Path::new("."));
                                let name = format!("Capture-edited.{}", this.format.extension());
                                let receiver = cx.prompt_for_new_path(parent, Some(&name));
                                cx.spawn_in(window, async move |this, cx| {
                                    if let Ok(Ok(Some(path))) = receiver.await {
                                        let _ = this.update(cx, |editor, cx| {
                                            editor.destination = path;
                                            editor.invalidate_compression_preview();
                                            editor.status = "Destination updated".into();
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            })),
                    )
                    .child(
                        self.button("png", "PNG", self.format == Format::Png)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Png;
                                this.invalidate_compression_preview();
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("jpeg", "JPEG", self.format == Format::Jpeg)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Jpeg;
                                this.invalidate_compression_preview();
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("webp", "WebP", self.format == Format::Webp)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Webp;
                                this.invalidate_compression_preview();
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("draft", "Save draft", false)
                            .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                    )
                    .child(
                        self.button("save", "Save copy", true)
                            .bg(t.accent)
                            .text_color(rgb(0x17140a))
                            .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                    ),
            )
    }
}

fn ensure_input(
    slot: &mut Option<Entity<crate::preferences::input::TextInput>>,
    value: impl ToString,
    placeholder: &'static str,
    cx: &mut Context<ScreenshotEditor>,
) -> Entity<crate::preferences::input::TextInput> {
    slot.get_or_insert_with(|| {
        let value = value.to_string();
        cx.new(|cx| crate::preferences::input::TextInput::new(value, placeholder, cx))
    })
    .clone()
}

fn replace_input(
    slot: &mut Option<Entity<crate::preferences::input::TextInput>>,
    value: String,
    cx: &mut Context<ScreenshotEditor>,
) {
    if let Some(input) = slot {
        input.update(cx, |input, cx| {
            *input = crate::preferences::input::TextInput::new(value, "", cx);
        });
    } else {
        *slot = Some(cx.new(|cx| crate::preferences::input::TextInput::new(value, "", cx)));
    }
}

fn input_value(input: &Option<Entity<crate::preferences::input::TextInput>>, cx: &App) -> String {
    input
        .as_ref()
        .map(|input| input.read(cx).value())
        .unwrap_or_default()
}

fn field(
    label: &'static str,
    input: Entity<crate::preferences::input::TextInput>,
    theme: Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(72.))
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme.subtle)
                .child(label),
        )
        .child(input)
}

fn parse_dimension(label: &str, value: String) -> Result<u32, String> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("{label} must be a whole number between 1 and 16384"))?;
    if !(1..=16_384).contains(&parsed) {
        return Err(format!("{label} must be between 1 and 16384"));
    }
    Ok(parsed)
}

fn parse_bounded_u8(label: &str, value: String, min: u8, max: u8) -> Result<u8, String> {
    let parsed = value
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("{label} must be a whole number from {min} to {max}"))?;
    if !(min..=max).contains(&parsed) {
        return Err(format!("{label} must be from {min} to {max}"));
    }
    Ok(parsed)
}

fn parse_f32(label: &str, value: String, min: f32, max: f32) -> Result<f32, String> {
    let parsed = value
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("{label} must be a number from {min} to {max}"))?;
    if !parsed.is_finite() || parsed < min || parsed > max {
        return Err(format!("{label} must be from {min} to {max}"));
    }
    Ok(parsed)
}

fn parse_max_size(value: String) -> Result<Option<u64>, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value == "none" {
        return Ok(None);
    }
    let (number, multiplier) = if let Some(number) = value.strip_suffix("mb") {
        (number, 1_000_000)
    } else if let Some(number) = value.strip_suffix("kb") {
        (number, 1_000)
    } else {
        (value.as_str(), 1)
    };
    let number = number
        .trim()
        .parse::<u64>()
        .map_err(|_| "Max size must be bytes, KB, or MB (for example 750KB)".to_string())?;
    let bytes = number
        .checked_mul(multiplier)
        .ok_or_else(|| "Max size is too large".to_string())?;
    if bytes == 0 {
        return Err("Max size must be greater than zero, or blank for no limit".into());
    }
    Ok(Some(bytes))
}

fn proportional_height(width: u32, source_width: u32, source_height: u32) -> Result<u32, String> {
    let height = u64::from(width) * u64::from(source_height) / u64::from(source_width.max(1));
    u32::try_from(height.max(1))
        .ok()
        .filter(|height| *height <= 16_384)
        .ok_or_else(|| {
            "Locked export height exceeds 16384; reduce the width or unlock aspect ratio".into()
        })
}

fn parse_color(label: &str, value: String) -> Result<[u8; 4], String> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#')
        && (hex.len() == 6 || hex.len() == 8)
    {
        let mut color = [0, 0, 0, 255];
        for (index, component) in color.iter_mut().enumerate().take(hex.len() / 2) {
            *component = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("{label} color contains invalid hex digits"))?;
        }
        return Ok(color);
    }
    let components = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>();
    if let Ok(components) = components
        && (components.len() == 3 || components.len() == 4)
    {
        return Ok([
            components[0],
            components[1],
            components[2],
            components.get(3).copied().unwrap_or(255),
        ]);
    }
    Err(format!(
        "{label} color must be #RRGGBB, #RRGGBBAA, or r,g,b,a"
    ))
}

fn color_hex(color: [u8; 4]) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        color[0], color[1], color[2], color[3]
    )
}

fn section(title: &'static str, t: Theme) -> Div {
    div()
        .p_4()
        .gap_3()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(title),
        )
}
fn render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = image.clone();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(bgra)]))
}

fn normalized_rect(start: Point, end: Point, square: bool) -> Rect {
    let mut dx = end.x - start.x;
    let mut dy = end.y - start.y;
    if square {
        let side = dx.abs().max(dy.abs());
        dx = side.copysign(dx);
        dy = side.copysign(dy);
    }
    Rect {
        x: start.x.min(start.x + dx),
        y: start.y.min(start.y + dy),
        width: dx.abs().max(1.),
        height: dy.abs().max(1.),
    }
}

fn gesture_shape(
    tool: Tool,
    start: Point,
    end: Point,
    points: Vec<Point>,
    shift: bool,
) -> Option<Shape> {
    let constrained = if shift {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length = dx.abs().max(dy.abs());
        Point {
            x: start.x + length.copysign(dx),
            y: start.y + length.copysign(dy),
        }
    } else {
        end
    };
    match tool {
        Tool::Pen => (points.len() > 1).then_some(Shape::Stroke(points)),
        Tool::Arrow => Some(Shape::Arrow(start, constrained)),
        Tool::Line => Some(Shape::Line(start, constrained)),
        Tool::Rectangle => Some(Shape::Rectangle(normalized_rect(start, end, shift))),
        Tool::Ellipse => Some(Shape::Ellipse(normalized_rect(start, end, shift))),
        Tool::Triangle => Some(Shape::Polygon(polygon(
            normalized_rect(start, end, shift),
            3,
        ))),
        Tool::Diamond => {
            let r = normalized_rect(start, end, shift);
            Some(Shape::Polygon(vec![
                Point {
                    x: r.x + r.width / 2.,
                    y: r.y,
                },
                Point {
                    x: r.x + r.width,
                    y: r.y + r.height / 2.,
                },
                Point {
                    x: r.x + r.width / 2.,
                    y: r.y + r.height,
                },
                Point {
                    x: r.x,
                    y: r.y + r.height / 2.,
                },
            ]))
        }
        Tool::Star => {
            let r = normalized_rect(start, end, shift);
            let center = Point {
                x: r.x + r.width / 2.,
                y: r.y + r.height / 2.,
            };
            Some(Shape::Polygon(
                (0..10)
                    .map(|index| {
                        let angle =
                            -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.;
                        let radius = if index % 2 == 0 { 1. } else { 0.39 };
                        Point {
                            x: center.x + angle.cos() * r.width / 2. * radius,
                            y: center.y + angle.sin() * r.height / 2. * radius,
                        }
                    })
                    .collect(),
            ))
        }
        _ => None,
    }
}

fn polygon(rect: Rect, sides: usize) -> Vec<Point> {
    let center = Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    };
    (0..sides)
        .map(|index| {
            let angle =
                -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::TAU / sides as f32;
            Point {
                x: center.x + angle.cos() * rect.width / 2.,
                y: center.y + angle.sin() * rect.height / 2.,
            }
        })
        .collect()
}

fn rotation_handle(layer: &captures_windows_native::editor::Layer) -> Option<Point> {
    let corners = layer.selection_corners()?;
    let top = Point {
        x: (corners[0].x + corners[1].x) / 2.,
        y: (corners[0].y + corners[1].y) / 2.,
    };
    let bounds = layer.geometry_bounds()?;
    let center = Point {
        x: bounds.x + bounds.width / 2.,
        y: bounds.y + bounds.height / 2.,
    };
    let dx = top.x - center.x;
    let dy = top.y - center.y;
    let length = dx.hypot(dy).max(1.);
    Some(Point {
        x: top.x + dx / length * 28.,
        y: top.y + dy / length * 28.,
    })
}

fn encode_compression_preview(
    source: RgbaImage,
    spec: ExportSpec,
) -> Result<CompressionPreview, String> {
    let before_bytes = encode_png(&source, None)?.len();
    let output = if source.dimensions() == (spec.width, spec.height) {
        source.clone()
    } else {
        image::imageops::resize(
            &source,
            spec.width,
            spec.height,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let encoded = match (spec.format, spec.max_bytes) {
        (Format::Png, Some(limit)) => encode_png_with_limit(&output, limit),
        (Format::Jpeg, Some(limit)) => encode_jpeg_with_limit(&output, limit),
        (Format::Webp, Some(limit)) => encode_webp_with_limit(&output, limit),
        (Format::Png, None) => encode_png(&output, Some(spec.quality)),
        (Format::Jpeg, None) => encode_jpeg(&output, spec.quality),
        (Format::Webp, None) => encode_webp(&output, Some(spec.quality)),
    }?;
    let after = image::load_from_memory(&encoded)
        .map_err(|error| format!("Could not decode export preview: {error}"))?
        .to_rgba8();
    Ok(CompressionPreview {
        before: render_image(&source),
        after: render_image(&after),
        before_bytes,
        after_bytes: encoded.len(),
    })
}

fn preview_result_matches(
    current_request: u64,
    request: u64,
    current_revision: u64,
    revision: u64,
) -> bool {
    current_request == request && current_revision == revision
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.)
    } else {
        format!("{:.1} KB", bytes as f64 / 1_000.)
    }
}

fn compression_image(image: Arc<RenderImage>, split: u8, before: bool, theme: Theme) -> Div {
    let width = if before { split } else { 100 - split };
    div()
        .h_full()
        .w(relative(width as f32 / 100.))
        .overflow_hidden()
        .rounded(px(5.))
        .border_1()
        .border_color(theme.border)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let _ = window.paint_image(bounds, Corners::default(), image.clone(), 0, false);
                },
            )
            .size_full(),
        )
}

fn decode(path: &Path) -> anyhow::Result<RgbaImage> {
    Ok(image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .to_rgba8())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing parent"))?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}
fn mock_artwork() -> RgbaImage {
    let mut out = RgbaImage::new(960, 600);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let band = ((x / 120 + y / 90) % 2) as u8;
        *p = image::Rgba([225 - band * 18, 228 - band * 10, 234, 255]);
    }
    for y in 110..490 {
        for x in 150..810 {
            if (x as i32 - 480).pow(2) / 4 + (y as i32 - 300).pow(2) < 26000 {
                out.put_pixel(x, y, image::Rgba([61, 73, 92, 255]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn edit_undo_crop_and_encode_produce_real_bytes() {
        let mut d = Document::new(mock_artwork());
        let original = d.render().unwrap();
        let id = d.add(
            Shape::Rectangle(Rect {
                x: 20.,
                y: 30.,
                width: 90.,
                height: 70.,
            }),
            ACCENT,
            5.,
        );
        assert_eq!(d.layers.last().unwrap().id, id);
        let edited = encode_png(&d.render().unwrap(), Some(92)).unwrap();
        assert!(edited.starts_with(b"\x89PNG") && edited.len() > 100);
        assert!(d.undo());
        assert_eq!(d.render().unwrap(), original);
        assert!(d.set_crop(Rect {
            x: 10.,
            y: 10.,
            width: 320.,
            height: 200.
        }));
        assert_eq!(d.render().unwrap().dimensions(), (320, 200));
    }
    #[test]
    fn export_helper_never_mutates_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        let bytes = b"immutable source";
        fs::write(&source, bytes).unwrap();
        let output = dir.path().join("captures/copy.png");
        atomic_write(&output, b"new bytes").unwrap();
        assert_eq!(fs::read(source).unwrap(), bytes);
        assert_eq!(fs::read(output).unwrap(), b"new bytes");
    }

    #[test]
    fn pointer_gesture_uses_asymmetric_coordinates_and_shift_constraint() {
        assert_eq!(
            gesture_shape(
                Tool::Rectangle,
                Point { x: 91., y: 17. },
                Point { x: 13., y: 53. },
                vec![],
                false,
            ),
            Some(Shape::Rectangle(Rect {
                x: 13.,
                y: 17.,
                width: 78.,
                height: 36.
            }))
        );
        assert_eq!(
            gesture_shape(
                Tool::Arrow,
                Point { x: 7., y: 11. },
                Point { x: 31., y: 20. },
                vec![],
                true,
            ),
            Some(Shape::Arrow(
                Point { x: 7., y: 11. },
                Point { x: 31., y: 35. }
            ))
        );
    }

    #[test]
    fn multi_image_import_and_handle_resize_keep_asymmetric_geometry() {
        let mut document = Document::new(RgbaImage::new(640, 360));
        let ids = document.add_images(
            vec![
                (RgbaImage::new(91, 37), "wide.png".into()),
                (RgbaImage::new(29, 83), "tall.png".into()),
            ],
            0,
        );
        assert_eq!(ids.len(), 2);
        let original = document
            .layers
            .iter()
            .find(|layer| layer.id == ids[0])
            .unwrap()
            .clone();
        let fixed = original.resize_handles()[2].1;
        let resized = resize_from_corner(
            &original,
            0,
            Point {
                x: fixed.x - 173.,
                y: fixed.y - 61.,
            },
        )
        .unwrap();
        let bounds = resized.geometry_bounds().unwrap();
        assert!((bounds.width - 173.).abs() < 0.01);
        assert!((bounds.height - 61.).abs() < 0.01);
        document.preview_layer(resized);
        assert!(document.commit_layer_preview(original));
        assert!(document.undo());
        assert_eq!(
            document.layers.len(),
            2,
            "resize undo must not undo grouped import"
        );
        assert!(document.undo());
        assert!(
            document.layers.is_empty(),
            "one undo removes the whole import batch"
        );
    }

    #[test]
    fn scaled_export_is_real_pixels_and_never_touches_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        fs::write(&source, b"source sentinel").unwrap();
        let rendered = Document::new(mock_artwork()).render().unwrap();
        let scaled =
            image::imageops::resize(&rendered, 317, 149, image::imageops::FilterType::Lanczos3);
        let bytes = encode_webp(&scaled, Some(70)).unwrap();
        let destination = dir.path().join("export.webp");
        atomic_write(&destination, &bytes).unwrap();
        let decoded = decode(&destination).unwrap();
        assert_eq!(decoded.dimensions(), (317, 149));
        assert_ne!(decoded.get_pixel(19, 71), decoded.get_pixel(203, 71));
        assert_eq!(fs::read(source).unwrap(), b"source sentinel");
    }

    #[test]
    fn export_dimensions_are_independent_when_aspect_is_unlocked() {
        let width = parse_dimension("Export width", "317".into()).unwrap();
        let height = parse_dimension("Export height", "149".into()).unwrap();
        assert_eq!((width, height), (317, 149));
        assert_eq!(proportional_height(width, 960, 600).unwrap(), 198);
    }

    #[test]
    fn export_field_validation_is_actionable() {
        assert!(
            parse_dimension("Export width", "0".into())
                .unwrap_err()
                .contains("1 and 16384")
        );
        assert!(
            parse_bounded_u8("Quality", "101".into(), 1, 100)
                .unwrap_err()
                .contains("1 to 100")
        );
        assert_eq!(parse_max_size("750KB".into()).unwrap(), Some(750_000));
        assert!(
            parse_max_size("large".into())
                .unwrap_err()
                .contains("for example 750KB")
        );
        assert_eq!(
            parse_color("Stroke", "12,34,56,78".into()).unwrap(),
            [12, 34, 56, 78]
        );
        assert!(
            parse_color("Fill", "#xyz".into())
                .unwrap_err()
                .contains("#RRGGBB")
        );
    }

    #[test]
    fn compression_preview_uses_cropped_source_but_requested_output_dimensions() {
        let mut document = Document::new(mock_artwork());
        document.set_crop(Rect {
            x: 11.,
            y: 19.,
            width: 321.,
            height: 177.,
        });
        let preview = encode_compression_preview(
            document.render().unwrap(),
            ExportSpec {
                format: Format::Jpeg,
                quality: 37,
                max_bytes: None,
                width: 123,
                height: 79,
            },
        )
        .unwrap();
        assert_eq!(preview.before.size(0).width, DevicePixels::from(321));
        assert_eq!(preview.before.size(0).height, DevicePixels::from(177));
        assert_eq!(preview.after.size(0).width, DevicePixels::from(123));
        assert_eq!(preview.after.size(0).height, DevicePixels::from(79));
        assert!(preview.before_bytes > 0);
        assert!(preview.after_bytes > 0);
    }

    #[test]
    fn compression_preview_fences_competing_requests_and_sources() {
        assert!(preview_result_matches(8, 8, 21, 21));
        assert!(!preview_result_matches(9, 8, 21, 21));
        assert!(!preview_result_matches(8, 8, 22, 21));
    }
}
