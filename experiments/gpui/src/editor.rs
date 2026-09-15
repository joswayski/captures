//! Native GPUI screenshot editor.
//!
//! The document/raster core is adapted from Captures' native GTK experiment;
//! all controls and interaction surfaces here are GPUI (no GTK widgets).
mod encoder;
mod model;
mod text_input;

use crate::ui::{Theme, button, icon, metric, root, theme};
use anyhow::{Context as _, Result};
use gpui::{
    Animation, AnimationExt as _, App, Bounds, ClipboardItem, Context, Div, ElementId, Entity,
    ExternalPaths, FocusHandle, Hsla, Image, ImageFormat, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, Window, WindowBounds, WindowOptions, actions,
    canvas as gpui_canvas, div, img, linear_color_stop, linear_gradient, point, prelude::*, px,
    size,
};
use image::RgbaImage;
use model::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

actions!(
    screenshot_editor,
    [
        Undo,
        Redo,
        DeleteLayer,
        DuplicateLayer,
        CopyLayer,
        PasteLayer,
        SelectTool,
        CropTool,
        TextTool,
        RectangleTool,
        EllipseTool,
        LineTool,
        ArrowTool,
        PenTool,
        RemoveBackgroundTool,
        ZoomIn,
        ZoomOut,
        ActualSize,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftLarge,
        NudgeRightLarge,
        NudgeUpLarge,
        NudgeDownLarge,
        CommitText,
        CancelText,
        SaveSource,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    Crop,
    Text,
    Rectangle,
    Ellipse,
    Line,
    Triangle,
    Diamond,
    Star,
    Arrow,
    Pen,
    RemoveBackground,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Crop => "Crop",
            Self::Text => "Text",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Line => "Line",
            Self::Triangle => "Triangle",
            Self::Diamond => "Diamond",
            Self::Star => "Star",
            Self::Arrow => "Arrow",
            Self::Pen => "Pen",
            Self::RemoveBackground => "Remove bg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EraseMode {
    Wand,
    Erase,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFormat {
    Png,
    Jpeg,
    Webp,
}

impl ExportFormat {
    fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualityMode {
    Preserve,
    Compress,
    Maximum,
}

const QUALITY_PRESETS: [(u8, &str); 5] = [
    (55, "Tiny"),
    (70, "Smaller"),
    (85, "Balanced"),
    (92, "High"),
    (98, "Highest"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
struct ComparisonRequest {
    generation: u64,
    format: ExportFormat,
    mode: QualityMode,
    quality: u8,
    maximum: u64,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
enum PickerPart {
    SaturationValue,
    Hue,
    Alpha,
}

#[derive(Clone, Copy)]
enum NumericTarget {
    CanvasWidth,
    CanvasHeight,
    OutputWidth,
    OutputHeight,
    LayerX(usize),
    LayerY(usize),
    LayerWidth(usize),
    LayerHeight(usize),
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
        frame: Rect,
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
        before: Document,
    },
    PathPoint {
        index: usize,
        point: isize,
        before: Document,
    },
    Pan {
        start: gpui::Point<Pixels>,
        offset: gpui::Point<Pixels>,
    },
}

struct ScreenshotEditor {
    doc: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    selected: Option<usize>,
    clipboard: Option<Layer>,
    tool: Tool,
    erase_mode: EraseMode,
    gesture: Option<Gesture>,
    preview: Option<Layer>,
    crop: Option<Rect>,
    crop_ratio: Option<f64>,
    zoom: f64,
    zoom_fit: bool,
    color: Color,
    stroke: f64,
    fill_shapes: bool,
    brush_softness: f64,
    wand_tolerance: u8,
    wand_contiguous: bool,
    hover_point: Option<Point>,
    source: Option<PathBuf>,
    source_ready: bool,
    draft: PathBuf,
    dirty: bool,
    restored: bool,
    rendered: Arc<Image>,
    render_generation: u64,
    canvas_bounds: Arc<Mutex<Bounds<Pixels>>>,
    canvas_scroll: ScrollHandle,
    focus: FocusHandle,
    status: SharedString,
    format: ExportFormat,
    quality_mode: QualityMode,
    quality: u8,
    maximum_bytes: u64,
    output_scale: f64,
    output_width: u32,
    output_height: u32,
    output_aspect_locked: bool,
    export_open: bool,
    shape_menu_open: bool,
    format_menu_open: bool,
    quality_menu_open: bool,
    quality_mode_menu_open: bool,
    make_copy: bool,
    comparison_request: Option<ComparisonRequest>,
    comparison_status: SharedString,
    comparison: Option<(Arc<Image>, usize, usize)>,
    comparison_split: f32,
    comparison_dragging: bool,
    text_input: Entity<text_input::TextInput>,
    output_name: Entity<text_input::TextInput>,
    layer_name: Entity<text_input::TextInput>,
    numeric_input: Entity<text_input::TextInput>,
    color_input: Entity<text_input::TextInput>,
    color_picker_open: bool,
    picker_hue: f32,
    picker_saturation: f32,
    picker_value: f32,
    picker_alpha: f32,
    picker_sv_bounds: Arc<Mutex<Bounds<Pixels>>>,
    picker_hue_bounds: Arc<Mutex<Bounds<Pixels>>>,
    picker_alpha_bounds: Arc<Mutex<Bounds<Pixels>>>,
    stroke_bounds: Arc<Mutex<Bounds<Pixels>>>,
    stroke_dragging: bool,
    stroke_focus: FocusHandle,
    editing_text: Option<(usize, Document)>,
    editing_layer_name: Option<usize>,
    numeric_target: NumericTarget,
    layer_drag: Option<(usize, Document, bool)>,
    space_pan: bool,
}

fn draft_path(source: Option<&Path>, image: &RgbaImage) -> PathBuf {
    let mut hash = DefaultHasher::new();
    source.hash(&mut hash);
    image.width().hash(&mut hash);
    image.height().hash(&mut hash);
    image.as_raw().hash(&mut hash);
    crate::settings::data_dir()
        .join("editor-drafts")
        .join(format!("{:016x}.json", hash.finish()))
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if !matches!(value.len(), 6 | 8) || !value.is_ascii() {
        return None;
    }
    let channel = |offset| u8::from_str_radix(&value[offset..offset + 2], 16).ok();
    Some(Color(
        f64::from(channel(0)?) / 255.,
        f64::from(channel(2)?) / 255.,
        f64::from(channel(4)?) / 255.,
        f64::from(if value.len() == 8 { channel(6)? } else { 255 }) / 255.,
    ))
}

fn hsv_color(hue: f32, saturation: f32, value: f32, alpha: f32) -> Color {
    let hue = hue.rem_euclid(1.) * 6.;
    let chroma = value * saturation;
    let x = chroma * (1. - (hue.rem_euclid(2.) - 1.).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let m = value - chroma;
    Color(
        f64::from(r + m),
        f64::from(g + m),
        f64::from(b + m),
        f64::from(alpha.clamp(0., 1.)),
    )
}

fn color_hsv(color: Color) -> (f32, f32, f32, f32) {
    let r = color.0 as f32;
    let g = color.1 as f32;
    let b = color.2 as f32;
    let maximum = r.max(g).max(b);
    let minimum = r.min(g).min(b);
    let delta = maximum - minimum;
    let hue = if delta == 0. {
        0.
    } else if maximum == r {
        ((g - b) / delta).rem_euclid(6.) / 6.
    } else if maximum == g {
        ((b - r) / delta + 2.) / 6.
    } else {
        ((r - g) / delta + 4.) / 6.
    };
    let saturation = if maximum == 0. { 0. } else { delta / maximum };
    (hue, saturation, maximum, color.3 as f32)
}

fn color_hex(color: Color) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        (color.0 * 255.).round().clamp(0., 255.) as u8,
        (color.1 * 255.).round().clamp(0., 255.) as u8,
        (color.2 * 255.).round().clamp(0., 255.) as u8,
        (color.3 * 255.).round().clamp(0., 255.) as u8,
    )
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("invalid output path")?;
    std::fs::create_dir_all(parent)?;
    for nonce in 0..100_u32 {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("invalid output filename")?;
        let staging = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&staging) {
            Ok(mut file) => {
                let result = file
                    .write_all(bytes)
                    .and_then(|_| file.sync_all())
                    .and_then(|_| std::fs::rename(&staging, path));
                if result.is_err() {
                    let _ = std::fs::remove_file(&staging);
                }
                return result.map_err(Into::into);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("could not allocate a private staging file")
}

fn png_image(doc: &Document) -> Result<Arc<Image>> {
    let image = model::render(doc).map_err(anyhow::Error::msg)?;
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        bytes.into_inner(),
    )))
}

fn load_source(path: &Path) -> Result<(Document, PathBuf, bool, Arc<Image>)> {
    let image = image::open(path)
        .with_context(|| format!("open screenshot {}", path.display()))?
        .to_rgba8();
    let draft = draft_path(Some(path), &image);
    let restored_doc = std::fs::read(&draft)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Document>(&bytes).ok())
        .filter(|doc| doc.width > 0 && doc.height > 0);
    let restored = restored_doc.is_some();
    let doc = restored_doc.unwrap_or_else(|| Document::new(image));
    let rendered = png_image(&doc)?;
    Ok((doc, draft, restored, rendered))
}

fn encode_scaled(
    doc: &Document,
    format: ExportFormat,
    mode: QualityMode,
    quality: u8,
    maximum: u64,
    output_width: u32,
    output_height: u32,
) -> Result<Vec<u8>> {
    let image = model::render(doc).map_err(anyhow::Error::msg)?;
    let output_width = output_width.clamp(1, 16_384);
    let output_height = output_height.clamp(1, 16_384);
    let image = if image.dimensions() == (output_width, output_height) {
        image
    } else {
        image::imageops::resize(
            &image,
            output_width,
            output_height,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let result = match (format, mode) {
        (ExportFormat::Png, QualityMode::Preserve) => encoder::encode_png(&image, None),
        (ExportFormat::Png, QualityMode::Compress) => encoder::encode_png(&image, Some(quality)),
        (ExportFormat::Png, QualityMode::Maximum) => {
            encoder::encode_png_with_limit(&image, maximum)
        }
        (ExportFormat::Jpeg, QualityMode::Preserve) => encoder::encode_jpeg(&image, 100),
        (ExportFormat::Jpeg, QualityMode::Compress) => encoder::encode_jpeg(&image, quality),
        (ExportFormat::Jpeg, QualityMode::Maximum) => {
            encoder::encode_jpeg_with_limit(&image, maximum)
        }
        (ExportFormat::Webp, QualityMode::Preserve) => encoder::encode_webp(&image, None),
        (ExportFormat::Webp, QualityMode::Compress) => encoder::encode_webp(&image, Some(quality)),
        (ExportFormat::Webp, QualityMode::Maximum) => {
            encoder::encode_webp_with_limit(&image, maximum)
        }
    };
    result.map_err(anyhow::Error::msg)
}

#[cfg(test)]
fn encode(
    doc: &Document,
    format: ExportFormat,
    mode: QualityMode,
    quality: u8,
    maximum: u64,
) -> Result<Vec<u8>> {
    encode_scaled(doc, format, mode, quality, maximum, doc.width, doc.height)
}

impl ScreenshotEditor {
    fn new(
        image: RgbaImage,
        source: Option<PathBuf>,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let draft = draft_path(source.as_deref(), &image);
        let restored_doc = std::fs::read(&draft)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Document>(&bytes).ok())
            .filter(|doc| doc.width > 0 && doc.height > 0);
        let restored = restored_doc.is_some();
        let doc = restored_doc.unwrap_or_else(|| Document::new(image));
        let rendered = png_image(&doc)?;
        let output_width = doc.width;
        let output_height = doc.height;
        let initial_color = Color(242. / 255., 61. / 255., 79. / 255., 1.);
        let (picker_hue, picker_saturation, picker_value, picker_alpha) = color_hsv(initial_color);
        let output_name = cx.new(text_input::TextInput::new);
        let layer_name = cx.new(text_input::TextInput::new);
        let numeric_input = cx.new(text_input::TextInput::new);
        let color_input = cx.new(text_input::TextInput::new);
        numeric_input.update(cx, |input, cx| input.set(doc.width.to_string(), cx));
        color_input.update(cx, |input, cx| input.set(color_hex(initial_color), cx));
        let suggested_name = source
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|stem| stem.to_str())
            .unwrap_or("Capture-edited")
            .to_string();
        let format = source
            .as_deref()
            .and_then(ExportFormat::for_path)
            .unwrap_or(ExportFormat::Png);
        output_name.update(cx, |input, cx| input.set(suggested_name, cx));
        Ok(Self {
            doc,
            undo: vec![],
            redo: vec![],
            selected: None,
            clipboard: None,
            tool: Tool::Select,
            erase_mode: EraseMode::Wand,
            gesture: None,
            preview: None,
            crop: None,
            crop_ratio: None,
            zoom: 1.,
            zoom_fit: true,
            color: initial_color,
            stroke: 4.,
            fill_shapes: false,
            brush_softness: 0.18,
            wand_tolerance: 36,
            wand_contiguous: true,
            hover_point: None,
            source,
            source_ready: true,
            draft,
            dirty: restored,
            restored,
            rendered,
            render_generation: 0,
            canvas_bounds: Default::default(),
            canvas_scroll: ScrollHandle::new(),
            focus,
            status: if restored {
                "Unsaved editing draft restored".into()
            } else {
                "Ready".into()
            },
            format,
            quality_mode: QualityMode::Preserve,
            quality: 92,
            maximum_bytes: 10 * 1024 * 1024,
            output_scale: 1.,
            output_width,
            output_height,
            output_aspect_locked: true,
            export_open: false,
            shape_menu_open: false,
            format_menu_open: false,
            quality_menu_open: false,
            quality_mode_menu_open: false,
            make_copy: false,
            comparison_request: None,
            comparison_status: "".into(),
            comparison: None,
            comparison_split: 0.5,
            comparison_dragging: false,
            text_input: cx.new(text_input::TextInput::new),
            output_name,
            layer_name,
            numeric_input,
            color_input,
            color_picker_open: false,
            picker_hue,
            picker_saturation,
            picker_value,
            picker_alpha,
            picker_sv_bounds: Default::default(),
            picker_hue_bounds: Default::default(),
            picker_alpha_bounds: Default::default(),
            stroke_bounds: Default::default(),
            stroke_dragging: false,
            stroke_focus: cx.focus_handle(),
            editing_text: None,
            editing_layer_name: None,
            numeric_target: NumericTarget::CanvasWidth,
            layer_drag: None,
            space_pan: false,
        })
    }

    fn input_focused(&self, window: &Window, cx: &App) -> bool {
        self.text_input.read(cx).is_focused(window)
            || self.output_name.read(cx).is_focused(window)
            || self.layer_name.read(cx).is_focused(window)
            || self.numeric_input.read(cx).is_focused(window)
            || self.color_input.read(cx).is_focused(window)
    }

    fn begin_text_edit(
        &mut self,
        index: usize,
        before: Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match &self.doc.layers[index].kind {
            LayerKind::Text(text) => text.clone(),
            _ => return,
        };
        self.text_input.update(cx, |input, cx| input.set(text, cx));
        self.text_input.read(cx).focus(window);
        self.editing_text = Some((index, before));
        self.selected = Some(index);
        cx.notify();
    }

    fn commit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, before)) = self.editing_text.take() else {
            return;
        };
        let value = self.text_input.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.doc.layers.remove(index);
            self.selected = None;
        } else if let Some(layer) = self.doc.layers.get_mut(index) {
            layer.kind = LayerKind::Text(value);
        }
        self.undo.push(before);
        self.redo.clear();
        self.changed();
        self.rerender(cx);
        self.focus.focus(window);
        cx.notify();
    }

    fn cancel_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, before)) = self.editing_text.take() {
            self.doc = before;
            self.selected = None;
            self.rerender(cx);
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn begin_layer_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.doc.layers[index].name.clone();
        self.layer_name.update(cx, |input, cx| input.set(name, cx));
        self.layer_name.read(cx).focus(window);
        self.editing_layer_name = Some(index);
        cx.notify();
    }

    fn commit_layer_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.editing_layer_name.take() else {
            return;
        };
        let name = self.layer_name.read(cx).value().trim().to_string();
        if !name.is_empty() && name != self.doc.layers[index].name {
            self.checkpoint();
            self.doc.layers[index].name = name;
            self.changed();
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn begin_numeric(
        &mut self,
        target: NumericTarget,
        value: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.numeric_target = target;
        self.numeric_input
            .update(cx, |input, cx| input.set(format!("{value:.0}"), cx));
        self.numeric_input.read(cx).focus(window);
        cx.notify();
    }

    fn apply_numeric(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(value) = self.numeric_input.read(cx).value().trim().parse::<f64>() else {
            self.status = "Enter a valid number".into();
            cx.notify();
            return;
        };
        let document_change = !matches!(
            self.numeric_target,
            NumericTarget::OutputWidth | NumericTarget::OutputHeight
        );
        if document_change {
            self.checkpoint();
        }
        match self.numeric_target {
            NumericTarget::CanvasWidth => self.doc.width = value.clamp(1., 16_384.) as u32,
            NumericTarget::CanvasHeight => self.doc.height = value.clamp(1., 16_384.) as u32,
            NumericTarget::OutputWidth => {
                self.output_width = value.clamp(1., 16_384.) as u32;
                if self.output_aspect_locked {
                    self.output_height = (f64::from(self.output_width) * f64::from(self.doc.height)
                        / f64::from(self.doc.width))
                    .round()
                    .max(1.) as u32;
                }
                self.output_scale = f64::from(self.output_width) / f64::from(self.doc.width);
            }
            NumericTarget::OutputHeight => {
                self.output_height = value.clamp(1., 16_384.) as u32;
                if self.output_aspect_locked {
                    self.output_width = (f64::from(self.output_height) * f64::from(self.doc.width)
                        / f64::from(self.doc.height))
                    .round()
                    .max(1.) as u32;
                }
                self.output_scale = f64::from(self.output_height) / f64::from(self.doc.height);
            }
            NumericTarget::LayerX(index) => self.doc.layers[index].frame.x = value,
            NumericTarget::LayerY(index) => self.doc.layers[index].frame.y = value,
            NumericTarget::LayerWidth(index) => {
                self.doc.layers[index].frame.w = value.clamp(1., 16_384.)
            }
            NumericTarget::LayerHeight(index) => {
                self.doc.layers[index].frame.h = value.clamp(1., 16_384.)
            }
        }
        if document_change {
            self.changed();
            self.rerender(cx);
        } else {
            self.status = "Output size updated".into();
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn apply_color(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(color) = parse_hex_color(self.color_input.read(cx).value()) else {
            self.status = "Enter a color as #RRGGBB or #RRGGBBAA".into();
            cx.notify();
            return;
        };
        (
            self.picker_hue,
            self.picker_saturation,
            self.picker_value,
            self.picker_alpha,
        ) = color_hsv(color);
        if let Some(index) = self.selected {
            self.checkpoint();
            let layer = &mut self.doc.layers[index];
            layer.color = color;
            if layer.fill.is_some() {
                layer.fill = Some(Color(color.0, color.1, color.2, color.3 * 0.28));
            }
            self.changed();
            self.rerender(cx);
        } else {
            self.color = color;
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn begin_picker_change(&mut self) {
        if self.selected.is_some() {
            self.checkpoint();
        }
    }

    fn update_picker(
        &mut self,
        part: PickerPart,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let bounds = *match part {
            PickerPart::SaturationValue => &self.picker_sv_bounds,
            PickerPart::Hue => &self.picker_hue_bounds,
            PickerPart::Alpha => &self.picker_alpha_bounds,
        }
        .lock()
        .expect("color picker bounds");
        if f32::from(bounds.size.width) <= 0. || f32::from(bounds.size.height) <= 0. {
            return;
        }
        let x =
            (f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width)).clamp(0., 1.);
        let y =
            (f32::from(position.y - bounds.origin.y) / f32::from(bounds.size.height)).clamp(0., 1.);
        match part {
            PickerPart::SaturationValue => {
                self.picker_saturation = x;
                self.picker_value = 1. - y;
            }
            PickerPart::Hue => self.picker_hue = x,
            PickerPart::Alpha => self.picker_alpha = x,
        }
        let color = hsv_color(
            self.picker_hue,
            self.picker_saturation,
            self.picker_value,
            self.picker_alpha,
        );
        self.color_input
            .update(cx, |input, cx| input.set(color_hex(color), cx));
        if let Some(index) = self.selected {
            let layer = &mut self.doc.layers[index];
            layer.color = color;
            if layer.fill.is_some() {
                layer.fill = Some(Color(color.0, color.1, color.2, color.3 * 0.28));
            }
            self.changed();
            self.rerender(cx);
        } else {
            self.color = color;
        }
        cx.notify();
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 50 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self) {
        self.dirty = true;
        match serde_json::to_vec(&self.doc)
            .map_err(Into::into)
            .and_then(|bytes| atomic_write(&self.draft, &bytes))
        {
            Ok(()) => self.status = "Draft saved privately".into(),
            Err(error) => self.status = format!("Draft error: {error}").into(),
        }
    }

    fn rerender(&mut self, cx: &mut Context<Self>) {
        self.render_generation = self.render_generation.wrapping_add(1);
        let generation = self.render_generation;
        let doc = self.doc.clone();
        let task = cx
            .background_executor()
            .spawn(async move { png_image(&doc) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if generation != this.render_generation {
                    return;
                }
                match result {
                    Ok(image) => this.rendered = image,
                    Err(error) => this.status = format!("Render error: {error}").into(),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let stack = if redo { &mut self.redo } else { &mut self.undo };
        if let Some(next) = stack.pop() {
            let old = std::mem::replace(&mut self.doc, next);
            if redo {
                self.undo.push(old)
            } else {
                self.redo.push(old)
            }
            self.selected = None;
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn select_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        self.shape_menu_open = false;
        if matches!(tool, Tool::Crop | Tool::Pen | Tool::RemoveBackground) {
            self.selected = None;
        }
        self.preview = None;
        self.crop = None;
        cx.notify();
    }

    fn document_point(&self, position: gpui::Point<Pixels>) -> Point {
        let bounds = *self.canvas_bounds.lock().expect("canvas bounds");
        Point {
            x: f64::from(position.x - bounds.origin.x) / self.zoom,
            y: f64::from(position.y - bounds.origin.y) / self.zoom,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.document_point(event.position);
        self.hover_point = Some(p);
        let before = self.doc.clone();
        if self.space_pan || event.modifiers.control || event.modifiers.platform {
            self.gesture = Some(Gesture::Pan {
                start: event.position,
                offset: self.canvas_scroll.offset(),
            });
            cx.notify();
            return;
        }
        match self.tool {
            Tool::Select => {
                self.selected = self.doc.layers.iter().rposition(|layer| hit(layer, p));
                if event.click_count >= 2
                    && let Some(index) = self.selected
                    && matches!(self.doc.layers[index].kind, LayerKind::Text(_))
                {
                    self.begin_text_edit(index, before, window, cx);
                    return;
                }
                if event.click_count >= 2
                    && let Some(index) = self.selected
                    && matches!(
                        self.doc.layers[index].kind,
                        LayerKind::Line(..) | LayerKind::Arrow(..)
                    )
                {
                    self.undo.push(before);
                    let local = local_point(&self.doc.layers[index], p);
                    let controls = &mut self.doc.layers[index].controls;
                    if let Some(control) = controls
                        .iter()
                        .position(|control| distance(*control, local) <= 10. / self.zoom)
                    {
                        controls.remove(control);
                    } else {
                        controls.push(local);
                    }
                    self.changed();
                    self.rerender(cx);
                    return;
                }
                if let Some(index) = self
                    .selected
                    .filter(|index| !self.doc.layers[*index].locked)
                {
                    let layer = &self.doc.layers[index];
                    let local = local_point(layer, p);
                    let path_point = match layer.kind {
                        LayerKind::Line(a, b) | LayerKind::Arrow(a, b) => {
                            if distance(local, a) <= 10. / self.zoom {
                                Some(-1)
                            } else if distance(local, b) <= 10. / self.zoom {
                                Some(-2)
                            } else {
                                layer
                                    .controls
                                    .iter()
                                    .position(|point| distance(local, *point) <= 10. / self.zoom)
                                    .map(|index| index as isize)
                            }
                        }
                        _ => None,
                    };
                    let rotate = rotate_handle(layer);
                    if let Some(point) = path_point {
                        self.gesture = Some(Gesture::PathPoint {
                            index,
                            point,
                            before,
                        });
                    } else if distance(p, rotate) <= 10. / self.zoom {
                        let center = Point {
                            x: layer.frame.x + layer.frame.w / 2.,
                            y: layer.frame.y + layer.frame.h / 2.,
                        };
                        self.gesture = Some(Gesture::Rotate {
                            index,
                            center,
                            offset: (p.y - center.y).atan2(p.x - center.x) - layer.rotation,
                            before,
                        });
                    } else if let Some(handle) = handles(layer)
                        .iter()
                        .position(|point| distance(p, *point) <= 9. / self.zoom)
                    {
                        self.gesture = Some(Gesture::Resize {
                            index,
                            frame: layer.frame,
                            handle,
                            before,
                        });
                    } else {
                        self.gesture = Some(Gesture::Move {
                            index,
                            start: p,
                            frame: layer.frame,
                            before,
                        });
                    }
                }
            }
            Tool::Crop => {
                self.crop = Some(Rect {
                    x: p.x,
                    y: p.y,
                    w: 0.,
                    h: 0.,
                });
                self.gesture = Some(Gesture::Crop { start: p });
            }
            Tool::Text => {
                let index = self.doc.add(
                    LayerKind::Text(String::new()),
                    Rect {
                        x: p.x,
                        y: p.y,
                        w: 180.,
                        h: 48.,
                    },
                    self.color,
                    self.stroke,
                );
                self.tool = Tool::Select;
                self.begin_text_edit(index, before, window, cx);
            }
            Tool::RemoveBackground => {
                if let Some(index) = self.doc.layers.iter().rposition(|layer| {
                    hit(layer, p) && matches!(layer.kind, LayerKind::Image { .. })
                }) {
                    self.checkpoint();
                    let result = if self.erase_mode == EraseMode::Wand {
                        remove_color(
                            &mut self.doc.layers[index],
                            p,
                            self.wand_tolerance,
                            self.wand_contiguous,
                        )
                    } else {
                        erase_soft(
                            &mut self.doc.layers[index],
                            p,
                            self.stroke * 3.,
                            self.erase_mode == EraseMode::Restore,
                            self.brush_softness,
                        )
                    };
                    match result {
                        Ok(()) => {
                            self.selected = Some(index);
                            if self.erase_mode == EraseMode::Wand {
                                self.changed();
                                self.rerender(cx);
                            } else {
                                self.gesture = Some(Gesture::Erase { index, before });
                            }
                        }
                        Err(error) => self.status = error.into(),
                    }
                }
            }
            _ => {
                self.gesture = Some(Gesture::Draw {
                    start: p,
                    points: vec![p],
                })
            }
        }
        cx.notify();
    }

    fn pan_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.gesture = Some(Gesture::Pan {
            start: event.position,
            offset: self.canvas_scroll.offset(),
        });
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.document_point(event.position);
        self.hover_point = Some(p);
        if self.comparison_dragging {
            self.comparison_split = (p.x / f64::from(self.doc.width)).clamp(0.06, 0.94) as f32;
            cx.notify();
            return;
        }
        let Some(gesture) = self.gesture.take() else {
            if self.tool == Tool::RemoveBackground {
                cx.notify();
            }
            return;
        };
        match gesture {
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
                let snapped = snap_translation(&self.doc, index, candidate, 10. / self.zoom);
                self.doc.layers[index].frame = snapped;
                self.gesture = Some(Gesture::Move {
                    index,
                    start,
                    frame,
                    before,
                });
            }
            Gesture::Resize {
                index,
                frame,
                handle,
                before,
            } => {
                let keep_aspect = matches!(self.doc.layers[index].kind, LayerKind::Image { .. })
                    || event.modifiers.shift;
                resize_layer(&mut self.doc.layers[index], frame, handle, p, keep_aspect);
                self.gesture = Some(Gesture::Resize {
                    index,
                    frame,
                    handle,
                    before,
                });
            }
            Gesture::Rotate {
                index,
                center,
                offset,
                before,
            } => {
                let mut rotation = (p.y - center.y).atan2(p.x - center.x) - offset;
                if event.modifiers.shift {
                    rotation = (rotation / (std::f64::consts::PI / 12.)).round()
                        * (std::f64::consts::PI / 12.);
                }
                self.doc.layers[index].rotation = rotation;
                self.gesture = Some(Gesture::Rotate {
                    index,
                    center,
                    offset,
                    before,
                });
            }
            Gesture::Crop { start } => {
                let mut rect = Rect::normalized(start, p);
                if let Some(ratio) = self.crop_ratio.or(event.modifiers.shift.then_some(1.)) {
                    if rect.w / rect.h.max(1.) > ratio {
                        rect.w = rect.h * ratio;
                    } else {
                        rect.h = rect.w / ratio;
                    }
                    rect.x = if p.x < start.x {
                        start.x - rect.w
                    } else {
                        start.x
                    };
                    rect.y = if p.y < start.y {
                        start.y - rect.h
                    } else {
                        start.y
                    };
                }
                self.crop = Some(rect);
                self.gesture = Some(Gesture::Crop { start });
            }
            Gesture::Draw { start, mut points } => {
                points.push(p);
                let frame = Rect::normalized(start, p);
                let local = |point: Point| Point {
                    x: point.x - frame.x,
                    y: point.y - frame.y,
                };
                let kind = match self.tool {
                    Tool::Pen => LayerKind::Stroke(points.iter().copied().map(local).collect()),
                    Tool::Arrow => LayerKind::Arrow(local(start), local(p)),
                    Tool::Line => LayerKind::Line(local(start), local(p)),
                    Tool::Ellipse => LayerKind::Ellipse,
                    Tool::Triangle => LayerKind::Triangle,
                    Tool::Diamond => LayerKind::Diamond,
                    Tool::Star => LayerKind::Star,
                    _ => LayerKind::Rectangle,
                };
                let mut layer = preview_layer(kind, frame, self.color, self.stroke);
                if self.fill_shapes
                    && matches!(
                        self.tool,
                        Tool::Rectangle
                            | Tool::Ellipse
                            | Tool::Triangle
                            | Tool::Diamond
                            | Tool::Star
                    )
                {
                    layer.fill = Some(Color(self.color.0, self.color.1, self.color.2, 0.28));
                }
                self.preview = Some(layer);
                self.gesture = Some(Gesture::Draw { start, points });
            }
            Gesture::Erase { index, before } => {
                let _ = erase_soft(
                    &mut self.doc.layers[index],
                    p,
                    self.stroke * 3.,
                    self.erase_mode == EraseMode::Restore,
                    self.brush_softness,
                );
                self.gesture = Some(Gesture::Erase { index, before });
            }
            Gesture::PathPoint {
                index,
                point,
                before,
            } => {
                let local = local_point(&self.doc.layers[index], p);
                match &mut self.doc.layers[index].kind {
                    LayerKind::Line(a, _) | LayerKind::Arrow(a, _) if point == -1 => *a = local,
                    LayerKind::Line(_, b) | LayerKind::Arrow(_, b) if point == -2 => *b = local,
                    _ if point >= 0 => self.doc.layers[index].controls[point as usize] = local,
                    _ => {}
                }
                self.gesture = Some(Gesture::PathPoint {
                    index,
                    point,
                    before,
                });
            }
            Gesture::Pan { start, offset } => {
                self.canvas_scroll.set_offset(point(
                    offset.x + event.position.x - start.x,
                    offset.y + event.position.y - start.y,
                ));
                self.gesture = Some(Gesture::Pan { start, offset });
            }
        }
        // Deliberately no full document raster here. GPUI paints gesture chrome;
        // the expensive Cairo composite runs once at mouse-up.
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.comparison_dragging = false;
        let gesture = self.gesture.take();
        match gesture {
            Some(Gesture::Move { before, .. })
            | Some(Gesture::Resize { before, .. })
            | Some(Gesture::Rotate { before, .. })
            | Some(Gesture::Erase { before, .. })
            | Some(Gesture::PathPoint { before, .. }) => {
                self.undo.push(before);
                self.redo.clear();
                self.changed();
                self.rerender(cx);
            }
            Some(Gesture::Pan { .. }) => {}
            Some(Gesture::Draw { .. }) => {
                if let Some(layer) = self
                    .preview
                    .take()
                    .filter(|layer| layer.frame.w >= 1. || layer.frame.h >= 1.)
                {
                    self.checkpoint();
                    let index = self
                        .doc
                        .add(layer.kind, layer.frame, layer.color, layer.stroke);
                    self.doc.layers[index].fill = layer.fill;
                    self.selected = Some(index);
                    self.changed();
                    self.rerender(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !(event.control || event.platform) {
            return;
        }
        let old_zoom = self.zoom;
        let amount = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if amount < 0. {
            self.zoom = (self.zoom * 1.12).min(8.);
        } else if amount > 0. {
            self.zoom = (self.zoom / 1.12).max(0.05);
        }
        self.zoom_fit = false;
        if (self.zoom - old_zoom).abs() > f64::EPSILON {
            let bounds = *self.canvas_bounds.lock().expect("canvas bounds");
            let ratio = (self.zoom / old_zoom) as f32;
            let offset = self.canvas_scroll.offset();
            self.canvas_scroll.set_offset(point(
                offset.x - (event.position.x - bounds.origin.x) * (ratio - 1.),
                offset.y - (event.position.y - bounds.origin.y) * (ratio - 1.),
            ));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn apply_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(rect) = self.crop.take().filter(|r| r.w >= 1. && r.h >= 1.) {
            self.checkpoint();
            self.doc.crop(rect);
            self.tool = Tool::Select;
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected.take().filter(|i| !self.doc.layers[*i].locked) {
            self.checkpoint();
            self.doc.layers.remove(index);
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.checkpoint();
            self.selected = self.doc.duplicate(index);
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn reorder_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(index) = self.selected else { return };
        let target = index
            .saturating_add_signed(delta)
            .min(self.doc.layers.len() - 1);
        if target == index {
            return;
        }
        self.checkpoint();
        if let Some(next) = self.doc.reorder(index, target) {
            self.selected = Some(next);
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn nudge(&mut self, dx: f64, dy: f64, cx: &mut Context<Self>) {
        if let Some(index) = self
            .selected
            .filter(|index| !self.doc.layers[*index].locked)
        {
            self.checkpoint();
            self.doc.layers[index].frame.x += dx;
            self.doc.layers[index].frame.y += dy;
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn merge_layers(&mut self, indices: Vec<usize>, name: &'static str, cx: &mut Context<Self>) {
        let before = self.doc.clone();
        let mut merged = self.doc.clone();
        self.status = format!("{name}…").into();
        let task = cx.background_executor().spawn(async move {
            merge_indices(&mut merged, &indices, name).map(|index| (merged, index))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((document, index)) => {
                        this.undo.push(before);
                        this.redo.clear();
                        this.doc = document;
                        this.selected = Some(index);
                        this.changed();
                        this.rerender(cx);
                    }
                    Err(error) => this.status = error.into(),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy_selected(&mut self) {
        self.clipboard = self
            .selected
            .and_then(|index| self.doc.layers.get(index))
            .cloned();
        self.status = if self.clipboard.is_some() {
            "Layer copied".into()
        } else {
            "Select a layer to copy".into()
        };
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(layer) = self.clipboard.clone() {
            self.checkpoint();
            self.selected = Some(paste_layer(&mut self.doc, &layer, self.selected, 24.));
            self.changed();
            self.rerender(cx);
            cx.notify();
        }
    }

    fn prompt_add_image(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add image layers".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                this.update(cx, |this, cx| {
                    this.add_paths(paths, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .filter_map(|path| image::open(&path).ok().map(|image| image.to_rgba8()))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            this.update(cx, |this, cx| {
                if loaded.is_empty() {
                    this.status = "No supported images were added".into();
                } else {
                    this.checkpoint();
                    for image in loaded {
                        let at = Point {
                            x: 24. + this.doc.layers.len() as f64 * 8.,
                            y: 24.,
                        };
                        this.selected = Some(this.doc.add_image(image, at));
                    }
                    this.changed();
                    this.rerender(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn export_to_new_path(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .source
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."));
        let stem = self.output_name.read(cx).value().trim().to_string();
        let stem = if stem.is_empty() {
            "Capture-edited".to_string()
        } else {
            stem
        };
        let suggested = format!("{stem}.{}", self.format.extension());
        let receiver = cx.prompt_for_new_path(directory, Some(&suggested));
        let doc = self.doc.clone();
        let format = self.format;
        let mode = self.quality_mode;
        let quality = self.quality;
        let maximum = self.maximum_bytes;
        let output_width = self.output_width;
        let output_height = self.output_height;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let output_path = path.clone();
                let result = executor
                    .spawn(async move {
                        encode_scaled(
                            &doc,
                            format,
                            mode,
                            quality,
                            maximum,
                            output_width,
                            output_height,
                        )
                        .and_then(|bytes| atomic_write(&output_path, &bytes).map(|_| bytes.len()))
                    })
                    .await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(size) => {
                            this.status =
                                format!("Saved {} ({} KB)", path.display(), size.div_ceil(1024))
                                    .into();
                            this.dirty = false;
                            let _ = std::fs::remove_file(&this.draft);
                            crate::app::saved(path.clone(), cx);
                        }
                        Err(error) => this.status = format!("Export failed: {error}").into(),
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn save_source(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.save_primary(cx);
    }

    fn save_primary(&mut self, cx: &mut Context<Self>) {
        if self.make_copy || self.format_requires_copy() {
            return self.export_to_new_path(cx);
        }
        let Some(path) = self.source.clone() else {
            return self.export_to_new_path(cx);
        };
        let doc = self.doc.clone();
        let format = self.format;
        let mode = self.quality_mode;
        let quality = self.quality;
        let maximum = self.maximum_bytes;
        let width = self.output_width;
        let height = self.output_height;
        let executor = cx.background_executor().clone();
        let output_path = path.clone();
        let task = executor.spawn(async move {
            encode_scaled(&doc, format, mode, quality, maximum, width, height)
                .and_then(|bytes| atomic_write(&output_path, &bytes))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.status = format!("Saved {}", path.display()).into();
                        this.dirty = false;
                        let _ = std::fs::remove_file(&this.draft);
                    }
                    Err(e) => this.status = format!("Save failed: {e}").into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn format_requires_copy(&self) -> bool {
        self.source.as_deref().and_then(ExportFormat::for_path) != Some(self.format)
    }

    fn refresh_comparison(&mut self, cx: &mut Context<Self>) {
        let request = (self.export_open && self.quality_mode != QualityMode::Preserve).then_some(
            ComparisonRequest {
                generation: self.render_generation,
                format: self.format,
                mode: self.quality_mode,
                quality: self.quality,
                maximum: self.maximum_bytes,
                width: self.output_width,
                height: self.output_height,
            },
        );
        if request == self.comparison_request {
            return;
        }
        self.comparison_request = request;
        self.comparison = None;
        self.comparison_status = "".into();
        let Some(request) = request else { return };
        self.comparison_status = "Preparing preview…".into();
        cx.spawn(async move |this, cx| {
            // Coalesce slider/resize changes before expensive encoding, and reject
            // stale completions after edits, preset changes, or closing the panel.
            gpui::Timer::after(Duration::from_millis(150)).await;
            let Ok(Some(doc)) = this.update(cx, |this, _| {
                (this.comparison_request == Some(request)).then(|| this.doc.clone())
            }) else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    let before = encode_scaled(
                        &doc,
                        ExportFormat::Png,
                        QualityMode::Preserve,
                        100,
                        u64::MAX,
                        request.width,
                        request.height,
                    )?;
                    let bytes = encode_scaled(
                        &doc,
                        request.format,
                        request.mode,
                        request.quality,
                        request.maximum,
                        request.width,
                        request.height,
                    )?;
                    let image_format = match request.format {
                        ExportFormat::Png => ImageFormat::Png,
                        ExportFormat::Jpeg => ImageFormat::Jpeg,
                        ExportFormat::Webp => ImageFormat::Webp,
                    };
                    Ok::<_, anyhow::Error>((
                        Arc::new(Image::from_bytes(image_format, bytes.clone())),
                        bytes.len(),
                        before.len(),
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.comparison_request != Some(request) {
                    return;
                }
                match result {
                    Ok(value) => {
                        let savings = if value.2 == 0 {
                            0
                        } else {
                            (100_f64 * (1. - value.1 as f64 / value.2 as f64)).round() as i32
                        };
                        this.comparison_status = format!(
                            "Before: {} KB  ·  After: {} KB  ·  {}% {}",
                            value.2.div_ceil(1024),
                            value.1.div_ceil(1024),
                            savings.abs(),
                            if savings >= 0 { "smaller" } else { "larger" },
                        )
                        .into();
                        this.comparison = Some(value);
                    }
                    Err(e) => this.comparison_status = format!("Preview failed: {e}").into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy_image(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_image(&self.rendered));
        self.status = "Edited image copied".into();
        cx.notify();
    }

    fn tool_button(&self, tool: Tool, t: Theme, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let active = self.tool == tool;
        let icon_name = match tool {
            Tool::Select => "select",
            Tool::Crop => "crop",
            Tool::Text => "text",
            Tool::Rectangle => "rectangle",
            Tool::Ellipse => "ellipse",
            Tool::Line => "line",
            Tool::Triangle => "triangle",
            Tool::Diamond => "diamond",
            Tool::Star => "star",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::RemoveBackground => "remove-bg",
        };
        div()
            .id(("tool", tool as usize))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size(px(38.))
            .flex_none()
            .rounded(metric("--r-lg"))
            .text_color(t.muted())
            .cursor_pointer()
            .hover(move |button| button.bg(t.color("--surface-hover")).text_color(t.text()))
            .child(icon(icon_name).size(px(18.)).text_color(if active {
                t.accent_ink
            } else {
                t.muted()
            }))
            .when(active, |b| b.bg(t.accent).text_color(t.accent_ink))
            .on_click(cx.listener(move |this, _, _, cx| this.select_tool(tool, cx)))
    }

    fn compact_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        t: Theme,
    ) -> gpui::Stateful<Div> {
        button(id, label, t).h(metric("--h-sm")).px(metric("--s-4"))
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        t: Theme,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .h(metric("--h-lg"))
            .flex_none()
            .flex()
            .items_center()
            .gap(metric("--s-3"))
            .cursor_pointer()
            .text_size(metric("--text-sm"))
            .child(
                div()
                    .w(px(32.))
                    .h(px(18.))
                    .p(px(2.))
                    .rounded_full()
                    .bg(if checked {
                        t.accent
                    } else {
                        t.color("--control-hover")
                    })
                    .flex()
                    .items_center()
                    .when(checked, |track| track.justify_end())
                    .child(div().size(px(14.)).rounded_full().bg(if checked {
                        t.accent_ink
                    } else {
                        t.muted()
                    })),
            )
            .child(label)
    }

    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        icon_name: &'static str,
        t: Theme,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(metric("--r-md"))
            .text_color(t.muted())
            .cursor_pointer()
            .hover(move |button| button.bg(t.color("--surface-hover")).text_color(t.text()))
            .child(icon(icon_name).size(px(16.)).text_color(t.muted()))
    }

    fn render_tool_rail(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let shape_active = matches!(
            self.tool,
            Tool::Rectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Triangle
                | Tool::Diamond
                | Tool::Star
        );
        let shape_icon = match self.tool {
            Tool::Ellipse => "ellipse",
            Tool::Line => "line",
            Tool::Triangle => "triangle",
            Tool::Diamond => "diamond",
            Tool::Star => "star",
            _ => "shapes",
        };
        let shape_trigger = div()
            .id("shape-tool")
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size(px(38.))
            .flex_none()
            .rounded(metric("--r-lg"))
            .text_color(t.muted())
            .cursor_pointer()
            .hover(move |button| button.bg(t.color("--surface-hover")).text_color(t.text()))
            .child(icon(shape_icon).size(px(18.)).text_color(if shape_active {
                t.accent_ink
            } else {
                t.muted()
            }))
            .child(
                div()
                    .absolute()
                    .right(px(5.))
                    .bottom(px(3.))
                    .child(icon("chevron-down").size(px(7.)).text_color(t.muted())),
            )
            .when(shape_active, |button| {
                button.bg(t.accent).text_color(t.accent_ink)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if !matches!(
                    this.tool,
                    Tool::Rectangle
                        | Tool::Ellipse
                        | Tool::Line
                        | Tool::Triangle
                        | Tool::Diamond
                        | Tool::Star
                ) {
                    this.tool = Tool::Rectangle;
                    this.selected = None;
                }
                this.shape_menu_open = !this.shape_menu_open;
                cx.notify();
            }));

        div()
            .relative()
            .w(px(56.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(metric("--s-1"))
            .px(metric("--s-3"))
            .py(metric("--s-4"))
            .border_r_1()
            .border_color(t.border())
            .bg(t.raised())
            .child(self.tool_button(Tool::Select, t, cx))
            .child(self.tool_button(Tool::Crop, t, cx))
            .child(self.tool_button(Tool::Text, t, cx))
            .child(shape_trigger)
            .child(self.tool_button(Tool::Arrow, t, cx))
            .child(self.tool_button(Tool::Pen, t, cx))
            .child(self.tool_button(Tool::RemoveBackground, t, cx))
            .when(self.shape_menu_open, |rail| {
                rail.child(gpui::deferred(
                    div()
                        .occlude()
                        .absolute()
                        .left(px(50.))
                        .top(px(142.))
                        .w(px(3. * 44. + 2.) + metric("--s-2") * 2. + metric("--s-3") * 2.)
                        .p(metric("--s-3"))
                        .flex()
                        .flex_wrap()
                        .gap(metric("--s-2"))
                        .rounded(metric("--r-lg"))
                        .border_1()
                        .border_color(t.border())
                        .bg(t.color("--surface-overlay"))
                        .shadow_lg()
                        .children(
                            [
                                Tool::Rectangle,
                                Tool::Ellipse,
                                Tool::Line,
                                Tool::Triangle,
                                Tool::Diamond,
                                Tool::Star,
                            ]
                            .map(|tool| self.tool_button(tool, t, cx).size(px(44.))),
                        ),
                ))
            })
    }

    fn set_stroke(&mut self, value: f64, cx: &mut Context<Self>) {
        let value = value.round().clamp(2., 40.);
        if let Some(index) = self.selected {
            if self.doc.layers[index].stroke == value {
                return;
            }
            self.doc.layers[index].stroke = value;
            self.changed();
            self.rerender(cx);
        } else {
            self.stroke = value;
        }
        cx.notify();
    }

    fn drag_stroke(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = *self.stroke_bounds.lock().expect("stroke bounds");
        self.set_stroke(stroke_at(position.x, bounds), cx);
    }

    fn stroke_control(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let value = self
            .selected
            .map_or(self.stroke, |index| self.doc.layers[index].stroke);
        let progress = ((value - 2.) / 38.).clamp(0., 1.) as f32;
        let bounds_slot = self.stroke_bounds.clone();
        div()
            .flex()
            .flex_col()
            .gap(metric("--s-2"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(t.muted()).child("Stroke width"))
                    .child(
                        div()
                            .text_color(t.muted())
                            .text_size(metric("--text-xs"))
                            .child(format!("{value:.0} px")),
                    ),
            )
            .child(
                div()
                    .id("stroke-slider")
                    .key_context("StrokeSlider")
                    .track_focus(&self.stroke_focus)
                    .h(px(20.))
                    .w_full()
                    .cursor_pointer()
                    .rounded(metric("--r-sm"))
                    .focus(|style| style.bg(t.color("--surface-hover")))
                    .child(
                        gpui_canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                *bounds_slot.lock().expect("stroke bounds") = bounds;
                                let track = Bounds::new(
                                    bounds.origin + point(px(7.), px(8.)),
                                    size(bounds.size.width - px(14.), px(4.)),
                                );
                                window.paint_quad(gpui::quad(
                                    track,
                                    px(2.),
                                    t.color("--n-6"),
                                    px(0.),
                                    gpui::transparent_black(),
                                    Default::default(),
                                ));
                                window.paint_quad(gpui::quad(
                                    Bounds::new(
                                        track.origin,
                                        size(track.size.width * progress, track.size.height),
                                    ),
                                    px(2.),
                                    t.accent,
                                    px(0.),
                                    gpui::transparent_black(),
                                    Default::default(),
                                ));
                                let thumb = Bounds::new(
                                    point(
                                        track.origin.x + track.size.width * progress - px(7.),
                                        bounds.origin.y + px(3.),
                                    ),
                                    size(px(14.), px(14.)),
                                );
                                window.paint_quad(gpui::quad(
                                    thumb,
                                    px(7.),
                                    t.accent,
                                    px(0.),
                                    gpui::transparent_black(),
                                    Default::default(),
                                ));
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.stroke_focus.focus(window);
                            this.begin_picker_change();
                            this.stroke_dragging = true;
                            this.drag_stroke(event.position, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        let next = match event.keystroke.key.as_str() {
                            "left" | "down" => value - 1.,
                            "right" | "up" => value + 1.,
                            "home" => 2.,
                            "end" => 40.,
                            _ => return,
                        };
                        this.begin_picker_change();
                        this.set_stroke(next, cx);
                        cx.stop_propagation();
                    })),
            )
    }

    fn color_control(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let selected = self
            .selected
            .and_then(|index| self.doc.layers.get(index))
            .map_or(self.color, |layer| layer.color);
        div()
            .flex()
            .flex_col()
            .gap(metric("--s-4"))
            .child(div().text_color(t.muted()).child("Color"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_y(metric("--s-4"))
                    .children(COLOR_SWATCHES.iter().enumerate().map(|(index, &hex)| {
                        let color = parse_hex_color(hex).expect("preset color");
                        let active = color_hex(selected)[..7].eq_ignore_ascii_case(hex);
                        div()
                            .w(gpui::relative(1. / 6.))
                            .h(px(36.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id(("color-swatch", index))
                                    .size(px(32.))
                                    .rounded_full()
                                    .border_2()
                                    .border_color(if active {
                                        t.accent
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .size(px(24.))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(t.border())
                                            .bg(gpui::rgba(
                                                u32::from_str_radix(
                                                    &format!("{}ff", &hex[1..]),
                                                    16,
                                                )
                                                .expect("preset rgba"),
                                            )),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.color_input.update(cx, |input, cx| {
                                            input.set(color_hex(color), cx)
                                        });
                                        this.apply_color(window, cx);
                                    })),
                            )
                    }))
                    .child(
                        div()
                            .w(gpui::relative(1. / 6.))
                            .h(px(36.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id("toggle-color-picker")
                                    .size(px(24.))
                                    .rounded(metric("--r-sm"))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .bg(linear_gradient(
                                        90.,
                                        linear_color_stop(gpui::rgba(0xff3b5cff), 0.),
                                        linear_color_stop(gpui::rgba(0x8b5cf6ff), 1.),
                                    ))
                                    .child(
                                        div()
                                            .size(px(16.))
                                            .rounded(metric("--r-xs"))
                                            .bg(t.raised())
                                            .child(icon("plus").size(px(16.)).text_color(t.text())),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.color_picker_open = !this.color_picker_open;
                                        if this.color_picker_open {
                                            let color = this
                                                .selected
                                                .and_then(|index| this.doc.layers.get(index))
                                                .map_or(this.color, |layer| layer.color);
                                            (
                                                this.picker_hue,
                                                this.picker_saturation,
                                                this.picker_value,
                                                this.picker_alpha,
                                            ) = color_hsv(color);
                                            this.color_input.update(cx, |input, cx| {
                                                input.set(color_hex(color), cx)
                                            });
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(self.color_picker_open, |picker| {
                picker
                    .child(
                        div().flex().items_center().gap(metric("--s-4")).child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .rounded(metric("--r-sm"))
                                .border_1()
                                .border_color(t.border())
                                .bg(t.canvas())
                                .child(self.color_input.clone()),
                        ),
                    )
                    .child(
                        self.render_color_picker(t, cx).with_animation(
                            "color-picker-reveal",
                            Animation::new(Duration::from_millis(140))
                                .with_easing(gpui::ease_out_quint()),
                            |picker, progress| picker.opacity(progress),
                        ),
                    )
            })
    }

    fn render_color_picker(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let sv_bounds = self.picker_sv_bounds.clone();
        let hue_bounds = self.picker_hue_bounds.clone();
        let alpha_bounds = self.picker_alpha_bounds.clone();
        let hue = Hsla {
            h: self.picker_hue,
            s: 1.,
            l: 0.5,
            a: 1.,
        };
        let chosen_hsla = Hsla {
            h: self.picker_hue,
            s: self.picker_saturation,
            l: self.picker_value * (1. - self.picker_saturation / 2.),
            a: self.picker_alpha,
        };
        div()
            .w(px(260.))
            .p(metric("--s-3"))
            .rounded(metric("--r-md"))
            .border_1()
            .border_color(t.border())
            .bg(t.canvas())
            .flex()
            .flex_col()
            .gap(metric("--s-3"))
            .child("SATURATION / VALUE")
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(132.))
                    .overflow_hidden()
                    .rounded(metric("--r-sm"))
                    .bg(hue)
                    .child(
                        gpui_canvas(
                            |_, _, _| {},
                            move |bounds, _, _, _| {
                                *sv_bounds.lock().expect("SV picker bounds") = bounds;
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(div().absolute().size_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(gpui::white(), 0.),
                        linear_color_stop(gpui::transparent_white(), 1.),
                    )))
                    .child(div().absolute().size_full().bg(linear_gradient(
                        180.,
                        linear_color_stop(gpui::transparent_black(), 0.),
                        linear_color_stop(gpui::black(), 1.),
                    )))
                    .child(
                        div()
                            .absolute()
                            .left(px(self.picker_saturation * 236. - 5.))
                            .top(px((1. - self.picker_value) * 132. - 5.))
                            .size(px(10.))
                            .rounded(px(5.))
                            .border_2()
                            .border_color(gpui::white())
                            .bg(chosen_hsla),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_picker_change();
                            this.update_picker(PickerPart::SaturationValue, event.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.update_picker(PickerPart::SaturationValue, event.position, cx);
                        }
                    })),
            )
            .child("HUE")
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(18.))
                    .overflow_hidden()
                    .rounded(px(9.))
                    .flex()
                    .child(
                        gpui_canvas(
                            |_, _, _| {},
                            move |bounds, _, _, _| {
                                *hue_bounds.lock().expect("hue picker bounds") = bounds;
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children((0..6).map(|index| {
                        let from = Hsla {
                            h: index as f32 / 6.,
                            s: 1.,
                            l: 0.5,
                            a: 1.,
                        };
                        let to = Hsla {
                            h: (index + 1) as f32 / 6.,
                            ..from
                        };
                        div().flex_1().bg(linear_gradient(
                            90.,
                            linear_color_stop(from, 0.),
                            linear_color_stop(to, 1.),
                        ))
                    }))
                    .child(
                        div()
                            .absolute()
                            .left(px(self.picker_hue * 236. - 2.))
                            .top_0()
                            .w(px(4.))
                            .h_full()
                            .bg(gpui::white())
                            .border_1()
                            .border_color(gpui::black()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_picker_change();
                            this.update_picker(PickerPart::Hue, event.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.update_picker(PickerPart::Hue, event.position, cx);
                        }
                    })),
            )
            .child(format!("ALPHA {:.0}%", self.picker_alpha * 100.))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(18.))
                    .overflow_hidden()
                    .rounded(px(9.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(
                            Hsla {
                                a: 0.,
                                ..chosen_hsla
                            },
                            0.,
                        ),
                        linear_color_stop(
                            Hsla {
                                a: 1.,
                                ..chosen_hsla
                            },
                            1.,
                        ),
                    ))
                    .child(
                        gpui_canvas(
                            |_, _, _| {},
                            move |bounds, _, _, _| {
                                *alpha_bounds.lock().expect("alpha picker bounds") = bounds;
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(self.picker_alpha * 236. - 2.))
                            .top_0()
                            .w(px(4.))
                            .h_full()
                            .bg(gpui::white())
                            .border_1()
                            .border_color(gpui::black()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_picker_change();
                            this.update_picker(PickerPart::Alpha, event.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.update_picker(PickerPart::Alpha, event.position, cx);
                        }
                    })),
            )
    }

    fn canvas_view(&self, t: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let background_bounds_slot = self.canvas_bounds.clone();
        let zoom = self.zoom;
        let image = self.rendered.clone();
        let comparison = self.comparison.clone();
        let comparison_split = self.comparison_split;
        let comparison_width = self.doc.width as f32 * zoom as f32 * comparison_split;
        let image_width = self.doc.width as f32 * zoom as f32;
        let image_height = self.doc.height as f32 * zoom as f32;
        let crop = self.crop;
        let selected = self.selected.and_then(|i| self.doc.layers.get(i)).cloned();
        let preview = self.preview.clone();
        let remove_background = self.tool == Tool::RemoveBackground;
        let erase_mode = self.erase_mode;
        let hover_point = self.hover_point;
        let brush_radius = self.stroke * 1.5 * zoom;
        let accent = t.accent;
        div()
            .id("screenshot-canvas")
            .relative()
            .flex_none()
            .w(px(self.doc.width as f32 * zoom as f32))
            .h(px(self.doc.height as f32 * zoom as f32))
            .bg(t.raised())
            .shadow_lg()
            .overflow_hidden()
            .child(
                gpui_canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        // This first absolute child is the canvas' actual painted surface. GPUI
                        // 0.2.2 can report a translated origin for later overlapping canvas
                        // children inside a scroll container, so pointer mapping is anchored here.
                        *background_bounds_slot.lock().expect("canvas bounds") = bounds;
                        let tile = px(12.);
                        let columns = (f32::from(bounds.size.width) / 12.).ceil() as usize;
                        let rows = (f32::from(bounds.size.height) / 12.).ceil() as usize;
                        for y in 0..rows {
                            for x in 0..columns {
                                let shade = if (x + y).is_multiple_of(2) {
                                    Hsla {
                                        h: 0.,
                                        s: 0.,
                                        l: if t.dark { 0.24 } else { 0.88 },
                                        a: 1.,
                                    }
                                } else {
                                    Hsla {
                                        h: 0.,
                                        s: 0.,
                                        l: if t.dark { 0.18 } else { 0.78 },
                                        a: 1.,
                                    }
                                };
                                let tile_bounds = Bounds::new(
                                    point(
                                        bounds.origin.x + tile * x as f32,
                                        bounds.origin.y + tile * y as f32,
                                    ),
                                    size(tile, tile),
                                );
                                window.paint_quad(gpui::quad(
                                    tile_bounds,
                                    px(0.),
                                    shade,
                                    px(0.),
                                    shade,
                                    Default::default(),
                                ));
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(comparison.is_none(), |canvas| {
                canvas.child(img(image.clone()).size_full())
            })
            .when_some(comparison, |canvas, (after, _, _)| {
                canvas
                    .child(img(after).size_full())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .w(px(comparison_width))
                            .h(px(image_height))
                            .overflow_hidden()
                            .child(
                                img(image)
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .w(px(image_width))
                                    .h(px(image_height)),
                            ),
                    )
                    .child(
                        div()
                            .id("comparison-divider")
                            .absolute()
                            .top_0()
                            .left(px(comparison_width - 2.))
                            .w(px(4.))
                            .h_full()
                            .bg(accent)
                            .cursor_col_resize()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.comparison_dragging = true;
                                    cx.stop_propagation();
                                }),
                            )
                            .with_animation(
                                "comparison-divider-reveal",
                                Animation::new(Duration::from_millis(160))
                                    .with_easing(gpui::ease_out_quint()),
                                |divider, progress| divider.opacity(progress),
                            ),
                    )
            })
            .child(
                gpui_canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if let Some(layer) = selected
                            .as_ref()
                            .filter(|layer| !layer.locked && layer.visible)
                        {
                            let r = layer_bounds(layer);
                            let b = Bounds::new(
                                point(
                                    bounds.origin.x + px((r.x * zoom) as f32),
                                    bounds.origin.y + px((r.y * zoom) as f32),
                                ),
                                size(px((r.w * zoom) as f32), px((r.h * zoom) as f32)),
                            );
                            window.paint_quad(gpui::quad(
                                b,
                                px(0.),
                                gpui::transparent_black(),
                                px(2.),
                                accent,
                                Default::default(),
                            ));
                            for handle in handles(layer).into_iter().chain([rotate_handle(layer)]) {
                                let center = point(
                                    bounds.origin.x + px((handle.x * zoom) as f32),
                                    bounds.origin.y + px((handle.y * zoom) as f32),
                                );
                                let hb = Bounds::new(
                                    point(center.x - px(4.), center.y - px(4.)),
                                    size(px(8.), px(8.)),
                                );
                                window.paint_quad(gpui::quad(
                                    hb,
                                    px(2.),
                                    accent,
                                    px(1.),
                                    t.raised(),
                                    Default::default(),
                                ));
                            }
                            if let LayerKind::Line(a, b) | LayerKind::Arrow(a, b) = layer.kind {
                                for path_point in [a, b]
                                    .into_iter()
                                    .chain(layer.controls.iter().copied())
                                    .map(|point| document_layer_point(layer, point))
                                {
                                    let center = point(
                                        bounds.origin.x + px((path_point.x * zoom) as f32),
                                        bounds.origin.y + px((path_point.y * zoom) as f32),
                                    );
                                    window.paint_quad(gpui::quad(
                                        Bounds::new(
                                            point(center.x - px(5.), center.y - px(5.)),
                                            size(px(10.), px(10.)),
                                        ),
                                        px(5.),
                                        t.raised(),
                                        px(2.),
                                        accent,
                                        Default::default(),
                                    ));
                                }
                            }
                        }
                        if let Some(r) = crop {
                            let b = Bounds::new(
                                point(
                                    bounds.origin.x + px((r.x * zoom) as f32),
                                    bounds.origin.y + px((r.y * zoom) as f32),
                                ),
                                size(px((r.w * zoom) as f32), px((r.h * zoom) as f32)),
                            );
                            window.paint_quad(gpui::quad(
                                b,
                                px(0.),
                                Hsla {
                                    h: 0.,
                                    s: 0.,
                                    l: 0.,
                                    a: 0.12,
                                },
                                px(2.),
                                accent,
                                Default::default(),
                            ));
                        }
                        if let Some(layer) = preview.as_ref() {
                            let r = layer_bounds(layer);
                            let b = Bounds::new(
                                point(
                                    bounds.origin.x + px((r.x * zoom) as f32),
                                    bounds.origin.y + px((r.y * zoom) as f32),
                                ),
                                size(px((r.w * zoom) as f32), px((r.h * zoom) as f32)),
                            );
                            window.paint_quad(gpui::quad(
                                b,
                                px(2.),
                                Hsla {
                                    h: accent.h,
                                    s: accent.s,
                                    l: accent.l,
                                    a: 0.16,
                                },
                                px((layer.stroke * zoom) as f32),
                                accent,
                                Default::default(),
                            ));
                        }
                        if remove_background && let Some(hover) = hover_point {
                            let radius = if erase_mode == EraseMode::Wand {
                                8.
                            } else {
                                brush_radius
                            } as f32;
                            let center = point(
                                bounds.origin.x + px((hover.x * zoom) as f32),
                                bounds.origin.y + px((hover.y * zoom) as f32),
                            );
                            window.paint_quad(gpui::quad(
                                Bounds::new(
                                    point(center.x - px(radius), center.y - px(radius)),
                                    size(px(radius * 2.), px(radius * 2.)),
                                ),
                                px(radius),
                                Hsla {
                                    h: 0.,
                                    s: 0.,
                                    l: 1.,
                                    a: 0.08,
                                },
                                px(2.),
                                accent,
                                Default::default(),
                            ));
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::pan_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
    }

    fn render_sidebar(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let mut layers = div().flex().flex_col().gap(metric("--s-3"));
        for (index, layer) in self.doc.layers.iter().enumerate().rev() {
            let selected = self.selected == Some(index);
            let name = layer.name.clone();
            let visible = layer.visible;
            let locked = layer.locked;
            layers = layers.child(
                div()
                    .id(("layer", index))
                    .flex()
                    .items_center()
                    .gap(metric("--s-3"))
                    .min_h(px(54.))
                    .px(metric("--s-3"))
                    .rounded(metric("--r-lg"))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, _| {
                            this.layer_drag = Some((index, this.doc.clone(), false));
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        let Some((source, before, changed)) = this.layer_drag.take() else {
                            return;
                        };
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.layer_drag = None;
                            return;
                        }
                        if source != index
                            && let Some(new_index) = this.doc.reorder(source, index)
                        {
                            if !changed {
                                this.undo.push(before.clone());
                                this.redo.clear();
                            }
                            this.selected = Some(new_index);
                            this.changed();
                            this.rerender(cx);
                            this.layer_drag = Some((new_index, before, true));
                            cx.notify();
                        } else {
                            this.layer_drag = Some((source, before, changed));
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.layer_drag = None),
                    )
                    .when(selected, |d| {
                        d.bg(Hsla {
                            a: 0.16,
                            ..t.accent
                        })
                        .border_1()
                        .border_color(t.accent)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(index);
                        this.tool = Tool::Select;
                        cx.notify();
                    }))
                    .child(
                        self.icon_button(
                            ("visibility", index),
                            if visible { "eye" } else { "hide" },
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.checkpoint();
                            this.doc.layers[index].visible = !this.doc.layers[index].visible;
                            this.changed();
                            this.rerender(cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .size(px(40.))
                            .h(px(30.))
                            .overflow_hidden()
                            .rounded(metric("--r-sm"))
                            .border_1()
                            .border_color(t.border())
                            .child(img(self.rendered.clone()).size_full()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(metric("--s-1"))
                            .text_size(metric("--text-sm"))
                            .child(name)
                            .child(
                                div()
                                    .text_size(metric("--text-xs"))
                                    .text_color(t.color("--text-subtle"))
                                    .child(if locked {
                                        "Locked background"
                                    } else {
                                        "Image layer"
                                    }),
                            ),
                    )
                    .child(
                        self.icon_button(
                            ("lock", index),
                            if locked { "lock" } else { "unlock" },
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.checkpoint();
                            this.doc.layers[index].locked = !this.doc.layers[index].locked;
                            this.changed();
                            cx.notify();
                        })),
                    ),
            );
        }
        let mut properties = div()
            .flex()
            .flex_col()
            .gap(metric("--s-4"))
            .pt(metric("--s-5"));
        if self.tool == Tool::Crop {
            properties = properties
                .child("CROP ASPECT")
                .child(
                    div().flex().flex_wrap().gap(metric("--s-3")).children(
                        [
                            (None, "Free"),
                            (Some(1.), "1:1"),
                            (Some(4. / 3.), "4:3"),
                            (Some(3. / 2.), "3:2"),
                            (Some(16. / 9.), "16:9"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (ratio, label))| {
                            self.compact_button(("crop-ratio", index), label, t)
                                .when(self.crop_ratio == ratio, |button| {
                                    button.bg(t.accent).text_color(t.accent_ink)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.crop_ratio = ratio;
                                    cx.notify();
                                }))
                        }),
                    ),
                )
                .child("Drag on the canvas, then apply or cancel. Hold Shift for transform aspect locking.");
        } else if self.tool == Tool::RemoveBackground {
            properties = properties
                .child("REMOVE BACKGROUND")
                .child(
                    div().flex().gap(metric("--s-3")).children(
                        [
                            (EraseMode::Wand, "Wand"),
                            (EraseMode::Erase, "Erase"),
                            (EraseMode::Restore, "Restore"),
                        ]
                        .map(|(mode, label)| {
                            self.compact_button(("erase", mode as usize), label, t)
                                .when(self.erase_mode == mode, |b| {
                                    b.bg(t.accent).text_color(t.glass_text())
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.erase_mode = mode;
                                    cx.notify();
                                }))
                        }),
                    ),
                )
                .child(format!(
                    "Tolerance {}  •  Brush {} px  •  Softness {}%",
                    self.wand_tolerance,
                    (self.stroke * 3.) as u32,
                    (self.brush_softness * 100.) as u32
                ))
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("tolerance-less", "Tolerance −", t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.wand_tolerance = this.wand_tolerance.saturating_sub(8);
                                    cx.notify();
                                })),
                        )
                        .child(
                            self.compact_button("tolerance-more", "Tolerance +", t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.wand_tolerance =
                                        this.wand_tolerance.saturating_add(8).min(120);
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(self.compact_button("brush-less", "Brush −", t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.stroke = (this.stroke - 1.).max(1.);
                                cx.notify();
                            }),
                        ))
                        .child(self.compact_button("brush-more", "Brush +", t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.stroke = (this.stroke + 1.).min(40.);
                                cx.notify();
                            }),
                        ))
                        .child(self.compact_button("softness", "Softness +", t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.brush_softness = (this.brush_softness + 0.1) % 1.1;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    self.compact_button(
                        "toggle-contiguous",
                        if self.wand_contiguous {
                            "✓ Contiguous only"
                        } else {
                            "Contiguous only"
                        },
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wand_contiguous = !this.wand_contiguous;
                        cx.notify();
                    })),
                );
        } else if let Some((index, layer)) = self
            .selected
            .and_then(|i| self.doc.layers.get(i).map(|l| (i, l)))
        {
            properties = properties
                .child("LAYER NAME")
                .child(
                    div()
                        .rounded(metric("--r-sm"))
                        .border_1()
                        .border_color(t.border())
                        .bg(t.canvas())
                        .child(self.layer_name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(self.compact_button("rename-layer", "Rename", t).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.begin_layer_rename(index, window, cx)
                            }),
                        ))
                        .child(
                            self.compact_button("apply-layer-name", "Apply name", t)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.commit_layer_rename(window, cx)
                                })),
                        ),
                )
                .child("TRANSFORM")
                .child(format!(
                    "X {:.0}   Y {:.0}   W {:.0}   H {:.0}",
                    layer.frame.x, layer.frame.y, layer.frame.w, layer.frame.h
                ))
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("edit-x", "Edit X", t)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let value = this.doc.layers[index].frame.x;
                                    this.begin_numeric(
                                        NumericTarget::LayerX(index),
                                        value,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            self.compact_button("edit-y", "Edit Y", t)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let value = this.doc.layers[index].frame.y;
                                    this.begin_numeric(
                                        NumericTarget::LayerY(index),
                                        value,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            self.compact_button("edit-w", "Edit W", t)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let value = this.doc.layers[index].frame.w;
                                    this.begin_numeric(
                                        NumericTarget::LayerWidth(index),
                                        value,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            self.compact_button("edit-h", "Edit H", t)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let value = this.doc.layers[index].frame.h;
                                    this.begin_numeric(
                                        NumericTarget::LayerHeight(index),
                                        value,
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                )
                .child(format!(
                    "Rotation {:.1}°  •  Opacity {:.0}%  •  {}",
                    layer.rotation.to_degrees(),
                    layer.opacity * 100.,
                    layer.blend.label()
                ))
                .child(self.color_control(t, cx))
                .child(self.stroke_control(t, cx))
                .when(
                    matches!(
                        layer.kind,
                        LayerKind::Rectangle
                            | LayerKind::Ellipse
                            | LayerKind::Triangle
                            | LayerKind::Diamond
                            | LayerKind::Star
                    ),
                    |properties| {
                        properties.child(
                            self.toggle("layer-fill", "Fill", layer.fill.is_some(), t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    let layer = &mut this.doc.layers[index];
                                    layer.fill = layer.fill.is_none().then_some(Color(
                                        layer.color.0,
                                        layer.color.1,
                                        layer.color.2,
                                        layer.color.3 * 0.28,
                                    ));
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                })),
                        )
                    },
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("opacity-less", "Opacity −", t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    this.doc.layers[index].opacity =
                                        (this.doc.layers[index].opacity - 0.1).max(0.1);
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            self.compact_button("opacity-more", "Opacity +", t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    this.doc.layers[index].opacity =
                                        (this.doc.layers[index].opacity + 0.1).min(1.);
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("rotate-left-small", "−15°", t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    this.doc.layers[index].rotation -= std::f64::consts::PI / 12.;
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            self.compact_button("rotate-right-small", "+15°", t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    this.doc.layers[index].rotation += std::f64::consts::PI / 12.;
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            self.compact_button(
                                "blend",
                                format!("Blend: {}", layer.blend.label()),
                                t,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkpoint();
                                    let current = this.doc.layers[index].blend;
                                    let at =
                                        Blend::ALL.iter().position(|b| *b == current).unwrap_or(0);
                                    this.doc.layers[index].blend =
                                        Blend::ALL[(at + 1) % Blend::ALL.len()];
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("duplicate", "Duplicate", t).on_click(
                                cx.listener(|this, _, _, cx| this.duplicate_selected(cx)),
                            ),
                        )
                        .child(
                            self.compact_button("delete", "Delete", t)
                                .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            self.compact_button("layer-lower", "Lower", t).on_click(
                                cx.listener(|this, _, _, cx| this.reorder_selected(-1, cx)),
                            ),
                        )
                        .child(
                            self.compact_button("layer-raise", "Raise", t).on_click(
                                cx.listener(|this, _, _, cx| this.reorder_selected(1, cx)),
                            ),
                        )
                        .when(index > 0, |row| {
                            row.child(self.compact_button("merge-down", "Merge down", t).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.merge_layers(vec![index - 1, index], "Merged layer", cx)
                                }),
                            ))
                        }),
                )
                .when(
                    matches!(layer.kind, LayerKind::Line(..) | LayerKind::Arrow(..)),
                    |properties| {
                        properties.child(
                            div()
                                .flex()
                                .gap(metric("--s-3"))
                                .child(
                                    self.compact_button("curve-negative", "Curve −", t)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.checkpoint();
                                            set_curve(&mut this.doc.layers[index], -50.);
                                            this.changed();
                                            this.rerender(cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    self.compact_button("curve-straight", "Straighten", t)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.checkpoint();
                                            this.doc.layers[index].controls.clear();
                                            this.changed();
                                            this.rerender(cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    self.compact_button("curve-positive", "Curve +", t)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.checkpoint();
                                            set_curve(&mut this.doc.layers[index], 50.);
                                            this.changed();
                                            this.rerender(cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                    },
                )
                .when(matches!(layer.kind, LayerKind::Image { .. }), |p| {
                    p.child(
                        div()
                            .flex()
                            .gap(metric("--s-3"))
                            .child(self.compact_button("rotate", "↷ Rotate", t).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.checkpoint();
                                    transform_image(&mut this.doc.layers[index], true);
                                    this.changed();
                                    this.rerender(cx);
                                    cx.notify();
                                }),
                            ))
                            .child(
                                self.compact_button("flip", "⇆ Flip", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        flip_image(&mut this.doc.layers[index], true);
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                .when(!matches!(layer.kind, LayerKind::Image { .. }), |p| {
                    p.child(
                        self.compact_button(
                            "shadow",
                            if layer.shadow.is_some() {
                                "✓ Drop shadow"
                            } else {
                                "Drop shadow"
                            },
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.checkpoint();
                            let layer = &mut this.doc.layers[index];
                            layer.shadow = if layer.shadow.is_some() {
                                None
                            } else {
                                Some(Shadow {
                                    color: Color(0., 0., 0., 0.45),
                                    blur: (layer.stroke * 0.85).max(6.),
                                    offset_x: 0.,
                                    offset_y: (layer.stroke * 0.32).max(2.),
                                })
                            };
                            this.changed();
                            this.rerender(cx);
                            cx.notify();
                        })),
                    )
                })
                .when(layer.shadow.is_some(), |p| {
                    p.child(format!(
                        "Shadow blur {:.0}  •  X {:.0}  Y {:.0}  •  {:.0}%",
                        layer.shadow.unwrap().blur,
                        layer.shadow.unwrap().offset_x,
                        layer.shadow.unwrap().offset_y,
                        layer.shadow.unwrap().color.3 * 100.
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(metric("--s-3"))
                            .child(
                                self.compact_button("shadow-blur-less", "Blur −", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.blur = (shadow.blur - 2.).max(0.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button("shadow-blur-more", "Blur +", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.blur = (shadow.blur + 2.).min(100.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button("shadow-offset-less", "Offset −", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.offset_x = (shadow.offset_x - 2.).max(-500.);
                                            shadow.offset_y = (shadow.offset_y - 2.).max(-500.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button("shadow-offset-more", "Offset +", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.offset_x = (shadow.offset_x + 2.).min(500.);
                                            shadow.offset_y = (shadow.offset_y + 2.).min(500.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button("shadow-opacity-less", "Alpha −", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.color.3 = (shadow.color.3 - 0.1).max(0.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button("shadow-opacity-more", "Alpha +", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.checkpoint();
                                        if let Some(shadow) = &mut this.doc.layers[index].shadow {
                                            shadow.color.3 = (shadow.color.3 + 0.1).min(1.);
                                        }
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                .when(matches!(layer.kind, LayerKind::Text(_)), |p| {
                    p.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(metric("--s-3"))
                            .child("TEXT")
                            .child(if self.editing_text.is_some() {
                                "Editing text directly on canvas"
                            } else {
                                "Double-click text or choose Edit text"
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap(metric("--s-3"))
                                    .child(
                                        self.compact_button("edit-text", "Edit text", t).on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                let before = this.doc.clone();
                                                this.begin_text_edit(index, before, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(self.compact_button("apply-text", "Apply", t).on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.commit_text(window, cx)
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(metric("--s-3"))
                                    .child(format!("Size {:.0}", layer.frame.h))
                                    .child(self.compact_button("text-size-less", "−", t).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.checkpoint();
                                            this.doc.layers[index].frame.h =
                                                (this.doc.layers[index].frame.h - 4.).max(8.);
                                            this.changed();
                                            this.rerender(cx);
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.compact_button("text-size-more", "+", t).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.checkpoint();
                                            this.doc.layers[index].frame.h =
                                                (this.doc.layers[index].frame.h + 4.).min(512.);
                                            this.changed();
                                            this.rerender(cx);
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(metric("--s-3"))
                                    .child(
                                        self.compact_button(
                                            "text-bold",
                                            if layer.text_style.bold {
                                                "✓ Bold"
                                            } else {
                                                "Bold"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let style = &mut this.doc.layers[index].text_style;
                                                style.bold = !style.bold;
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.compact_button(
                                            "text-italic",
                                            if layer.text_style.italic {
                                                "✓ Italic"
                                            } else {
                                                "Italic"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let style = &mut this.doc.layers[index].text_style;
                                                style.italic = !style.italic;
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.compact_button(
                                            "text-align",
                                            format!("Align: {}", layer.text_style.align),
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let align =
                                                    &mut this.doc.layers[index].text_style.align;
                                                *align = match align.as_str() {
                                                    "left" => "center",
                                                    "center" => "right",
                                                    _ => "left",
                                                }
                                                .into();
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(metric("--s-3"))
                                    .child(
                                        self.compact_button(
                                            "text-family",
                                            format!("Font: {}", layer.text_style.family),
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let family =
                                                    &mut this.doc.layers[index].text_style.family;
                                                *family = match family.as_str() {
                                                    "Sans" => "Serif",
                                                    "Serif" => "Monospace",
                                                    "Monospace" => "DejaVu Sans",
                                                    _ => "Sans",
                                                }
                                                .into();
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.compact_button(
                                            "text-background",
                                            if layer.text_style.background.is_some() {
                                                "✓ Background"
                                            } else {
                                                "Background"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let style = &mut this.doc.layers[index].text_style;
                                                style.background = if style.background.is_some() {
                                                    None
                                                } else {
                                                    Some(Color(0.05, 0.05, 0.06, 0.82))
                                                };
                                                style.rounded_background =
                                                    style.background.is_some();
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.compact_button(
                                            "text-outline",
                                            if layer.text_style.outlined {
                                                "✓ Outline"
                                            } else {
                                                "Outline"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let style = &mut this.doc.layers[index].text_style;
                                                style.outlined = !style.outlined;
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.compact_button(
                                            "text-rounded-background",
                                            if layer.text_style.rounded_background {
                                                "✓ Rounded"
                                            } else {
                                                "Rounded"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.checkpoint();
                                                let style = &mut this.doc.layers[index].text_style;
                                                style.rounded_background =
                                                    !style.rounded_background;
                                                this.changed();
                                                this.rerender(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            ),
                    )
                });
        } else if self.tool != Tool::Select {
            properties = properties
                .child(self.color_control(t, cx))
                .child(self.stroke_control(t, cx))
                .when(
                    matches!(
                        self.tool,
                        Tool::Rectangle
                            | Tool::Ellipse
                            | Tool::Triangle
                            | Tool::Diamond
                            | Tool::Star
                    ),
                    |properties| {
                        properties.child(self.toggle("fill", "Fill", self.fill_shapes, t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.fill_shapes = !this.fill_shapes;
                                cx.notify();
                            }),
                        ))
                    },
                );
        }
        div()
            .w(px(320.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(t.border())
            .bg(t.raised())
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(metric("--s-5"))
                    .border_b_1()
                    .border_color(t.border())
                    .child("Layers")
                    .child(
                        div()
                            .ml(metric("--s-3"))
                            .px(metric("--s-2"))
                            .rounded(px(10.))
                            .bg(t.color("--surface-sunken"))
                            .text_size(metric("--text-xs"))
                            .text_color(t.color("--text-subtle"))
                            .child(self.doc.layers.len().to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        self.compact_button("merge-visible", "⋈", t)
                            .w(px(30.))
                            .px_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let indices = this
                                    .doc
                                    .layers
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(index, layer)| layer.visible.then_some(index))
                                    .collect();
                                this.merge_layers(indices, "Merged visible", cx);
                            })),
                    )
                    .child(
                        self.compact_button("flatten", "▣", t)
                            .w(px(30.))
                            .px_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let indices = (0..this.doc.layers.len()).collect();
                                this.merge_layers(indices, "Flattened image", cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("layer-list-scroll")
                            .h(px(174.))
                            .flex_none()
                            .overflow_y_scroll()
                            .px(metric("--s-4"))
                            .py(metric("--s-3"))
                            .child(layers),
                    )
                    .child(
                        div()
                            .id("properties-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .border_t_1()
                            .border_color(t.border())
                            .px(metric("--s-5"))
                            .pb(metric("--s-6"))
                            .when(
                                self.selected.is_some() || self.tool != Tool::Select,
                                |panel| {
                                    panel.child(
                                        div()
                                            .h(px(48.))
                                            .flex()
                                            .items_center()
                                            .border_b_1()
                                            .border_color(t.border())
                                            .child(
                                                self.selected
                                                    .and_then(|index| self.doc.layers.get(index))
                                                    .map(|layer| layer.name.clone())
                                                    .unwrap_or_else(|| self.tool.label().into()),
                                            ),
                                    )
                                },
                            )
                            .child(properties),
                    ),
            )
    }

    fn format_control(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        div()
            .relative()
            .flex_none()
            .child(
                self.compact_button("format", format!(".{}  ⌄", self.format.extension()), t)
                    .h(metric("--h-lg"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.format_menu_open = !this.format_menu_open;
                        cx.notify();
                    })),
            )
            .when(self.format_menu_open, |control| {
                control.child(gpui::deferred(
                    div()
                        .occlude()
                        .absolute()
                        .bottom(metric("--h-lg") + metric("--s-4"))
                        .left_0()
                        .w(px(126.))
                        .p(metric("--s-2"))
                        .flex()
                        .flex_col()
                        .gap(metric("--s-1"))
                        .rounded(metric("--r-lg"))
                        .border_1()
                        .border_color(t.border())
                        .bg(t.color("--surface-overlay"))
                        .shadow_lg()
                        .children(
                            [
                                (ExportFormat::Png, "PNG"),
                                (ExportFormat::Jpeg, "JPEG"),
                                (ExportFormat::Webp, "WebP"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, (format, label))| {
                                self.compact_button(("format-option", index), label, t)
                                    .w_full()
                                    .when(self.format == format, |button| {
                                        button.bg(Hsla {
                                            a: 0.16,
                                            ..t.accent
                                        })
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.format = format;
                                        this.format_menu_open = false;
                                        cx.notify();
                                    }))
                            }),
                        ),
                ))
            })
    }

    fn quality_control(&self, presets: bool, t: Theme, cx: &mut Context<Self>) -> Div {
        let modes = [
            (QualityMode::Preserve, "Preserve quality"),
            (QualityMode::Compress, "Compress"),
            (QualityMode::Maximum, "Maximum file size"),
        ];
        let (id, heading, open, selected, choices) = if presets {
            (
                "quality-preset",
                "Quality",
                self.quality_menu_open,
                QUALITY_PRESETS
                    .iter()
                    .position(|(value, _)| *value == self.quality)
                    .unwrap_or(3),
                QUALITY_PRESETS
                    .iter()
                    .map(|(_, label)| *label)
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                "quality-mode",
                "Save quality",
                self.quality_mode_menu_open,
                modes
                    .iter()
                    .position(|(value, _)| *value == self.quality_mode)
                    .unwrap_or(0),
                modes.iter().map(|(_, label)| *label).collect::<Vec<_>>(),
            )
        };
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap(metric("--s-2"))
            .child(
                div()
                    .text_size(metric("--text-xs"))
                    .text_color(t.muted())
                    .child(heading),
            )
            .child(
                self.compact_button(id, choices[selected], t)
                    .h(metric("--h-lg"))
                    .child(icon("chevron-down").size(px(12.)).text_color(t.muted()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quality_menu_open = presets && !this.quality_menu_open;
                        this.quality_mode_menu_open = !presets && !this.quality_mode_menu_open;
                        this.format_menu_open = false;
                        cx.notify();
                    })),
            )
            .when(open, |control| {
                control.child(gpui::deferred(
                    div()
                        .absolute()
                        .bottom(metric("--h-lg") + metric("--s-2"))
                        .left_0()
                        .w(px(210.))
                        .p(metric("--s-2"))
                        .flex()
                        .flex_col()
                        .gap(metric("--s-1"))
                        .occlude()
                        .rounded(metric("--r-lg"))
                        .border_1()
                        .border_color(t.border())
                        .bg(t.color("--surface-overlay"))
                        .shadow_lg()
                        .children(choices.into_iter().enumerate().map(|(index, label)| {
                            self.compact_button((id, index), label, t)
                                .h(metric("--h-lg"))
                                .justify_start()
                                .when(index == selected, |choice| {
                                    choice.bg(Hsla {
                                        a: 0.16,
                                        ..t.accent
                                    })
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if presets {
                                        this.quality = QUALITY_PRESETS[index].0;
                                    } else {
                                        this.quality_mode = modes[index].0;
                                    }
                                    this.quality_menu_open = false;
                                    this.quality_mode_menu_open = false;
                                    cx.notify();
                                }))
                        })),
                ))
            })
    }

    fn footer(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let mut settings = div();
        if self.export_open {
            settings = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(metric("--s-4"))
                .p(metric("--s-6"))
                .bg(t.color("--surface-sunken"))
                .rounded(metric("--r-md"))
                .child(
                    self.compact_button(
                        "output-size",
                        match (self.output_width, self.output_height) {
                            size if size == (self.doc.width, self.doc.height) => "Output: Original",
                            size if size
                                == (
                                    (f64::from(self.doc.width) * 0.75).round() as u32,
                                    (f64::from(self.doc.height) * 0.75).round() as u32,
                                ) =>
                            {
                                "Output: 75%"
                            }
                            size if size
                                == (
                                    (f64::from(self.doc.width) * 0.5).round() as u32,
                                    (f64::from(self.doc.height) * 0.5).round() as u32,
                                ) =>
                            {
                                "Output: 50%"
                            }
                            _ => "Output: Custom",
                        },
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.output_scale = if (this.output_scale - 1.).abs() < f64::EPSILON {
                            0.75
                        } else if (this.output_scale - 0.75).abs() < f64::EPSILON {
                            0.5
                        } else {
                            1.
                        };
                        this.output_width =
                            (f64::from(this.doc.width) * this.output_scale).round() as u32;
                        this.output_height =
                            (f64::from(this.doc.height) * this.output_scale).round() as u32;
                        cx.notify();
                    })),
                )
                .child(format!("{} × {}", self.output_width, self.output_height))
                .child(
                    self.compact_button("output-width", "Edit W", t)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_numeric(
                                NumericTarget::OutputWidth,
                                f64::from(this.output_width),
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    self.compact_button("output-height", "Edit H", t)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_numeric(
                                NumericTarget::OutputHeight,
                                f64::from(this.output_height),
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    self.compact_button(
                        "output-aspect-lock",
                        if self.output_aspect_locked {
                            "🔒 Aspect"
                        } else {
                            "Aspect free"
                        },
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.output_aspect_locked = !this.output_aspect_locked;
                        cx.notify();
                    })),
                )
                .child(self.quality_control(false, t, cx))
                .when(self.quality_mode == QualityMode::Compress, |settings| {
                    settings.child(self.quality_control(true, t, cx))
                })
                .when(self.quality_mode == QualityMode::Maximum, |settings| {
                    settings
                        .child(format!("Limit {} KB", self.maximum_bytes.div_ceil(1024)))
                        .child(
                            self.compact_button("maximum-size", "Change limit", t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.maximum_bytes = match this.maximum_bytes {
                                        0..=262_144 => 512 * 1024,
                                        262_145..=524_288 => 1024 * 1024,
                                        524_289..=1_048_576 => 2 * 1024 * 1024,
                                        1_048_577..=2_097_152 => 5 * 1024 * 1024,
                                        _ => 256 * 1024,
                                    };
                                    cx.notify();
                                })),
                        )
                })
                .when(self.quality_mode != QualityMode::Preserve, |settings| {
                    settings.child(
                        div()
                            .text_size(metric("--text-xs"))
                            .text_color(t.muted())
                            .child(self.comparison_status.clone()),
                    )
                });
        }
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap(metric("--s-3"))
            .px(metric("--s-5"))
            .py(metric("--s-4"))
            .border_t_1()
            .border_color(t.border())
            .bg(t.raised())
            .when(self.export_open, |footer| footer.child(settings))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(metric("--s-5"))
                    .child(
                        div()
                            .id("export-settings")
                            .w(px(210.))
                            .min_w(px(178.))
                            .h(px(48.))
                            .px(metric("--s-4"))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(metric("--s-1"))
                            .rounded(metric("--r-md"))
                            .border_1()
                            .border_color(t.border())
                            .bg(t.color("--control"))
                            .cursor_pointer()
                            .hover(move |button| button.bg(t.color("--control-hover")))
                            .child(div().text_size(metric("--text-sm")).child(
                                if self.export_open {
                                    "Export settings  ⌃"
                                } else {
                                    "Export settings  ⌄"
                                },
                            ))
                            .child(
                                div()
                                    .text_size(metric("--text-xs"))
                                    .text_color(t.color("--text-subtle"))
                                    .child(format!(
                                        "{}  •  {} × {}",
                                        self.format.extension().to_uppercase(),
                                        self.output_width,
                                        self.output_height
                                    )),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_open = !this.export_open;
                                this.format_menu_open = false;
                                this.quality_menu_open = false;
                                this.quality_mode_menu_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .max_w(px(390.))
                            .flex()
                            .flex_col()
                            .gap(metric("--s-1"))
                            .child(
                                div()
                                    .text_size(metric("--text-xs"))
                                    .text_color(t.muted())
                                    .child("Filename"),
                            )
                            .child(
                                div()
                                    .h(px(36.))
                                    .rounded(metric("--r-md"))
                                    .border_1()
                                    .border_color(t.border())
                                    .bg(t.color("--surface-field"))
                                    .child(self.output_name.clone()),
                            ),
                    )
                    .child(self.format_control(t, cx))
                    .child(
                        self.compact_button("copy-image", "Copy image", t)
                            .h(metric("--h-lg"))
                            .px(metric("--s-5"))
                            .flex_none()
                            .on_click(cx.listener(Self::copy_image)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(metric("--h-lg"))
                            .flex()
                            .items_center()
                            .justify_end()
                            .text_size(metric("--text-xs"))
                            .text_color(t.muted())
                            .child(self.status.clone()),
                    )
                    .when(!self.format_requires_copy(), |row| {
                        row.child(
                            self.toggle("save-new", "Save as new file", self.make_copy, t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.make_copy = !this.make_copy;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        button("save-source", "Save", t)
                            .h(metric("--h-lg"))
                            .flex_none()
                            .min_w(px(106.))
                            .bg(t.accent)
                            .border_color(t.accent)
                            .text_color(t.accent_ink)
                            .child(icon("save").size(px(14.)).text_color(t.accent_ink))
                            .on_click(cx.listener(Self::save_source)),
                    ),
            )
    }
}

fn preview_layer(kind: LayerKind, frame: Rect, color: Color, stroke: f64) -> Layer {
    Layer {
        id: 0,
        name: "Preview".into(),
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
        shadow: None,
        text_style: TextStyle::default(),
        controls: vec![],
    }
}

fn distance(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn set_curve(layer: &mut Layer, amount: f64) {
    let (a, b) = match layer.kind {
        LayerKind::Line(a, b) | LayerKind::Arrow(a, b) => (a, b),
        _ => return,
    };
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = (dx * dx + dy * dy).sqrt().max(1.);
    layer.controls = vec![Point {
        x: (a.x + b.x) / 2. - dy / length * amount,
        y: (a.y + b.y) / 2. + dx / length * amount,
    }];
}

// Drawing colors match ScreenshotEditor.tsx; these are document colors, not chrome.
const COLOR_SWATCHES: [&str; 8] = [
    "#ff3b5c", "#ff8a22", "#ffd22e", "#36c96b", "#2d9cff", "#8b5cf6", "#111318", "#ffffff",
];

fn stroke_at(x: Pixels, bounds: Bounds<Pixels>) -> f64 {
    let track_width = f32::from(bounds.size.width) - 14.;
    let fraction = (f32::from(x - bounds.origin.x) - 7.) / track_width.max(1.);
    (2. + f64::from(fraction.clamp(0., 1.)) * 38.).round()
}

fn document_layer_point(layer: &Layer, local: Point) -> Point {
    let center = Point {
        x: layer.frame.w / 2.,
        y: layer.frame.h / 2.,
    };
    let x = local.x - center.x;
    let y = local.y - center.y;
    Point {
        x: layer.frame.x + center.x + x * layer.rotation.cos() - y * layer.rotation.sin(),
        y: layer.frame.y + center.y + x * layer.rotation.sin() + y * layer.rotation.cos(),
    }
}

fn handles(layer: &Layer) -> [Point; 8] {
    let r = layer_bounds(layer);
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

impl Render for ScreenshotEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if !self.source_ready {
            return root(t)
                .id("screenshot-loading")
                .p(metric("--s-8"))
                .child(self.status.clone());
        }
        self.refresh_comparison(cx);
        if self.zoom_fit {
            let viewport = window.viewport_size();
            let available_width = (f64::from(f32::from(viewport.width)) - 56. - 320. - 64.).max(1.);
            let footer_height = if self.export_open { 190. } else { 76. };
            let available_height =
                (f64::from(f32::from(viewport.height)) - 52. - footer_height - 64.).max(1.);
            self.zoom = (available_width / f64::from(self.doc.width))
                .min(available_height / f64::from(self.doc.height))
                .clamp(0.05, 8.);
        }
        root(t)
            .id("screenshot-editor")
            .relative()
            .track_focus(&self.focus)
            .key_context("ScreenshotEditor")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.numeric_input.read(cx).is_focused(window)
                {
                    this.focus.focus(window);
                    cx.stop_propagation();
                    cx.notify();
                } else if event.keystroke.key == "space" && !this.input_focused(window, cx) {
                    this.space_pan = true;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, _, cx| {
                if event.keystroke.key == "space" {
                    this.space_pan = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.stroke_dragging && event.pressed_button == Some(MouseButton::Left) {
                    this.drag_stroke(event.position, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.stroke_dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.stroke_dragging = false),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().to_vec(), cx)
            }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.focus.focus(window);
                this.undo(false, cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.focus.focus(window);
                this.undo(true, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteLayer, _, cx| this.delete_selected(cx)))
            .on_action(cx.listener(|this, _: &DuplicateLayer, _, cx| this.duplicate_selected(cx)))
            .on_action(cx.listener(|this, _: &CopyLayer, _, cx| {
                this.copy_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PasteLayer, _, cx| this.paste(cx)))
            .on_action(
                cx.listener(|this, _: &SelectTool, _, cx| this.select_tool(Tool::Select, cx)),
            )
            .on_action(cx.listener(|this, _: &CropTool, _, cx| this.select_tool(Tool::Crop, cx)))
            .on_action(cx.listener(|this, _: &TextTool, _, cx| this.select_tool(Tool::Text, cx)))
            .on_action(
                cx.listener(|this, _: &RectangleTool, _, cx| this.select_tool(Tool::Rectangle, cx)),
            )
            .on_action(
                cx.listener(|this, _: &EllipseTool, _, cx| this.select_tool(Tool::Ellipse, cx)),
            )
            .on_action(cx.listener(|this, _: &LineTool, _, cx| this.select_tool(Tool::Line, cx)))
            .on_action(cx.listener(|this, _: &ArrowTool, _, cx| this.select_tool(Tool::Arrow, cx)))
            .on_action(cx.listener(|this, _: &PenTool, _, cx| this.select_tool(Tool::Pen, cx)))
            .on_action(cx.listener(|this, _: &RemoveBackgroundTool, _, cx| {
                this.select_tool(Tool::RemoveBackground, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.zoom = (this.zoom * 1.25).min(8.);
                this.zoom_fit = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.zoom = (this.zoom / 1.25).max(0.05);
                this.zoom_fit = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ActualSize, _, cx| {
                this.zoom = 1.;
                this.zoom_fit = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NudgeLeft, _, cx| this.nudge(-1., 0., cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, cx| this.nudge(1., 0., cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, _, cx| this.nudge(0., -1., cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, _, cx| this.nudge(0., 1., cx)))
            .on_action(cx.listener(|this, _: &NudgeLeftLarge, _, cx| this.nudge(-10., 0., cx)))
            .on_action(cx.listener(|this, _: &NudgeRightLarge, _, cx| this.nudge(10., 0., cx)))
            .on_action(cx.listener(|this, _: &NudgeUpLarge, _, cx| this.nudge(0., -10., cx)))
            .on_action(cx.listener(|this, _: &NudgeDownLarge, _, cx| this.nudge(0., 10., cx)))
            .on_action(cx.listener(|this, _: &CommitText, window, cx| {
                if this.color_input.read(cx).is_focused(window) {
                    this.apply_color(window, cx);
                } else if this.numeric_input.read(cx).is_focused(window) {
                    this.apply_numeric(window, cx);
                } else {
                    this.commit_text(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CancelText, window, cx| this.cancel_text(window, cx)))
            .on_action(cx.listener(|this, _: &SaveSource, _, cx| this.save_primary(cx)))
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(metric("--s-5"))
                    .px(metric("--s-5"))
                    .border_b_1()
                    .border_color(t.border())
                    .bg(t.raised())
                    .child(
                        div()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .gap(metric("--s-2"))
                            .p(metric("--s-1"))
                            .rounded(metric("--r-lg"))
                            .border_1()
                            .border_color(t.border())
                            .bg(t.color("--surface-sunken"))
                            .child(
                                div()
                                    .px(metric("--s-3"))
                                    .text_size(metric("--text-xs"))
                                    .text_color(t.color("--text-subtle"))
                                    .child("Canvas"),
                            )
                            .child(
                                self.compact_button(
                                    "canvas-width",
                                    format!("W  {}", self.doc.width),
                                    t,
                                )
                                .border_color(gpui::transparent_black())
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.begin_numeric(
                                            NumericTarget::CanvasWidth,
                                            f64::from(this.doc.width),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child("×")
                            .child(
                                self.compact_button(
                                    "canvas-height",
                                    format!("H  {}", self.doc.height),
                                    t,
                                )
                                .border_color(gpui::transparent_black())
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.begin_numeric(
                                            NumericTarget::CanvasHeight,
                                            f64::from(this.doc.height),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                self.compact_button("trim", "Trim edges", t)
                                    .border_color(gpui::transparent_black())
                                    .child(icon("trim").size(px(13.)).text_color(t.muted()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.checkpoint();
                                        trim_to_content(&mut this.doc, 0.);
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.compact_button(
                                    "background",
                                    match self.doc.background {
                                        None => "Background  ◫",
                                        Some(Color(r, g, b, _)) if r + g + b < 1. => {
                                            "Background  ■"
                                        }
                                        Some(_) => "Background  □",
                                    },
                                    t,
                                )
                                .border_color(gpui::transparent_black())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.checkpoint();
                                        this.doc.background = match this.doc.background {
                                            None => Some(Color(
                                                247. / 255.,
                                                247. / 255.,
                                                245. / 255.,
                                                1.,
                                            )),
                                            Some(Color(r, g, b, _)) if r + g + b > 1. => {
                                                Some(Color(0., 0., 0., 1.))
                                            }
                                            _ => None,
                                        };
                                        this.changed();
                                        this.rerender(cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(div().flex_1())
                    .when(self.numeric_input.read(cx).is_focused(window), |header| {
                        header
                            .child(
                                div()
                                    .w(px(72.))
                                    .rounded(metric("--r-sm"))
                                    .border_1()
                                    .border_color(t.border())
                                    .bg(t.canvas())
                                    .child(self.numeric_input.clone()),
                            )
                            .child(self.compact_button("apply-number", "Apply", t).on_click(
                                cx.listener(|this, _, window, cx| this.apply_numeric(window, cx)),
                            ))
                    })
                    .child(
                        self.icon_button("undo", "undo", t)
                            .on_click(cx.listener(|this, _, _, cx| this.undo(false, cx))),
                    )
                    .child(
                        self.icon_button("redo", "redo", t)
                            .on_click(cx.listener(|this, _, _, cx| this.undo(true, cx))),
                    )
                    .child(
                        self.icon_button("zoom-out", "minus", t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = (this.zoom / 1.25).max(0.05);
                                this.zoom_fit = false;
                                cx.notify();
                            })),
                    )
                    .child(format!("{:.0}%", self.zoom * 100.))
                    .child(self.icon_button("zoom-in", "plus", t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.zoom = (this.zoom * 1.25).min(8.);
                            this.zoom_fit = false;
                            cx.notify();
                        },
                    )))
                    .child(
                        self.icon_button("fit", "fit", t)
                            .when(self.zoom_fit, |button| {
                                button.bg(Hsla {
                                    a: 0.16,
                                    ..t.accent
                                })
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_fit = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.compact_button("add-image", "Add images", t)
                            .child(icon("image").size(px(14.)).text_color(t.muted()))
                            .on_click(cx.listener(Self::prompt_add_image)),
                    ),
            )
            .when(self.restored, |r| {
                r.child(
                    div()
                        .px(metric("--s-5"))
                        .py(metric("--s-3"))
                        .bg(Hsla {
                            a: 0.16,
                            ..t.accent
                        })
                        .child("Unsaved editing draft restored — export, save, or keep editing."),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_tool_rail(t, cx))
                    .child(
                        div()
                            .id("canvas-scroll")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_scroll()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p(metric("--s-6"))
                            .bg(t.color("--surface-sunken"))
                            .track_scroll(&self.canvas_scroll)
                            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                            .child(self.canvas_view(t, cx)),
                    )
                    .child(self.render_sidebar(t, cx)),
            )
            .when(self.tool == Tool::Crop && self.crop.is_some(), |r| {
                r.child(
                    div()
                        .absolute()
                        .bottom(px(76.))
                        .left(px(100.))
                        .flex()
                        .gap(metric("--s-3"))
                        .child(
                            button("apply-crop", "Apply crop", t)
                                .bg(t.accent)
                                .text_color(t.glass_text())
                                .on_click(cx.listener(|this, _, _, cx| this.apply_crop(cx))),
                        )
                        .child(button("cancel-crop", "Cancel", t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.crop = None;
                                this.gesture = None;
                                cx.notify();
                            },
                        ))),
                )
            })
            .child(self.footer(t, cx))
            .when_some(
                self.editing_text
                    .as_ref()
                    .and_then(|(index, _)| self.doc.layers.get(*index))
                    .cloned(),
                |root, layer| {
                    let bounds = *self.canvas_bounds.lock().expect("canvas bounds");
                    root.child(
                        div()
                            .absolute()
                            .left(bounds.origin.x + px((layer.frame.x * self.zoom) as f32))
                            .top(bounds.origin.y + px((layer.frame.y * self.zoom) as f32))
                            .w(px((layer.frame.w * self.zoom).max(120.) as f32))
                            .h(px((layer.frame.h * self.zoom).max(36.) as f32))
                            .border_2()
                            .border_color(t.accent)
                            .bg(Hsla {
                                h: 0.,
                                s: 0.,
                                l: 0.,
                                a: 0.72,
                            })
                            .text_size(px(
                                (layer.frame.h * 0.58 * self.zoom).clamp(12., 128.) as f32
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.text_input.read(cx).focus(window);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(self.text_input.clone()),
                    )
                },
            )
    }
}

fn bind_keys(cx: &mut App) {
    text_input::bind_keys(cx);
    let editor = Some("ScreenshotEditor && !EditorTextInput");
    let canvas = Some("ScreenshotEditor && !EditorTextInput && !StrokeSlider");
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{primary}-z"), Undo, editor),
        KeyBinding::new(&format!("{primary}-shift-z"), Redo, editor),
        KeyBinding::new(&format!("{primary}-y"), Redo, editor),
        KeyBinding::new("delete", DeleteLayer, editor),
        KeyBinding::new(&format!("{primary}-d"), DuplicateLayer, editor),
        KeyBinding::new(&format!("{primary}-c"), CopyLayer, editor),
        KeyBinding::new(&format!("{primary}-v"), PasteLayer, editor),
        KeyBinding::new(&format!("{primary}-s"), SaveSource, editor),
        KeyBinding::new("v", SelectTool, editor),
        KeyBinding::new("c", CropTool, editor),
        KeyBinding::new("t", TextTool, editor),
        KeyBinding::new("r", RectangleTool, editor),
        KeyBinding::new("o", EllipseTool, editor),
        KeyBinding::new("l", LineTool, editor),
        KeyBinding::new("a", ArrowTool, editor),
        KeyBinding::new("p", PenTool, editor),
        KeyBinding::new("b", RemoveBackgroundTool, editor),
        KeyBinding::new(&format!("{primary}-="), ZoomIn, Some("ScreenshotEditor")),
        KeyBinding::new(&format!("{primary}--"), ZoomOut, Some("ScreenshotEditor")),
        KeyBinding::new(
            &format!("{primary}-0"),
            ActualSize,
            Some("ScreenshotEditor"),
        ),
        KeyBinding::new("left", NudgeLeft, canvas),
        KeyBinding::new("right", NudgeRight, canvas),
        KeyBinding::new("up", NudgeUp, canvas),
        KeyBinding::new("down", NudgeDown, canvas),
        KeyBinding::new("shift-left", NudgeLeftLarge, canvas),
        KeyBinding::new("shift-right", NudgeRightLarge, canvas),
        KeyBinding::new("shift-up", NudgeUpLarge, canvas),
        KeyBinding::new("shift-down", NudgeDownLarge, canvas),
        KeyBinding::new("enter", CommitText, Some("EditorTextInput")),
        KeyBinding::new("escape", CancelText, Some("EditorTextInput")),
    ]);
}

pub fn open(path: PathBuf, cx: &mut App) -> Result<()> {
    bind_keys(cx);
    let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
    let task = cx.background_executor().spawn({
        let path = path.clone();
        async move { load_source(&path) }
    });
    cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Captures GPUI Image".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            let editor = cx.new(|cx| {
                let mut editor = ScreenshotEditor::new(RgbaImage::new(1, 1), Some(path), focus, cx)
                    .expect("initialize screenshot editor");
                editor.source_ready = false;
                editor.status = "Loading source image…".into();
                editor
            });
            let result_editor = editor.clone();
            cx.spawn(async move |cx| {
                let result = task.await;
                cx.update(|cx| {
                    result_editor.update(cx, |editor, cx| {
                        match result {
                            Ok((doc, draft, restored, rendered)) => {
                                editor.source_ready = true;
                                editor.output_width = doc.width;
                                editor.output_height = doc.height;
                                editor.doc = doc;
                                editor.draft = draft;
                                editor.restored = restored;
                                editor.dirty = restored;
                                editor.rendered = rendered;
                                editor.numeric_target = NumericTarget::CanvasWidth;
                                editor.numeric_input.update(cx, |input, cx| {
                                    input.set(editor.doc.width.to_string(), cx)
                                });
                                editor.status = if restored {
                                    "Unsaved editing draft restored".into()
                                } else {
                                    "Ready".into()
                                };
                            }
                            Err(error) => {
                                editor.status = format!("Could not open image: {error}").into()
                            }
                        }
                        cx.notify();
                    })
                })
                .ok();
            })
            .detach();
            editor
        },
    )?;
    cx.activate(true);
    Ok(())
}

pub fn canvas(width: u32, height: u32, cx: &mut App) -> Result<()> {
    if width == 0 || height == 0 {
        anyhow::bail!("canvas dimensions must be nonzero");
    }
    open_impl(RgbaImage::new(width, height), None, cx)
}

fn open_impl(image: RgbaImage, source: Option<PathBuf>, cx: &mut App) -> Result<()> {
    bind_keys(cx);
    let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
    cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Captures GPUI Image".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            cx.new(|cx| {
                ScreenshotEditor::new(image, source, focus, cx)
                    .expect("initialize screenshot editor")
            })
        },
    )?;
    cx.activate(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_formats_are_real_and_decodable() {
        let mut doc = Document::transparent(19, 11);
        let i = doc.add(
            LayerKind::Rectangle,
            Rect {
                x: 2.,
                y: 1.,
                w: 12.,
                h: 7.,
            },
            Color(1., 0.1, 0.2, 1.),
            2.,
        );
        doc.layers[i].fill = Some(Color(0.2, 0.7, 0.3, 0.8));
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp] {
            let bytes = encode(&doc, format, QualityMode::Compress, 77, 1_000_000).unwrap();
            let image = image::load_from_memory(&bytes).unwrap();
            assert_eq!((image.width(), image.height()), (19, 11));
            assert!(bytes.len() > 20);
        }
    }

    #[test]
    fn stroke_slider_uses_inset_track_rounding_and_clamps_drag_outside() {
        // Thumb centers travel from x=107 to x=297, not across the full 204px box.
        let bounds = Bounds::new(point(px(100.), px(35.)), size(px(204.), px(20.)));
        for (x, expected) in [
            (50., 2.),
            (107., 2.),
            (153., 11.),
            (155., 12.),
            (297., 40.),
            (350., 40.),
        ] {
            assert_eq!(stroke_at(px(x), bounds), expected);
        }
    }

    #[test]
    fn maximum_export_rejects_impossible_limit_instead_of_faking_success() {
        let image = RgbaImage::from_fn(128, 128, |x, y| {
            image::Rgba([(x * 31) as u8, (y * 29) as u8, ((x + y) * 17) as u8, 255])
        });
        let doc = Document::new(image);
        assert!(encode(&doc, ExportFormat::Jpeg, QualityMode::Maximum, 92, 10).is_err());
    }

    #[test]
    fn exact_output_dimensions_are_encoded_instead_of_only_using_presets() {
        let doc = Document::new(RgbaImage::from_fn(13, 7, |x, y| {
            image::Rgba([(x * 17) as u8, (y * 29) as u8, 90, 255])
        }));
        let bytes = encode_scaled(
            &doc,
            ExportFormat::Png,
            QualityMode::Preserve,
            100,
            u64::MAX,
            31,
            19,
        )
        .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (31, 19));
    }

    #[test]
    fn draft_identity_separates_same_size_different_sources() {
        let image = RgbaImage::new(2, 3);
        assert_ne!(
            draft_path(Some(Path::new("a.png")), &image),
            draft_path(Some(Path::new("b.png")), &image)
        );
    }

    #[test]
    fn hex_colors_accept_rgb_and_alpha_but_reject_ambiguous_lengths() {
        assert_eq!(
            parse_hex_color("#ff8040"),
            Some(Color(1., 128. / 255., 64. / 255., 1.))
        );
        assert_eq!(parse_hex_color("10203080").unwrap().3, 128. / 255.);
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("not-a-color"), None);
        assert_eq!(parse_hex_color("aéabc"), None);
    }

    #[test]
    fn source_format_recognizes_jpeg_aliases_but_requires_copy_for_unknown_formats() {
        assert_eq!(
            ExportFormat::for_path(Path::new("photo.JPEG")),
            Some(ExportFormat::Jpeg)
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("photo.jpg")),
            Some(ExportFormat::Jpeg)
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("photo.webp")),
            Some(ExportFormat::Webp)
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("photo.png")),
            Some(ExportFormat::Png)
        );
        assert_eq!(ExportFormat::for_path(Path::new("photo.gif")), None);
        assert_eq!(ExportFormat::for_path(Path::new("photo")), None);
    }

    #[test]
    fn custom_picker_hsv_round_trip_preserves_asymmetric_rgba() {
        let expected = Color(242. / 255., 61. / 255., 79. / 255., 0.37);
        let (hue, saturation, value, alpha) = color_hsv(expected);
        let actual = hsv_color(hue, saturation, value, alpha);
        assert!((actual.0 - expected.0).abs() < 1e-6);
        assert!((actual.1 - expected.1).abs() < 1e-6);
        assert!((actual.2 - expected.2).abs() < 1e-6);
        assert!((actual.3 - expected.3).abs() < 1e-6);
    }
}
