//! Screenshot editor GPUI surface.
//!
//! The chrome deliberately follows the shared React editor rather than the
//! Windows experiment.  Pixel/document behavior is delegated to the portable,
//! tested `captures-windows-native` model.

mod fonts;

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

#[cfg(test)]
const ACCENT: [u8; 4] = [255, 202, 40, 255];
// ScreenshotEditor.tsx's annotation palette (independent of the UI accent).
const COLOR_SWATCHES: [u32; 8] = [
    0xff3b5c, 0xff8a22, 0xffd22e, 0x36c96b, 0x2d9cff, 0x8b5cf6, 0x111318, 0xffffff,
];

#[derive(Clone, Copy, PartialEq)]
enum EditorSlider {
    Zoom,
    Stroke,
    Opacity,
    LayerOpacity,
}

#[derive(Clone, Copy)]
enum ImageNumber {
    Width,
    Height,
    X,
    Y,
}

enum TextFontChange {
    Family(&'static str),
    Bold,
    Italic,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    Shapes,
    Undo,
    Redo,
    Fit,
    Plus,
    Minus,
    Image,
    Eye,
    Lock,
    Unlock,
    More,
    Copy,
    Save,
    AlignLeft,
    AlignCenter,
    AlignRight,
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    FlipVertical,
    BringFront,
    SendBack,
    MergeDown,
    MergeVisible,
    Flatten,
    Duplicate,
    Trash,
}

fn editor_icon(icon: EditorIcon, color: &'static str) -> Img {
    thread_local! {
        static ICONS: std::cell::RefCell<std::collections::HashMap<(EditorIcon, &'static str), Arc<RenderImage>>> = Default::default();
    }
    if let Some(image) = ICONS.with(|cache| cache.borrow().get(&(icon, color)).cloned()) {
        return img(image).size(px(18.));
    }
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
        EditorIcon::Shapes => {
            r#"<rect x="3" y="9" width="12" height="12" rx="2"/><circle cx="15" cy="9" r="6"/>"#
        }
        EditorIcon::Undo => r#"<path d="m9 5-5 5 5 5M4 10h10a5 5 0 0 1 0 10"/>"#,
        EditorIcon::Redo => r#"<path d="m15 5 5 5-5 5m5-5H10a5 5 0 0 0 0 10"/>"#,
        EditorIcon::Fit => r#"<path d="M9 4H4v5m11-5h5v5M4 15v5h5m11-5v5h-5"/>"#,
        EditorIcon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
        EditorIcon::Minus => r#"<path d="M5 12h14"/>"#,
        EditorIcon::Image => {
            r#"<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8" cy="8" r="1.5"/><path d="m3 16 5-5 4 4 3-3 6 6"/>"#
        }
        EditorIcon::Eye => {
            r#"<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>"#
        }
        EditorIcon::Lock => {
            r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V6a4 4 0 0 1 8 0v4"/>"#
        }
        EditorIcon::Unlock => {
            r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V6a4 4 0 0 1 8 0"/>"#
        }
        EditorIcon::More => r#"<path d="M12 5h.01M12 12h.01M12 19h.01" stroke-width="3"/>"#,
        EditorIcon::Copy => {
            r#"<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M15 8V3H3v13h5"/>"#
        }
        EditorIcon::Save => r#"<path d="M4 3h13l4 4v14H3V3Z"/><path d="M7 3v6h9V3M7 21v-8h10v8"/>"#,
        EditorIcon::AlignLeft => r#"<path d="M4 5h16M4 10h10M4 15h16M4 20h10"/>"#,
        EditorIcon::AlignCenter => r#"<path d="M4 5h16M7 10h10M4 15h16M7 20h10"/>"#,
        EditorIcon::AlignRight => r#"<path d="M4 5h16M10 10h10M4 15h16M10 20h10"/>"#,
        EditorIcon::RotateLeft => r#"<path d="M3 10a9 9 0 1 1 2 8M3 4v6h6"/>"#,
        EditorIcon::RotateRight => r#"<path d="M21 10a9 9 0 1 0-2 8m2-14v6h-6"/>"#,
        EditorIcon::FlipHorizontal => {
            r#"<path d="M12 3v3m0 3v3m0 3v3m0 3v1M3 7l6 5-6 5Zm18 0-6 5 6 5Z"/>"#
        }
        EditorIcon::FlipVertical => {
            r#"<path d="M3 12h3m3 0h3m3 0h3m3 0h1M7 3l5 6 5-6Zm0 18 5-6 5 6Z"/>"#
        }
        EditorIcon::BringFront => {
            r#"<rect x="5" y="12" width="10" height="8" rx="1.2" opacity=".55"/><rect x="9" y="4" width="10" height="8" rx="1.2"/>"#
        }
        EditorIcon::SendBack => {
            r#"<rect x="9" y="4" width="10" height="8" rx="1.2" opacity=".55"/><rect x="5" y="12" width="10" height="8" rx="1.2"/>"#
        }
        EditorIcon::MergeDown => r#"<path d="M7 4h10v4H7zM12 9v5m-3-2 3 3 3-3M5 17h14v3H5z"/>"#,
        EditorIcon::MergeVisible => {
            r#"<path d="M7 3h10v3H7zM7 8h10v3H7zM12 12v3m-3-1.5 3 3 3-3M5 18h14v3H5z"/>"#
        }
        EditorIcon::Flatten => {
            r#"<path d="M6 4h12v2.5H6zM6 8.5h12v2.5H6zM6 13h12v2.5H6zM4 18h16v2.5H4z"/>"#
        }
        EditorIcon::Duplicate => {
            r#"<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3M13.5 11v5M11 13.5h5"/>"#
        }
        EditorIcon::Trash => r#"<path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5M14 11v5"/>"#,
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    );
    let image = svg_render_image(svg);
    ICONS.with(|cache| cache.borrow_mut().insert((icon, color), image.clone()));
    img(image).size(px(18.))
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Webp,
}

#[derive(Clone, Copy, PartialEq)]
enum ExportQualityMode {
    Preserve,
    Compress,
    Maximum,
}

#[derive(Clone, Copy, PartialEq)]
enum FileSizeUnit {
    Kb,
    Mb,
    Gb,
}

#[derive(Clone, Copy)]
struct ExportSpec {
    format: Format,
    quality: u8,
    export_quality_mode: ExportQualityMode,
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
    filename_input: Option<Entity<crate::preferences::input::TextInput>>,
    synced_canvas_size: (u32, u32),
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
    image_width_input: Option<Entity<crate::preferences::input::TextInput>>,
    image_height_input: Option<Entity<crate::preferences::input::TextInput>>,
    image_x_input: Option<Entity<crate::preferences::input::TextInput>>,
    image_y_input: Option<Entity<crate::preferences::input::TextInput>>,
    background_color_input: Option<Entity<crate::preferences::input::TextInput>>,
    synced_text_layer: Option<(u64, u64)>,
    synced_image_layer: Option<(u64, u64)>,
    text_menu: Option<&'static str>,
    text_menu_bounds: Bounds<Pixels>,
    text_menu_above: bool,
    text_font_family: &'static str,
    text_font: Option<Arc<[u8]>>,
    document: Document,
    identity: DraftIdentity,
    source_path: Option<PathBuf>,
    rendered: Arc<RenderImage>,
    source_thumbnail: Arc<RenderImage>,
    canvas_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    tool: Tool,
    crop_selection: Option<Rect>,
    crop_aspect: Option<f32>,
    crop_shift_aspect: Option<f32>,
    crop_aspect_open: bool,
    shapes_open: bool,
    zoom_open: bool,
    custom_style_open: bool,
    slider_drag: Option<EditorSlider>,
    background_open: bool,
    source_menu_open: bool,
    layer_menu: Option<u64>,
    layer_menu_origin: gpui::Point<Pixels>,
    format_open: bool,
    selected: Option<u64>,
    zoom: u16,
    fit: bool,
    format: Format,
    quality: u8,
    export_quality_mode: ExportQualityMode,
    export_open: bool,
    export_menu: Option<&'static str>,
    export_scale: u8,
    export_width: u32,
    export_height: u32,
    export_max_bytes: Option<u64>,
    export_aspect_locked: bool,
    export_size_unit: FileSizeUnit,
    destination: PathBuf,
    make_copy: bool,
    compression_preview: Option<CompressionPreview>,
    compression_preview_pending: bool,
    compression_preview_error: Option<String>,
    compression_preview_request: u64,
    compression_preview_revision: u64,
    document_revision: u64,
    compression_split: u8,
    compression_compare_dismissed: bool,
    compression_drag: bool,
    pan: gpui::Point<Pixels>,
    last_canvas_point: Option<Point>,
    space_down: bool,
    style_color: [u8; 4],
    style_fill: Option<[u8; 4]>,
    style_stroke: f32,
    style_opacity: u8,
    style_font_size: f32,
    default_text_style: captures_image::TextStyleSettings,
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
        let source_thumbnail = render_image(&image::imageops::thumbnail(&pixels, 84, 60));
        let store = DraftStore::new(&launch.profile.join("drafts"));
        let mut document = store
            .load(&identity, source_path.as_deref())
            .map_err(anyhow::Error::msg)?
            .unwrap_or_else(|| Document::new(pixels));
        document.materialize_source(true);
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
            filename_input: None,
            synced_canvas_size: (export_width, export_height),
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
            image_width_input: None,
            image_height_input: None,
            image_x_input: None,
            image_y_input: None,
            background_color_input: None,
            synced_text_layer: None,
            synced_image_layer: None,
            text_menu: None,
            text_menu_bounds: Bounds::default(),
            text_menu_above: false,
            text_font_family: "rounded",
            text_font: None,
            launch,
            document,
            identity,
            source_path,
            rendered,
            source_thumbnail,
            canvas_bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            tool: Tool::Select,
            shapes_open: false,
            zoom_open: false,
            custom_style_open: false,
            slider_drag: None,
            background_open: false,
            source_menu_open: false,
            layer_menu: None,
            layer_menu_origin: point(px(8.), px(8.)),
            crop_selection: None,
            crop_aspect: None,
            crop_shift_aspect: None,
            crop_aspect_open: false,
            format_open: false,
            selected: None,
            zoom: 100,
            fit: true,
            format,
            quality: 100,
            export_quality_mode: ExportQualityMode::Preserve,
            export_open: false,
            export_menu: None,
            export_scale: 100,
            export_width,
            export_height,
            export_max_bytes: None,
            export_aspect_locked: true,
            export_size_unit: FileSizeUnit::Mb,
            destination,
            make_copy: true,
            compression_preview: None,
            compression_preview_pending: false,
            compression_preview_error: None,
            compression_preview_request: 0,
            compression_preview_revision: u64::MAX,
            document_revision: 0,
            compression_split: 50,
            compression_compare_dismissed: false,
            compression_drag: false,
            pan: point(px(0.), px(0.)),
            last_canvas_point: None,
            space_down: false,
            style_color: [255, 59, 92, 255],
            style_fill: None,
            style_stroke: 8.,
            style_opacity: 255,
            style_font_size: (export_width.min(export_height) as f32 * 0.055)
                .round()
                .clamp(24., 72.),
            default_text_style: captures_image::TextStyleSettings {
                background: Some([17, 19, 24, 255]),
                rounded_background: true,
                ..Default::default()
            },
            status: "Ready".into(),
        })
    }

    fn refresh(&mut self) {
        self.document.materialize_source(true);
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
        if tool == Tool::Text
            && !self
                .selected
                .and_then(|id| self.document.layers.iter().find(|l| l.id == id))
                .is_some_and(|l| matches!(l.shape, Shape::Text { .. }))
        {
            self.selected = None;
        }
        self.tool = tool;
        cx.notify();
    }

    fn document_point(&self, p: gpui::Point<Pixels>) -> Option<Point> {
        let b = self.canvas_bounds.get();
        if !b.contains(&p) && self.tool != Tool::Crop {
            return None;
        }
        Some(Point {
            x: ((p.x - b.origin.x) / b.size.width).clamp(0., 1.)
                * self.document.canvas_width as f32
                + self.document.crop.x,
            y: ((p.y - b.origin.y) / b.size.height).clamp(0., 1.)
                * self.document.canvas_height as f32
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
        if self.comparison_visible() {
            let bounds = self.canvas_bounds.get();
            if comparison_hide_bounds(bounds).contains(&ev.position) {
                self.compression_compare_dismissed = true;
                cx.notify();
                return;
            }
            let divider =
                bounds.left() + bounds.size.width * (f32::from(self.compression_split) / 100.);
            if (ev.position.x - divider).abs() <= px(20.)
                && (ev.position.y - bounds.center().y).abs() <= px(20.)
            {
                self.compression_drag = true;
                self.set_compression_split(ev.position.x);
                cx.notify();
                return;
            }
        }
        let Some(p) = self.document_point(ev.position) else {
            return;
        };
        if self.tool == Tool::Crop {
            self.crop_selection = None;
            self.crop_shift_aspect = None;
        }
        if self.tool == Tool::Text {
            let result = (|| -> anyhow::Result<()> {
                if self.text_font.is_none() {
                    let (bytes, font) = fonts::resolve(
                        self.text_font_family,
                        self.default_text_style.bold,
                        self.default_text_style.italic,
                    )?;
                    self.default_text_style.font = Some(font);
                    self.text_font = Some(bytes);
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
                            style: self.default_text_style.clone(),
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
        if self.compression_drag {
            self.set_compression_split(ev.position.x);
            cx.notify();
            return;
        }
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
                if self.tool == Tool::Crop {
                    if !ev.modifiers.shift {
                        self.crop_shift_aspect = None;
                    } else if self.crop_shift_aspect.is_none() {
                        self.crop_shift_aspect = Some(
                            self.crop_selection
                                .filter(|r| r.width >= 8. && r.height >= 8.)
                                .map_or(1., |r| r.width / r.height),
                        );
                    }
                    self.crop_selection = Some(bounded_crop(
                        *start,
                        p,
                        self.document.crop,
                        self.crop_aspect.or(self.crop_shift_aspect),
                    ));
                    cx.notify();
                    return;
                }
                if let Some(shape) =
                    gesture_shape(self.tool, *start, p, points.clone(), ev.modifiers.shift)
                {
                    let mut preview = self.document.clone();
                    let id = preview.add(shape, self.style_color, self.style_stroke);
                    preview.set_layer_opacity(id, self.style_opacity);
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
        if self.compression_drag {
            self.compression_drag = false;
            cx.notify();
            return;
        }
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
                        self.crop_selection = Some(bounded_crop(
                            start,
                            end,
                            self.document.crop,
                            self.crop_aspect.or(self.crop_shift_aspect),
                        ));
                        cx.notify();
                        return;
                    } else if let Some(shape) =
                        gesture_shape(self.tool, start, end, points, ev.modifiers.shift)
                    {
                        let id = self
                            .document
                            .add(shape, self.style_color, self.style_stroke);
                        self.document.set_layer_opacity(id, self.style_opacity);
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

    fn comparison_visible(&self) -> bool {
        self.export_open
            && self.export_quality_mode != ExportQualityMode::Preserve
            && !self.compression_compare_dismissed
            && self.compression_preview.is_some()
            && self.drag.is_none()
    }

    fn set_compression_split(&mut self, pointer_x: Pixels) {
        let bounds = self.canvas_bounds.get();
        let fraction = ((pointer_x - bounds.left()) / bounds.size.width).clamp(0.06, 0.94);
        self.compression_split = (fraction * 100.).round() as u8;
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
        if event.keystroke.key == "escape" && self.export_menu.take().is_some() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
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
                self.shapes_open = false;
                self.source_menu_open = false;
                self.background_open = false;
                self.format_open = false;
                self.zoom_open = false;
                self.export_open = false;
                self.text_menu = None;
                self.crop_selection = None;
                self.crop_shift_aspect = None;
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
        let height = parse_dimension("Export height", input_value(&self.export_height_input, cx))?;
        let quality = if self.export_quality_mode == ExportQualityMode::Preserve {
            100
        } else {
            parse_bounded_u8("Quality", input_value(&self.quality_input, cx), 1, 100)?
        };
        let max_bytes = if self.export_quality_mode == ExportQualityMode::Maximum {
            parse_maximum_size(input_value(&self.max_size_input, cx), self.export_size_unit)?
        } else {
            None
        };
        self.export_width = width;
        self.export_height = height;
        self.quality = quality;
        self.export_max_bytes = max_bytes;
        if self.export_aspect_locked {
            replace_input(&mut self.export_height_input, height.to_string(), cx);
        }
        Ok(())
    }

    fn choose_export_scale(&mut self, scale: u8, cx: &mut Context<Self>) {
        self.export_scale = scale;
        (self.export_width, self.export_height) = preset_output_size(
            (self.document.canvas_width, self.document.canvas_height),
            if scale == 0 { 100 } else { scale },
        );
        replace_input(
            &mut self.export_width_input,
            self.export_width.to_string(),
            cx,
        );
        replace_input(
            &mut self.export_height_input,
            self.export_height.to_string(),
            cx,
        );
        self.invalidate_compression_preview();
    }

    fn change_custom_export_dimension(&mut self, width_changed: bool, cx: &mut Context<Self>) {
        let result = (|| {
            if self.export_aspect_locked {
                let (input, source, other) = if width_changed {
                    (
                        &self.export_width_input,
                        self.document.canvas_width,
                        self.document.canvas_height,
                    )
                } else {
                    (
                        &self.export_height_input,
                        self.document.canvas_height,
                        self.document.canvas_width,
                    )
                };
                let value = parse_dimension("Output dimension", input_value(input, cx))?;
                let linked = proportional_height(value, source, other);
                replace_input(
                    if width_changed {
                        &mut self.export_height_input
                    } else {
                        &mut self.export_width_input
                    },
                    linked.to_string(),
                    cx,
                );
            }
            self.apply_export_fields(cx)
        })();
        if result.is_ok() {
            self.invalidate_compression_preview();
        }
        cx.notify();
    }

    fn nudge_custom_export_dimension(
        &mut self,
        width_changed: bool,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let input = if width_changed {
            &self.export_width_input
        } else {
            &self.export_height_input
        };
        let fallback = if width_changed {
            self.export_width
        } else {
            self.export_height
        };
        let current = input_value(input, cx)
            .trim()
            .parse::<u32>()
            .unwrap_or(fallback);
        let next = current.saturating_add_signed(delta).clamp(1, 16_384);
        replace_input(
            if width_changed {
                &mut self.export_width_input
            } else {
                &mut self.export_height_input
            },
            next.to_string(),
            cx,
        );
        self.change_custom_export_dimension(width_changed, cx);
    }

    fn export_select(
        &self,
        id: &'static str,
        current: &'static str,
        options: &[(&'static str, &'static str)],
        cx: &Context<Self>,
    ) -> Div {
        let t = self.theme;
        let label = options
            .iter()
            .find(|(v, _)| *v == current)
            .map_or(current, |(_, label)| *label);
        div()
            .relative()
            .child(
                self.button(id, "", false)
                    .min_w(px(if id == "size-unit" { 70. } else { 130. }))
                    .when(id == "blend-mode", |d| d.w_full())
                    .justify_between()
                    .gap_3()
                    .child(label)
                    .child("⌄")
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.export_menu = if s.export_menu == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    })),
            )
            .when(self.export_menu == Some(id), |d| {
                d.child(
                    deferred(
                        div()
                            .id(SharedString::from(format!("{id}-options")))
                            .occlude()
                            .absolute()
                            .when(id == "blend-mode", |d| d.top(px(36.)))
                            .when(id != "blend-mode", |d| d.bottom(px(36.)))
                            .left_0()
                            .min_w(px(180.))
                            .p_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .shadow_lg()
                            .on_mouse_down_out(cx.listener(move |s, _, _, cx| {
                                if s.export_menu == Some(id) {
                                    s.export_menu = None;
                                    cx.notify();
                                }
                            }))
                            .children(options.iter().map(|&(value, label)| {
                                self.button(
                                    SharedString::from(format!("{id}-{value}")),
                                    label,
                                    false,
                                )
                                .w_full()
                                .justify_start()
                                .border_0()
                                .bg(if value == current { t.hover } else { t.raised })
                                .on_click(cx.listener(
                                    move |s, _, _, cx| {
                                        match id {
                                            "output-size" => s.choose_export_scale(
                                                value.parse().unwrap_or(0),
                                                cx,
                                            ),
                                            "quality-mode" => s.choose_quality_mode(
                                                match value {
                                                    "compress" => ExportQualityMode::Compress,
                                                    "maximum" => ExportQualityMode::Maximum,
                                                    _ => ExportQualityMode::Preserve,
                                                },
                                                cx,
                                            ),
                                            "compression-quality" => {
                                                s.quality =
                                                    value.parse().expect("fixed quality preset");
                                                replace_input(
                                                    &mut s.quality_input,
                                                    value.into(),
                                                    cx,
                                                );
                                                s.invalidate_compression_preview();
                                            }
                                            "size-unit" => {
                                                let bytes = parse_maximum_size(
                                                    input_value(&s.max_size_input, cx),
                                                    s.export_size_unit,
                                                )
                                                .ok()
                                                .flatten();
                                                s.export_size_unit = match value {
                                                    "kb" => FileSizeUnit::Kb,
                                                    "gb" => FileSizeUnit::Gb,
                                                    _ => FileSizeUnit::Mb,
                                                };
                                                if let Some(bytes) = bytes {
                                                    let unit = match s.export_size_unit {
                                                        FileSizeUnit::Kb => 1_000.,
                                                        FileSizeUnit::Mb => 1_000_000.,
                                                        FileSizeUnit::Gb => 1_000_000_000.,
                                                    };
                                                    replace_input(
                                                        &mut s.max_size_input,
                                                        (bytes as f64 / unit).to_string(),
                                                        cx,
                                                    );
                                                }
                                                if s.apply_export_fields(cx).is_ok() {
                                                    s.invalidate_compression_preview();
                                                }
                                            }
                                            "blend-mode" => {
                                                if let Some(id) = s.layer_menu {
                                                    let mode = match value {
                                                        "Multiply" => BlendMode::Multiply,
                                                        "Screen" => BlendMode::Screen,
                                                        "Overlay" => BlendMode::Overlay,
                                                        "Darken" => BlendMode::Darken,
                                                        "Lighten" => BlendMode::Lighten,
                                                        _ => BlendMode::Normal,
                                                    };
                                                    if let Some(original) = s
                                                        .document
                                                        .layers
                                                        .iter()
                                                        .find(|l| l.id == id)
                                                        .cloned()
                                                    {
                                                        let mut layer = original.clone();
                                                        layer.blend_mode = mode;
                                                        s.document.preview_layer(layer);
                                                        s.document.commit_layer_preview(original);
                                                    }
                                                    s.refresh();
                                                }
                                            }
                                            _ => unreachable!("unknown screenshot select"),
                                        }
                                        s.export_menu = None;
                                        cx.notify();
                                    },
                                ))
                            })),
                    )
                    .with_priority(4),
                )
            })
    }

    fn choose_quality_mode(&mut self, mode: ExportQualityMode, cx: &mut Context<Self>) {
        self.export_quality_mode = mode;
        self.export_max_bytes = None;
        match mode {
            ExportQualityMode::Preserve => self.quality = 100,
            ExportQualityMode::Compress => {
                self.quality = input_value(&self.quality_input, cx).parse().unwrap_or(98);
                self.compression_compare_dismissed = false;
            }
            ExportQualityMode::Maximum => {
                self.compression_compare_dismissed = false;
                if input_value(&self.max_size_input, cx).trim().is_empty() {
                    replace_input(&mut self.max_size_input, "10".to_string(), cx);
                }
                self.export_max_bytes = parse_maximum_size(
                    input_value(&self.max_size_input, cx),
                    self.export_size_unit,
                )
                .ok()
                .flatten();
            }
        }
        self.invalidate_compression_preview();
    }

    fn apply_canvas_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let width = parse_dimension("Canvas width", input_value(&self.canvas_width_input, cx))?;
        let height = parse_dimension("Canvas height", input_value(&self.canvas_height_input, cx))?;
        self.document.set_canvas_size(width, height)?;
        self.refresh();
        Ok(())
    }

    fn update_image_number(
        &mut self,
        field: ImageNumber,
        requested: f32,
        typed: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let id = self
            .selected
            .ok_or_else(|| "No image layer selected".to_string())?;
        let layer = self
            .document
            .layers
            .iter()
            .find(|layer| layer.id == id)
            .ok_or_else(|| "No image layer selected".to_string())?;
        if layer.locked {
            return Err("Unlock this layer to change size and position.".into());
        }
        let Shape::Image {
            origin,
            width,
            height,
            ..
        } = &layer.shape
        else {
            return Err("Selected layer is not an image".into());
        };
        let (mut x, mut y, mut next_width, mut next_height) = (origin.x, origin.y, *width, *height);
        match field {
            ImageNumber::Width => {
                (next_width, next_height) =
                    proportional_image_size(*width, *height, requested, true)
            }
            ImageNumber::Height => {
                (next_width, next_height) =
                    proportional_image_size(*width, *height, requested, false)
            }
            ImageNumber::X => x = requested.clamp(-16_384., 16_384.),
            ImageNumber::Y => y = requested.clamp(-16_384., 16_384.),
        }
        self.edit_selected(
            |layer| {
                if let Shape::Image {
                    origin,
                    width,
                    height,
                    ..
                } = &mut layer.shape
                {
                    *origin = Point { x, y };
                    *width = next_width;
                    *height = next_height;
                }
            },
            cx,
        );
        replace_input(
            &mut self.image_width_input,
            next_width.round().to_string(),
            cx,
        );
        replace_input(
            &mut self.image_height_input,
            next_height.round().to_string(),
            cx,
        );
        replace_input(&mut self.image_x_input, x.round().to_string(), cx);
        replace_input(&mut self.image_y_input, y.round().to_string(), cx);
        if let Some(typed) = typed {
            let slot = match field {
                ImageNumber::Width => &mut self.image_width_input,
                ImageNumber::Height => &mut self.image_height_input,
                ImageNumber::X => &mut self.image_x_input,
                ImageNumber::Y => &mut self.image_y_input,
            };
            replace_input(slot, typed, cx);
        }
        self.synced_image_layer = Some((id, self.document_revision));
        Ok(())
    }

    fn nudge_image_number(&mut self, field: ImageNumber, direction: f32, cx: &mut Context<Self>) {
        let slot = match field {
            ImageNumber::Width => &self.image_width_input,
            ImageNumber::Height => &self.image_height_input,
            ImageNumber::X => &self.image_x_input,
            ImageNumber::Y => &self.image_y_input,
        };
        let current = input_value(slot, cx).trim().parse::<f32>().unwrap_or(0.);
        if let Err(error) = self.update_image_number(field, current + direction, None, cx) {
            self.status = error;
        }
        cx.notify();
    }

    fn apply_background_color(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let color = parse_color("Background", input_value(&self.background_color_input, cx))?;
        self.document.set_background(Some(color));
        self.background_open = false;
        self.refresh();
        cx.notify();
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
        if self
            .selected
            .and_then(|id| self.document.layers.iter().find(|l| l.id == id))
            .is_some_and(|l| l.locked)
        {
            return Err("Unlock this layer before editing it".into());
        }
        let value = input_value(&self.text, cx);
        let font_size = parse_f32(
            "Text size",
            input_value(&self.text_size_input, cx),
            8.,
            512.,
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
        if self.selected.is_some() {
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
                        update_text_effect_fields(
                            style,
                            width,
                            blur,
                            offset_x,
                            offset_y,
                            shadow_color,
                        );
                    }
                },
                cx,
            );
            // The fields already contain this edit. Do not reset the caret or
            // partially entered numbers when render synchronizes another layer.
            self.synced_text_layer = self.selected.map(|id| (id, self.document_revision));
        } else {
            update_text_effect_fields(
                &mut self.default_text_style,
                width,
                blur,
                offset_x,
                offset_y,
                shadow_color,
            );
            cx.notify();
        }
        Ok(())
    }

    /// Applies inspector changes to the selected text layer, or to the typed
    /// defaults used by the next placement when no text layer is selected.
    fn edit_text_settings(
        &mut self,
        edit: impl FnOnce(&mut captures_image::TextStyleSettings),
        cx: &mut Context<Self>,
    ) {
        match edit_text_style_state(
            &mut self.document,
            self.selected,
            &mut self.default_text_style,
            edit,
        ) {
            Ok(true) => self.refresh(),
            Ok(false) => {}
            Err(message) => self.status = message.into(),
        }
        cx.notify();
    }

    fn change_text_font(&mut self, change: TextFontChange, cx: &mut Context<Self>) {
        if self
            .selected
            .and_then(|id| self.document.layers.iter().find(|layer| layer.id == id))
            .is_some_and(|layer| layer.locked)
        {
            self.status = "Unlock this layer before editing it".into();
            cx.notify();
            return;
        }
        let mut style = self
            .selected
            .and_then(|id| self.document.layers.iter().find(|layer| layer.id == id))
            .and_then(|layer| match &layer.shape {
                Shape::Text { style, .. } => Some(style.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.default_text_style.clone());
        let family = match change {
            TextFontChange::Family(family) => family,
            TextFontChange::Bold => {
                style.bold = !style.bold;
                self.text_font_family
            }
            TextFontChange::Italic => {
                style.italic = !style.italic;
                self.text_font_family
            }
        };
        match fonts::resolve(family, style.bold, style.italic) {
            Ok((bytes, font)) => {
                style.font = Some(font);
                self.text_font = Some(bytes.clone());
                self.text_font_family = family;
                self.default_text_style = style.clone();
                if self.selected.is_some_and(|id| {
                    self.document
                        .layers
                        .iter()
                        .any(|layer| layer.id == id && matches!(layer.shape, Shape::Text { .. }))
                }) {
                    self.edit_selected(
                        |layer| {
                            if let Shape::Text {
                                font_data,
                                style: current,
                                ..
                            } = &mut layer.shape
                            {
                                *font_data = bytes;
                                *current = style;
                            }
                        },
                        cx,
                    );
                } else {
                    cx.notify();
                }
            }
            Err(error) => {
                self.status = format!("Could not load {family} font: {error}");
                cx.notify();
            }
        }
    }

    fn text_picker(
        &self,
        id: &'static str,
        style: &captures_image::TextStyleSettings,
        cx: &Context<Self>,
    ) -> Div {
        let t = self.theme;
        let options: &[(&str, &str)] = if id == "text-font" {
            &[
                ("system", "Sans serif"),
                ("serif", "Serif"),
                ("mono", "Monospace"),
                ("rounded", "Rounded"),
            ]
        } else {
            &[
                ("standard", "Standard"),
                ("rounded", "Rounded"),
                ("outlined", "Outlined"),
                ("mono", "Mono"),
                ("box", "Box"),
                ("mono-box", "Mono Box"),
                ("rounded-box", "Rounded Box"),
            ]
        };
        let selected = if id == "text-font" {
            self.text_font_family
        } else if style.outlined {
            "outlined"
        } else if style.background.is_some() {
            if style.rounded_background {
                "rounded-box"
            } else if self.text_font_family == "mono" {
                "mono-box"
            } else {
                "box"
            }
        } else {
            match self.text_font_family {
                "rounded" => "rounded",
                "mono" => "mono",
                _ => "standard",
            }
        };
        let label = options
            .iter()
            .find(|(value, _)| *value == selected)
            .map_or("Standard", |(_, label)| *label);
        let measured = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let recorded = measured.clone();
        let menu_height = px(options.len() as f32 * 38. + 10.);
        let sample = |value: &str| {
            div()
                .w(px(54.))
                .h(px(26.))
                .rounded(px(if value == "rounded-box" { 7. } else { 0. }))
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(if value.starts_with("mono") {
                    "monospace"
                } else {
                    font()
                })
                .bg(if value.ends_with("box") {
                    t.text
                } else {
                    rgba(0)
                })
                .text_color(if value.ends_with("box") {
                    t.raised
                } else {
                    t.text
                })
                .child("Text")
        };
        div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .child(
                div()
                    .id(id)
                    .relative()
                    .w_full()
                    .min_h(px(if id == "text-font" { 32. } else { 40. }))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(t.field)
                    .border_1()
                    .border_color(t.border)
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(id != "text-font", |d| d.child(sample(selected)))
                    .child(div().flex_1().child(label))
                    .child("⌄")
                    .child(
                        canvas(move |bounds, _, _| recorded.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .inset_0()
                            .size_full(),
                    )
                    .on_click(cx.listener(move |s, _, window, cx| {
                        s.text_menu_bounds = measured.get();
                        s.text_menu_above = s.text_menu_bounds.bottom() + menu_height + px(4.)
                            > window.viewport_size().height - px(8.);
                        s.text_menu = if s.text_menu == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    })),
            )
            .when(self.text_menu == Some(id), |d| {
                d.child(
                    deferred(
                        anchored()
                            .anchor(if self.text_menu_above {
                                Corner::BottomLeft
                            } else {
                                Corner::TopLeft
                            })
                            .position(point(
                                self.text_menu_bounds.left(),
                                if self.text_menu_above {
                                    self.text_menu_bounds.top() - px(4.)
                                } else {
                                    self.text_menu_bounds.bottom() + px(4.)
                                },
                            ))
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .occlude()
                                    .w(self.text_menu_bounds.size.width)
                                    .p_1()
                                    .bg(t.raised)
                                    .border_1()
                                    .border_color(t.border)
                                    .rounded(px(8.))
                                    .shadow_lg()
                                    .children(options.iter().map(|&(value, label)| {
                                        div()
                                            .id(SharedString::from(format!("{id}-{value}")))
                                            .h(px(38.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded(px(5.))
                                            .bg(if value == selected { t.hover } else { t.raised })
                                            .hover(|d| d.bg(t.hover))
                                            .cursor_pointer()
                                            .when(id != "text-font", |d| d.child(sample(value)))
                                            .child(label)
                                            .on_click(cx.listener(move |s, _, _, cx| {
                                                if id == "text-font" {
                                                    s.change_text_font(
                                                        TextFontChange::Family(value),
                                                        cx,
                                                    );
                                                } else {
                                                    s.edit_text_settings(
                                                        |style| {
                                                            style.background =
                                                                value.ends_with("box").then_some(
                                                                    style.background.unwrap_or([
                                                                        17, 19, 24, 255,
                                                                    ]),
                                                                );
                                                            style.rounded_background =
                                                                value == "rounded-box";
                                                            style.outlined = value == "outlined";
                                                        },
                                                        cx,
                                                    );
                                                    s.change_text_font(
                                                        TextFontChange::Family(
                                                            if value.starts_with("mono") {
                                                                "mono"
                                                            } else if value.starts_with("rounded") {
                                                                "rounded"
                                                            } else {
                                                                "system"
                                                            },
                                                        ),
                                                        cx,
                                                    );
                                                }
                                                s.text_menu = None;
                                                cx.notify();
                                            }))
                                    })),
                            ),
                    )
                    .with_priority(3),
                )
            })
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
            (Format::Png, None) => encode_png(
                &image,
                (self.export_quality_mode != ExportQualityMode::Preserve).then_some(self.quality),
            ),
            (Format::Jpeg, None) => encode_jpeg(&image, self.quality),
            (Format::Webp, None) => encode_webp(
                &image,
                (self.export_quality_mode != ExportQualityMode::Preserve).then_some(self.quality),
            ),
        };
        let bytes = match bytes {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Export failed: {e}");
                cx.notify();
                return;
            }
        };
        let name = input_value(&self.filename_input, cx);
        let path = match export_path(&self.destination, &name, self.format) {
            Ok(path) => path,
            Err(error) => {
                self.status = error.into();
                cx.notify();
                return;
            }
        };
        if self.make_copy
            && self
                .source_path
                .as_deref()
                .is_some_and(|source| paths_refer_to_same_file(source, &path))
        {
            self.status = "Choose a different filename to save as a new file".into();
            cx.notify();
            return;
        }
        self.status = match write_export(&path, &bytes, self.make_copy) {
            Ok(()) => {
                let saved = format!("Saved {} ({} KB)", path.display(), bytes.len() / 1024);
                match crate::preferences::history::record_export(
                    &self.launch.profile,
                    self.source_path.as_deref(),
                    &path,
                    self.make_copy,
                ) {
                    Ok(_) => saved,
                    Err(error) => format!("{saved}; history copy failed: {error}"),
                }
            }
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
            export_quality_mode: self.export_quality_mode,
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
        if layer == original {
            return;
        }
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
        let label = label.into();
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
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|button| button.bg(self.theme.hover))
            .when(!label.is_empty(), |button| button.child(label))
    }

    fn image_number_field(
        &self,
        label: &'static str,
        id: &'static str,
        input: Entity<crate::preferences::input::TextInput>,
        field: ImageNumber,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Div {
        let t = self.theme;
        field_container(label, t)
            .opacity(if disabled { 0.5 } else { 1. })
            .child(
                div()
                    .id(id)
                    .h(px(34.))
                    .flex()
                    .overflow_hidden()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.field)
                    .child(div().flex_1().min_w_0().child(input))
                    .when(!disabled, |control| {
                        control.child(
                            div()
                                .w(px(26.))
                                .flex_shrink_0()
                                .grid()
                                .grid_rows(2)
                                .border_l_1()
                                .border_color(t.border)
                                .bg(t.hover)
                                .child(
                                    div()
                                        .id(SharedString::from(format!("{id}-up")))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .child("⌃")
                                        .on_click(cx.listener(move |s, _, _, cx| {
                                            s.nudge_image_number(field, 1., cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("{id}-down")))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .border_t_1()
                                        .border_color(t.border)
                                        .child("⌄")
                                        .on_click(cx.listener(move |s, _, _, cx| {
                                            s.nudge_image_number(field, -1., cx)
                                        })),
                                ),
                        )
                    })
                    .on_key_down(cx.listener(move |s, event: &KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "up" => {
                                cx.stop_propagation();
                                s.nudge_image_number(field, 1., cx);
                            }
                            "down" => {
                                cx.stop_propagation();
                                s.nudge_image_number(field, -1., cx);
                            }
                            "enter" => {
                                cx.stop_propagation();
                                let slot = match field {
                                    ImageNumber::Width => &s.image_width_input,
                                    ImageNumber::Height => &s.image_height_input,
                                    ImageNumber::X => &s.image_x_input,
                                    ImageNumber::Y => &s.image_y_input,
                                };
                                let value = input_value(slot, cx).trim().parse::<f32>();
                                match value {
                                    Ok(value) if value.is_finite() => {
                                        if let Err(error) =
                                            s.update_image_number(field, value, None, cx)
                                        {
                                            s.status = error;
                                        }
                                    }
                                    _ => s.status = format!("{label} must be a number"),
                                }
                                cx.notify();
                            }
                            _ => {}
                        }
                    }))
                    .on_key_up(cx.listener(move |s, _, _, cx| {
                        let slot = match field {
                            ImageNumber::Width => &s.image_width_input,
                            ImageNumber::Height => &s.image_height_input,
                            ImageNumber::X => &s.image_x_input,
                            ImageNumber::Y => &s.image_y_input,
                        };
                        let typed = input_value(slot, cx);
                        if let Ok(value) = typed.trim().parse::<f32>()
                            && value.is_finite()
                        {
                            let _ = s.update_image_number(field, value, Some(typed), cx);
                        }
                    })),
            )
    }

    fn custom_export_number_field(
        &self,
        label: &'static str,
        id: &'static str,
        input: Entity<crate::preferences::input::TextInput>,
        width_changed: bool,
        cx: &Context<Self>,
    ) -> Div {
        let t = self.theme;
        field_container(label, t).w(px(100.)).child(
            div()
                .id(id)
                .h(px(34.))
                .flex()
                .overflow_hidden()
                .rounded(px(7.))
                .border_1()
                .border_color(t.border)
                .bg(t.field)
                .child(div().flex_1().min_w_0().child(input))
                .child(
                    div()
                        .w(px(26.))
                        .flex_shrink_0()
                        .grid()
                        .grid_rows(2)
                        .border_l_1()
                        .border_color(t.border)
                        .bg(t.hover)
                        .child(
                            div()
                                .id(SharedString::from(format!("{id}-up")))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .child("⌃")
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    cx.stop_propagation();
                                    s.nudge_custom_export_dimension(width_changed, 1, cx)
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("{id}-down")))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .border_t_1()
                                .border_color(t.border)
                                .child("⌄")
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    cx.stop_propagation();
                                    s.nudge_custom_export_dimension(width_changed, -1, cx)
                                })),
                        ),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(move |s, event: &KeyDownEvent, _, cx| {
                    match event.keystroke.key.as_str() {
                        "up" => {
                            cx.stop_propagation();
                            s.nudge_custom_export_dimension(width_changed, 1, cx);
                        }
                        "down" => {
                            cx.stop_propagation();
                            s.nudge_custom_export_dimension(width_changed, -1, cx);
                        }
                        _ => {}
                    }
                }))
                .on_key_up(cx.listener(move |s, _, _, cx| {
                    cx.stop_propagation();
                    s.change_custom_export_dimension(width_changed, cx);
                })),
        )
    }

    fn icon_button(&self, id: impl Into<ElementId>, icon: EditorIcon) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|button| button.bg(self.theme.hover))
            .child(
                editor_icon(
                    icon,
                    if self.theme.text == rgb(0x131318) {
                        "#5c5c69"
                    } else {
                        "#b9b9c4"
                    },
                )
                .size(px(14.)),
            )
    }

    fn prompt_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import images".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |editor, cx| editor.import_images(paths, cx));
            }
        })
        .detach();
    }

    fn slider(&self, id: &'static str, control: EditorSlider, cx: &Context<Self>) -> Stateful<Div> {
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let recorded = bounds.clone();
        let pressed = bounds.clone();
        let t = self.theme;
        let ratio = match control {
            EditorSlider::Zoom => (f32::from(self.zoom) - 10.) / 190.,
            EditorSlider::Stroke => (self.style_stroke - 1.) / 47.,
            EditorSlider::Opacity => f32::from(self.style_opacity) / 255.,
            EditorSlider::LayerOpacity => self
                .selected
                .and_then(|id| self.document.layers.iter().find(|l| l.id == id))
                .map_or(1., |l| f32::from(l.opacity) / 255.),
        }
        .clamp(0., 1.);
        div()
            .id(id)
            .w_full()
            .h(px(24.))
            .cursor_pointer()
            .child(
                canvas(
                    move |area, _, _| recorded.set(area),
                    move |area, _, window, _| {
                        let left = area.left() + px(5.);
                        let width = area.size.width - px(10.);
                        let y = area.center().y;
                        window.paint_quad(fill(
                            Bounds::new(point(left, y - px(1.5)), size(width, px(3.))),
                            t.border,
                        ));
                        if control != EditorSlider::Zoom {
                            window.paint_quad(fill(
                                Bounds::new(point(left, y - px(1.5)), size(width * ratio, px(3.))),
                                t.accent,
                            ));
                        }
                        window.paint_quad(quad(
                            Bounds::new(
                                point(left + width * ratio - px(5.), y - px(5.)),
                                size(px(10.), px(10.)),
                            ),
                            Corners::all(px(5.)),
                            t.field,
                            Edges::all(px(1.)),
                            t.border,
                            BorderStyle::Solid,
                        ));
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, event: &MouseDownEvent, _, cx| {
                    s.slider_drag = Some(control);
                    s.change_slider(control, event.position.x, pressed.get(), cx);
                }),
            )
            .on_mouse_move(cx.listener(move |s, event: &MouseMoveEvent, _, cx| {
                if s.slider_drag == Some(control) {
                    s.change_slider(control, event.position.x, bounds.get(), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.slider_drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.slider_drag = None),
            )
    }

    fn change_slider(
        &mut self,
        control: EditorSlider,
        x: Pixels,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let ratio = ((x - bounds.left() - px(5.)) / (bounds.size.width - px(10.)).max(px(1.)))
            .clamp(0., 1.);
        match control {
            EditorSlider::Zoom => {
                self.zoom = (10. + ratio * 190.).round() as u16;
                self.fit = false;
            }
            EditorSlider::Stroke => {
                self.style_stroke = (1. + ratio * 47.).round();
                self.stroke_input = None;
                if let Some(id) = self.selected {
                    self.document.set_layer_stroke(id, self.style_stroke);
                    self.refresh();
                }
            }
            EditorSlider::Opacity | EditorSlider::LayerOpacity => {
                self.style_opacity = (ratio * 255.).round() as u8;
                if let Some(id) = self.selected {
                    if control == EditorSlider::LayerOpacity {
                        if let Some(original) =
                            self.document.layers.iter().find(|l| l.id == id).cloned()
                        {
                            let mut layer = original.clone();
                            layer.opacity = self.style_opacity;
                            self.document.preview_layer(layer);
                            self.document.commit_layer_preview(original);
                        }
                    } else {
                        self.document.set_layer_opacity(id, self.style_opacity);
                    }
                    self.refresh();
                }
            }
        }
        cx.notify();
    }

    fn layer_settings(&self, id: u64, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Div {
        let Some((index, layer)) = self
            .document
            .layers
            .iter()
            .enumerate()
            .find(|(_, l)| l.id == id)
        else {
            return div();
        };
        let t = self.theme;
        let locked = layer.locked;
        let image = matches!(layer.shape, Shape::Image { .. });
        let mode = match layer.blend_mode {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
        };
        let section = |title| {
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .p(px(12.))
                .gap(px(8.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .text_size(px(10.))
                        .line_height(relative(1.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.subtle)
                        .child(title),
                )
        };
        let action = |label: &'static str, enabled: bool| {
            let icon = match label {
                "Bring to front" => EditorIcon::BringFront,
                "Send to back" => EditorIcon::SendBack,
                "Merge down" => EditorIcon::MergeDown,
                "Merge visible" => EditorIcon::MergeVisible,
                "Flatten image" => EditorIcon::Flatten,
                "Duplicate" => EditorIcon::Duplicate,
                _ => EditorIcon::Trash,
            };
            self.button(label, "", false)
                .w_full()
                .h(px(34.))
                .justify_start()
                .gap(px(10.))
                .border_0()
                .bg(t.raised)
                .child(
                    editor_icon(
                        icon,
                        if label == "Delete" {
                            "#d13450"
                        } else if t.text == rgb(0x131318) {
                            "#65656f"
                        } else {
                            "#b8b8c0"
                        },
                    )
                    .size(px(15.)),
                )
                .child(label)
                .when(!enabled, |d| d.opacity(0.4).cursor_default())
                .on_click(cx.listener(move |s, _, _, cx| {
                    if !enabled {
                        return;
                    }
                    match label {
                        "Bring to front" => {
                            s.document.move_layer(id, isize::MAX);
                        }
                        "Send to back" => {
                            s.document.move_layer(id, isize::MIN);
                        }
                        "Merge down" => match s.document.merge_layer_down(id) {
                            Ok(next) => {
                                s.selected = next;
                                s.layer_menu = next;
                            }
                            Err(e) => s.status = e,
                        },
                        "Merge visible" => match s.document.merge_visible_layers() {
                            Ok(next) => {
                                s.selected = next;
                                s.layer_menu = next;
                            }
                            Err(e) => s.status = e,
                        },
                        "Flatten image" => {
                            if let Err(e) = s.document.flatten_layers() {
                                s.status = e;
                            }
                            s.selected = None;
                            s.layer_menu = None;
                        }
                        "Duplicate" => {
                            s.selected = s.document.duplicate(id);
                            s.layer_menu = None;
                        }
                        "Delete" => {
                            if s.document.delete(id) {
                                s.selected = None;
                                s.layer_menu = None;
                            }
                        }
                        _ => unreachable!("fixed layer action"),
                    }
                    s.refresh();
                    cx.notify();
                }))
        };
        div()
            .absolute()
            .inset_0()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, _, _, cx| {
                    s.layer_menu = None;
                    s.export_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .id("layer-settings-panel")
                    .occlude()
                    .absolute()
                    .left(
                        self.layer_menu_origin
                            .x
                            .max(px(8.))
                            .min((viewport.width - px(288.)).max(px(8.))),
                    )
                    .top(
                        self.layer_menu_origin
                            .y
                            .max(px(8.))
                            .min((viewport.height - px(568.)).max(px(8.))),
                    )
                    .h((viewport.height
                        - self
                            .layer_menu_origin
                            .y
                            .max(px(8.))
                            .min((viewport.height - px(568.)).max(px(8.)))
                        - px(8.))
                    .min(px(640.)))
                    .w(px(280.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        section("APPEARANCE")
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child("Blend mode"),
                            )
                            .child(self.export_select(
                                "blend-mode",
                                mode,
                                &[
                                    ("Normal", "Normal"),
                                    ("Multiply", "Multiply"),
                                    ("Screen", "Screen"),
                                    ("Overlay", "Overlay"),
                                    ("Darken", "Darken"),
                                    ("Lighten", "Lighten"),
                                ],
                                cx,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child("Opacity")
                                    .child(format!(
                                        "{}%",
                                        (f32::from(layer.opacity) * 100. / 255.).round()
                                    )),
                            )
                            .child(self.slider("layer-opacity", EditorSlider::LayerOpacity, cx)),
                    )
                    .child(
                        div()
                            .id("layer-settings-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when(image, |d| {
                                d.child(
                                    section("TRANSFORM").child(
                                        div().grid().grid_cols(2).gap(px(8.)).children(
                                            [
                                                (
                                                    "Rotate left",
                                                    EditorIcon::RotateLeft,
                                                    -1,
                                                    false,
                                                    false,
                                                ),
                                                (
                                                    "Rotate right",
                                                    EditorIcon::RotateRight,
                                                    1,
                                                    false,
                                                    false,
                                                ),
                                                (
                                                    "Flip horizontal",
                                                    EditorIcon::FlipHorizontal,
                                                    0,
                                                    true,
                                                    false,
                                                ),
                                                (
                                                    "Flip vertical",
                                                    EditorIcon::FlipVertical,
                                                    0,
                                                    false,
                                                    true,
                                                ),
                                            ]
                                            .into_iter()
                                            .map(
                                                |(label, icon, turns, horizontal, vertical)| {
                                                    self.button(label, "", false)
                                                        .h(px(36.))
                                                        .px(px(8.))
                                                        .justify_start()
                                                        .gap(px(6.))
                                                        .text_size(px(11.))
                                                        .child(
                                                            editor_icon(
                                                                icon,
                                                                if t.text == rgb(0x131318) {
                                                                    "#65656f"
                                                                } else {
                                                                    "#b8b8c0"
                                                                },
                                                            )
                                                            .size(px(15.)),
                                                        )
                                                        .child(label)
                                                        .on_click(cx.listener(
                                                            move |s, _, _, cx| {
                                                                if s.document.transform_image(
                                                                    id, turns, horizontal, vertical,
                                                                ) {
                                                                    s.refresh();
                                                                }
                                                                cx.notify();
                                                            },
                                                        ))
                                                },
                                            ),
                                        ),
                                    ),
                                )
                            })
                            .child(
                                section("ARRANGE").child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(action(
                                            "Bring to front",
                                            !locked && index + 1 < self.document.layers.len(),
                                        ))
                                        .child(action("Send to back", !locked && index > 0)),
                                ),
                            )
                            .child(
                                section("COMBINE").child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(action(
                                            "Merge down",
                                            self.document.can_merge_layer_down(id),
                                        ))
                                        .child(action(
                                            "Merge visible",
                                            self.document.can_merge_visible_layers(),
                                        ))
                                        .child(action(
                                            "Flatten image",
                                            self.document.can_flatten_layers(),
                                        )),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .p(px(10.))
                            .gap(px(4.))
                            .border_t_1()
                            .border_color(t.border)
                            .bg(t.sunken)
                            .child(
                                action("Duplicate", true)
                                    .border_1()
                                    .border_color(t.border)
                                    .bg(t.field),
                            )
                            .child(
                                action("Delete", !locked)
                                    .border_1()
                                    .border_color(t.border)
                                    .text_color(t.signal)
                                    .bg(t.field),
                            ),
                    ),
            )
    }

    fn style_controls(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let color = color_hex(self.style_color);
        let body = match self.tool {
            Tool::Arrow => r#"<path d="M88 62 198 27m-18-4h22v22"/>"#,
            Tool::Line => r#"<path d="M88 62 202 26"/>"#,
            Tool::Ellipse => r#"<ellipse cx="148" cy="44" rx="57" ry="26"/>"#,
            Tool::Pen => r#"<path d="M84 60c28-52 35-38 49-14s25 21 77-26"/>"#,
            Tool::Triangle => r#"<path d="m148 15 55 58H93Z"/>"#,
            Tool::Diamond => r#"<path d="m148 12 57 32-57 32-57-32Z"/>"#,
            Tool::Star => {
                r#"<path d="m148 8 13 23 29 4-21 19 5 27-26-14-26 14 5-27-21-19 29-4Z"/>"#
            }
            _ => r#"<rect x="90" y="20" width="116" height="48" rx="3"/>"#,
        };
        let (checker_a, checker_b) = if t.text == rgb(0x131318) {
            ("#f5f5f7", "#e8e8ed")
        } else {
            ("#16161b", "#232329")
        };
        let preview = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="592" height="176" viewBox="0 0 296 88"><defs><pattern id="grid" width="16" height="16" patternUnits="userSpaceOnUse"><rect width="16" height="16" fill="{checker_a}"/><path d="M0 0h8v8H0Zm8 8h8v8H8Z" fill="{checker_b}"/></pattern></defs><rect width="296" height="88" fill="url(#grid)"/><g fill="none" stroke="{color}" stroke-width="{}" opacity="{}" stroke-linecap="round" stroke-linejoin="round">{body}</g></svg>"##,
            self.style_stroke * 0.5,
            f32::from(self.style_opacity) / 255.
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .pt_2()
            .child(
                img(svg_render_image(preview))
                    .w_full()
                    .h(px(88.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.border),
            )
            .child(div().text_size(px(12.)).text_color(t.muted).child("Color"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .children(
                        COLOR_SWATCHES
                            .into_iter()
                            .enumerate()
                            .map(|(index, color)| {
                                let bytes =
                                    [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
                                div()
                                    .id(("swatch", index))
                                    .w(relative(1. / 6.))
                                    .h(px(32.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .size(px(30.))
                                            .rounded_full()
                                            .border_2()
                                            .border_color(if self.style_color == bytes {
                                                t.accent
                                            } else {
                                                rgba(0)
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .size(px(24.))
                                                    .rounded_full()
                                                    .bg(rgb(color))
                                                    .border_1()
                                                    .border_color(t.border),
                                            ),
                                    )
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.style_color = bytes;
                                        s.stroke_color_input = None;
                                        if let Some(id) = s.selected {
                                            s.document.set_layer_color(id, bytes);
                                            s.refresh();
                                        }
                                        cx.notify();
                                    }))
                            }),
                    )
                    .child(
                        div()
                            .w(relative(1. / 6.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                self.button("custom-style", "+", false)
                                    .size(px(24.))
                                    .px_0()
                                    .on_click(cx.listener(|s, _, _, cx| {
                                        s.custom_style_open = !s.custom_style_open;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child("Size"),
            )
            .child(
                div()
                    .text_right()
                    .text_size(px(11.))
                    .text_color(t.subtle)
                    .child(format!("{} px", self.style_stroke)),
            )
            .child(self.slider("stroke-slider", EditorSlider::Stroke, cx))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child("Opacity"),
            )
            .child(
                div()
                    .text_right()
                    .text_size(px(11.))
                    .text_color(t.subtle)
                    .child(format!(
                        "{}%",
                        (f32::from(self.style_opacity) / 2.55).round()
                    )),
            )
            .child(self.slider("opacity-slider", EditorSlider::Opacity, cx))
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
            .hover(|button| {
                button.bg(if active {
                    self.theme.accent
                } else {
                    self.theme.hover
                })
            })
            .child(editor_icon(
                icon,
                if active {
                    "#17140a"
                } else if self.theme.text == rgb(0x131318) {
                    "#5c5c69"
                } else {
                    "#b9b9c4"
                },
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.shapes_open = false;
                this.choose_tool(tool, cx)
            }))
    }
}

impl Render for ScreenshotEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = Theme::for_window(&self.launch, window, cx);
        let canvas_size = (self.document.canvas_width, self.document.canvas_height);
        if self.synced_canvas_size != canvas_size {
            // Original / preset-size exports follow a crop or canvas resize.
            // A separately chosen output size remains an explicit override.
            let output = follow_canvas_export_size(
                self.synced_canvas_size,
                canvas_size,
                (self.export_width, self.export_height),
                self.export_scale,
            );
            if output != (self.export_width, self.export_height) {
                (self.export_width, self.export_height) = output;
                replace_input(
                    &mut self.export_width_input,
                    self.export_width.to_string(),
                    cx,
                );
                replace_input(
                    &mut self.export_height_input,
                    self.export_height.to_string(),
                    cx,
                );
            }
            for (field, value) in [
                (&self.canvas_width_input, canvas_size.0),
                (&self.canvas_height_input, canvas_size.1),
            ] {
                if let Some(input) = field {
                    input.update(cx, |input, cx| input.set_value(value.to_string(), cx));
                }
            }
            self.synced_canvas_size = canvas_size;
        }
        if self.export_open
            && !self.compression_preview_pending
            && self.compression_preview_revision != self.document_revision
        {
            self.start_compression_preview(cx);
        }
        let t = self.theme;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let compact = window.viewport_size().width <= px(1040.);
        let icon_color = if t.text == rgb(0x131318) {
            "#5c5c69"
        } else {
            "#b9b9c4"
        };
        let _text_input = self
            .text
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    crate::preferences::input::TextInput::new("Text", "Annotation text", cx)
                        .multiline(16_384)
                        .height(px(88.))
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
        let selected_image = self.selected.and_then(|id| {
            self.document
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(|layer| {
                    if let Shape::Image {
                        origin,
                        width,
                        height,
                        ..
                    } = &layer.shape
                    {
                        Some((origin.x, origin.y, *width, *height, layer.locked))
                    } else {
                        None
                    }
                })
        });
        if let Some((x, y, width, height, _)) = selected_image {
            let key = self.selected.map(|id| (id, self.document_revision));
            if self.synced_image_layer != key {
                replace_input(&mut self.image_x_input, x.round().to_string(), cx);
                replace_input(&mut self.image_y_input, y.round().to_string(), cx);
                replace_input(&mut self.image_width_input, width.round().to_string(), cx);
                replace_input(&mut self.image_height_input, height.round().to_string(), cx);
                self.synced_image_layer = key;
            }
        } else {
            self.synced_image_layer = None;
        }
        if let Some((id, value, font_size, style)) = &selected_text
            && self.synced_text_layer != Some((*id, self.document_revision))
        {
            if let Some(layer) = self.document.layers.iter().find(|layer| layer.id == *id) {
                replace_input(&mut self.stroke_color_input, color_hex(layer.color), cx);
                if let Shape::Text {
                    font_data, style, ..
                } = &layer.shape
                {
                    self.text_font = Some(font_data.clone());
                    self.default_text_style = style.clone();
                    self.text_font_family =
                        match style.font.as_ref().map(|font| font.family.as_str()) {
                            Some("serif") => "serif",
                            Some("mono") => "mono",
                            Some("rounded") => "rounded",
                            _ => "system",
                        };
                }
            }
            replace_input(
                &mut self.fill_color_input,
                style.background.map(color_hex).unwrap_or_default(),
                cx,
            );
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
        let _quality_input = ensure_input(&mut self.quality_input, 98, "1–100", cx);
        let max_size_input = ensure_input(&mut self.max_size_input, "", "Optional bytes (1MB)", cx);
        let filename_input = ensure_chrome_input(
            &mut self.filename_input,
            self.destination
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy(),
            "Filename",
            cx,
        );
        let canvas_width_input = ensure_chrome_input(
            &mut self.canvas_width_input,
            self.document.canvas_width,
            "Width",
            cx,
        );
        let canvas_height_input = ensure_chrome_input(
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
        let image_width_input = ensure_input(
            &mut self.image_width_input,
            selected_image.map_or(1., |v| v.2).round(),
            "Width",
            cx,
        );
        let image_height_input = ensure_input(
            &mut self.image_height_input,
            selected_image.map_or(1., |v| v.3).round(),
            "Height",
            cx,
        );
        let image_x_input = ensure_input(
            &mut self.image_x_input,
            selected_image.map_or(0., |v| v.0).round(),
            "X",
            cx,
        );
        let image_y_input = ensure_input(
            &mut self.image_y_input,
            selected_image.map_or(0., |v| v.1).round(),
            "Y",
            cx,
        );
        let image_locked = selected_image.is_some_and(|value| value.4);
        for input in [
            &image_width_input,
            &image_height_input,
            &image_x_input,
            &image_y_input,
        ] {
            input.update(cx, |input, cx| input.set_disabled(image_locked, cx));
        }
        let background_color_input = ensure_input(
            &mut self.background_color_input,
            self.document
                .background
                .map(color_hex)
                .unwrap_or_else(|| "#F7F7F5FF".into()),
            "#RRGGBB",
            cx,
        );
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
        let crop_selection = (self.tool == Tool::Crop)
            .then_some(self.crop_selection)
            .flatten();
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
        let shapes = [
            ("Rectangle", EditorIcon::Rectangle, Tool::Rectangle),
            ("Ellipse", EditorIcon::Ellipse, Tool::Ellipse),
            ("Line", EditorIcon::Line, Tool::Line),
            ("Triangle", EditorIcon::Triangle, Tool::Triangle),
            ("Diamond", EditorIcon::Diamond, Tool::Diamond),
            ("Star", EditorIcon::Star, Tool::Star),
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
                    .h(px(54.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if active { t.accent } else { rgba(0) })
                    .bg(if active { t.hover } else { t.raised })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id);
                        if let Some(layer) =
                            this.document.layers.iter().find(|layer| layer.id == id)
                        {
                            this.style_color = layer.color;
                            this.style_stroke = layer.stroke;
                            this.style_opacity = layer.opacity;
                            this.stroke_input = None;
                            this.stroke_color_input = None;
                        }
                        cx.notify()
                    }))
                    .child(div().text_color(t.subtle).child("⠿"))
                    .child(
                        div()
                            .w(px(42.))
                            .h(px(30.))
                            .flex_shrink_0()
                            .rounded(px(6.))
                            .bg(t.sunken)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(match &layer.shape {
                                Shape::Image { pixels, .. } => img(render_image(
                                    &image::imageops::thumbnail(pixels.as_ref(), 84, 60),
                                ))
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .into_any_element(),
                                shape => editor_icon(
                                    match shape {
                                        Shape::Arrow(..) => EditorIcon::Arrow,
                                        Shape::Text { .. } => EditorIcon::Text,
                                        Shape::Ellipse(_) => EditorIcon::Ellipse,
                                        Shape::Stroke(_) => EditorIcon::Pen,
                                        Shape::Line(..) => EditorIcon::Line,
                                        _ => EditorIcon::Rectangle,
                                    },
                                    "#5c5c69",
                                )
                                .into_any_element(),
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .truncate()
                            .child(layer.name.clone()),
                    )
                    .child(
                        self.icon_button(("layer-visible", id as usize), EditorIcon::Eye)
                            .w(px(24.))
                            .opacity(if layer.visible { 1. } else { 0.35 })
                            .on_click(cx.listener(move |s, _, _, cx| {
                                cx.stop_propagation();
                                s.document.toggle_visibility(id);
                                s.refresh();
                                cx.notify();
                            })),
                    )
                    .child(
                        self.icon_button(("layer-lock", id as usize), EditorIcon::Lock)
                            .w(px(24.))
                            .opacity(if layer.locked { 1. } else { 0.35 })
                            .on_click(cx.listener(move |s, _, _, cx| {
                                cx.stop_propagation();
                                s.document.toggle_locked(id);
                                s.refresh();
                                cx.notify();
                            })),
                    )
                    .child(
                        self.icon_button(("layer-menu", id as usize), EditorIcon::More)
                            .w(px(24.))
                            .on_click(cx.listener(move |s, event: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                s.selected = Some(id);
                                s.layer_menu_origin = event.position() - point(px(302.), px(12.));
                                s.layer_menu = if s.layer_menu == Some(id) {
                                    None
                                } else {
                                    Some(id)
                                };
                                cx.notify();
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let selected = self.selected;
        let compression_before = self.compression_preview.as_ref().map(|p| p.before.clone());
        let compression_after = self.compression_preview.as_ref().map(|p| p.after.clone());
        let comparison = self.comparison_visible().then(|| {
            (
                compression_before.clone().unwrap(),
                compression_after.clone().unwrap(),
                f32::from(self.compression_split) / 100.,
                self.compression_preview.as_ref().unwrap().before_bytes,
                self.compression_preview.as_ref().unwrap().after_bytes,
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
            .relative()
            .min_w(px(860.))
            .bg(t.canvas)
            .text_color(t.text)
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .flex_shrink_0()
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
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.sunken)
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(div().px_2().text_size(px(11.)).text_color(t.subtle).child("Canvas"))
                            .child("W")
                            .child(
                                div().w(px(64.)).text_color(t.text).child(canvas_width_input)
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                        if event.keystroke.key == "enter" {
                                            if let Err(error) = this.apply_canvas_fields(cx) { this.status = error; }
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child("× H")
                            .child(
                                div().w(px(64.)).text_color(t.text).child(canvas_height_input)
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                        if event.keystroke.key == "enter" {
                                            if let Err(error) = this.apply_canvas_fields(cx) { this.status = error; }
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(div().w(px(1.)).h(px(16.)).mx_1().bg(t.border))
                            .child(self.button("trim", "Trim edges", false).h(px(26.)).border_0().bg(t.sunken)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    match this.document.trim_to_visible_content() {
                                        Ok(_) => this.refresh(),
                                        Err(e) => this.status = e.into(),
                                    }
                                    cx.notify();
                                })))
                            .child(self.button("background", "Background color ⌄", false).h(px(26.)).border_0().bg(t.sunken)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.background_open = !this.background_open;
                                    cx.notify();
                                }))),
                    )
                    .child(div().flex_1())
                    .when(!compact, |row| row.child(
                        self.icon_button("undo", EditorIcon::Undo)
                            .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                    )
                    .child(
                        self.icon_button("redo", EditorIcon::Redo)
                            .on_click(cx.listener(|this, _, _, cx| this.redo(cx))),
                    ))
                    .child(div().h(px(34.)).flex().items_center().rounded(px(10.)).border_1().border_color(t.border).bg(t.sunken)
                    .child(self.icon_button("recenter", EditorIcon::Fit).when(self.fit, |button| button.bg(t.hover))
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.pan = point(px(0.), px(0.)); s.fit = true; cx.notify();
                        })))
                    .child(self.icon_button("zoom-out", EditorIcon::Minus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.fit = false;
                            this.zoom = this.zoom.saturating_sub(10).max(10);
                            cx.notify()
                        },
                    )))
                    .child(div().w(px(if compact { 72. } else { 92. })).px_2().child(self.slider("zoom-track", EditorSlider::Zoom, cx)))
                    .child(self.icon_button("zoom-in", EditorIcon::Plus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.fit = false;
                            this.zoom = (this.zoom + 10).min(400);
                            cx.notify()
                        },
                    )))
                    .child(self.button("zoom-preset", if self.fit { "Fit  ⌄".into() } else { format!("{}%  ⌄", self.zoom) }, false)
                            .w(px(76.)).border_0().bg(t.sunken)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.zoom_open = !s.zoom_open;
                                cx.notify()
                            }))))
                    .child(self.button("import", "", false).gap_2().h(px(34.))
                        .child(editor_icon(EditorIcon::Image, icon_color).size(px(14.)))
                        .child("Add images")
                        .on_click(cx.listener(|this, _, window, cx| this.prompt_images(window, cx))),
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
                            .flex_shrink_0()
                            .py_2()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .border_r_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .child(self.tool_button("Select", EditorIcon::Select, Tool::Select, cx))
                            .child(self.tool_button("Crop", EditorIcon::Crop, Tool::Crop, cx))
                            .child(self.tool_button("Text", EditorIcon::Text, Tool::Text, cx))
                            .child(div().id("shapes").size(px(38.)).relative().flex().items_center().justify_center().rounded(px(8.))
                                .bg(if shapes.iter().any(|(_, _, tool)| *tool == self.tool) { t.accent } else { rgba(0) })
                                .cursor_pointer().hover(|button| button.bg(t.hover))
                                .child(editor_icon(EditorIcon::Shapes, icon_color))
                                .child(div().absolute().right(px(2.)).bottom(px(0.)).text_size(px(8.)).text_color(t.subtle).child("◢"))
                                .on_click(cx.listener(|s, _, _, cx| { s.shapes_open = !s.shapes_open; cx.notify(); })))
                            .child(self.tool_button("Arrow", EditorIcon::Arrow, Tool::Arrow, cx))
                            .child(self.tool_button("Pen", EditorIcon::Pen, Tool::Pen, cx))
                            .child(self.tool_button("Eraser", EditorIcon::Eraser, Tool::Eraser, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .bg(t.editor_well)
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
                                                    ((available_w + 8.) / image_size.width)
                                                        .min((available_h + 8.) / image_size.height).min(1.)
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
                                                            + ((bounds.size.width - size.width) / 2.).max(px(0.))
                                                            + pan.x,
                                                        bounds.origin.y
                                                            + (bounds.size.height - size.height)
                                                                / 2.
                                                            + pan.y,
                                                    ),
                                                    size,
                                                });
                                            },
                                            move |_, _, window, cx| {
                                                let image = paint_bounds.get();
                                                window.paint_shadows(image, Corners::default(), &[BoxShadow {
                                                    color: rgba(0x1313181f).into(),
                                                    offset: point(px(0.), px(16.)),
                                                    blur_radius: px(40.),
                                                    spread_radius: px(0.),
                                                }]);
                                                window.paint_quad(quad(image, Corners::default(), rgb(0xf7f7f5), Edges::all(px(1.)), t.border, BorderStyle::Solid));
                                                let _ = window.paint_image(
                                                    paint_bounds.get(),
                                                    Corners::default(),
                                                    rendered.clone(),
                                                    0,
                                                    false,
                                                );
                                                let image = paint_bounds.get();
                                                if let Some(selection) = crop_selection {
                                                    let sx = image.size.width / image_size.width;
                                                    let sy = image.size.height / image_size.height;
                                                    let selected = Bounds::new(
                                                        point(image.origin.x + sx * (selection.x - crop.x), image.origin.y + sy * (selection.y - crop.y)),
                                                        size(sx * selection.width, sy * selection.height));
                                                    for bounds in [
                                                        Bounds::new(image.origin, size(image.size.width, selected.top() - image.top())),
                                                        Bounds::new(point(image.left(), selected.top()), size(selected.left() - image.left(), selected.size.height)),
                                                        Bounds::new(point(selected.right(), selected.top()), size(image.right() - selected.right(), selected.size.height)),
                                                        Bounds::new(point(image.left(), selected.bottom()), size(image.size.width, image.bottom() - selected.bottom())),
                                                    ] {
                                                        window.paint_quad(fill(bounds, rgba(0x00000066)));
                                                    }
                                                    window.paint_quad(quad(selected, Corners::default(), transparent_black(), Edges::all(px(1.)), white(), BorderStyle::Solid));
                                                }
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
                                                if let Some((before, after, split, before_bytes, after_bytes)) = &comparison {
                                                    let _ = window.paint_image(image, Corners::default(), before.clone(), 0, false);
                                                    let right = comparison_clip(image, *split);
                                                    window.with_content_mask(Some(ContentMask { bounds: right }), |window| {
                                                        let _ = window.paint_image(image, Corners::default(), after.clone(), 0, false);
                                                    });
                                                    let x = image.left() + image.size.width * *split;
                                                    window.paint_quad(fill(
                                                        Bounds::new(point(x - px(1.), image.top()), size(px(2.), image.size.height)),
                                                        t.glass_text,
                                                    ));
                                                    comparison_badge("↔", Bounds::new(point(x - px(18.), image.center().y - px(18.)), size(px(36.), px(36.))), t, window, cx);
                                                    window.with_content_mask(Some(ContentMask { bounds: image }), |window| {
                                                        comparison_badge("Hide", comparison_hide_bounds(image), t, window, cx);
                                                        comparison_badge(&format!("Before · {}", human_bytes(*before_bytes)), Bounds::new(point(image.left() + px(12.), image.bottom() - px(36.)), size(px(120.), px(24.))), t, window, cx);
                                                        comparison_badge(&format!("After · {}", human_bytes(*after_bytes)), Bounds::new(point(image.right() - px(132.), image.bottom() - px(36.)), size(px(120.), px(24.))), t, window, cx);
                                                    });
                                                }
                                            },
                                        )
                                        .size_full(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("screenshot-inspector")
                            .w(px(if compact { 280. } else { 320. }))
                            .flex_shrink_0()
                            .min_h_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .child(div().h(px(254.)).flex_shrink_0().border_b_1().border_color(t.border)
                                .child(div().h(px(48.)).px_3().flex().items_center().gap(px(6.)).border_b_1().border_color(t.border)
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Layers"))
                                    .child(div().min_w(px(19.)).h(px(19.)).rounded_full().bg(t.sunken).text_size(px(10.)).text_color(t.subtle).flex().items_center().justify_center().child((self.document.layers.len() + usize::from(self.document.source_present)).to_string()))
                                    .child(div().flex_1())
                                    .child(self.icon_button("add-layer", EditorIcon::Plus).on_click(cx.listener(|s, _, window, cx| s.prompt_images(window, cx)))))
                                .child(div().id("layer-list").h(px(205.)).overflow_y_scroll().p_2().children(layers)
                                    .when(self.document.source_present, |list| list.child(
                                        div().h(px(54.)).px_2().flex().items_center().gap_2()
                                            .child(div().text_size(px(11.)).text_color(t.subtle).child("⠿"))
                                            .child(img(self.source_thumbnail.clone()).w(px(42.)).h(px(30.)).object_fit(ObjectFit::Contain).rounded(px(6.)))
                                            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                                                .child(div().text_size(px(12.)).child(self.document.source_name.clone()))
                                                .child(div().text_size(px(11.)).text_color(t.subtle).child("Locked background")))
                                            .child(self.icon_button("source-visible", EditorIcon::Eye).w(px(24.)).opacity(if self.document.source_visible { 1. } else { 0.35 })
                                                .on_click(cx.listener(|s, _, _, cx| { s.document.toggle_source_visibility(); s.refresh(); cx.notify(); })))
                                            .child(div().id("source-unlock").w(px(24.)).h(px(30.)).rounded(px(6.)).bg(t.hover).flex().items_center().justify_center().cursor_pointer().child(editor_icon(EditorIcon::Lock, "#8b730a").size(px(14.)))
                                                .on_click(cx.listener(|s, _, _, cx| { s.selected = s.document.unlock_source(); s.tool = Tool::Select; s.refresh(); cx.notify(); })))
                                            .child(self.icon_button("source-menu", EditorIcon::More).w(px(24.))
                                                .on_click(cx.listener(|s, _, _, cx| { s.source_menu_open = !s.source_menu_open; cx.notify(); })))
                                    ))))
                            .when(self.tool != Tool::Select || selected.is_some(), |sidebar| {
                                sidebar.child(
                                    section(match self.tool {
                                        Tool::Select => "Selection", Tool::Crop => "Crop", Tool::Text => "Text", Tool::Pen => "Freehand", Tool::Arrow => "Arrow", Tool::Rectangle => "Rectangle", Tool::Ellipse => "Ellipse", Tool::Line => "Line", Tool::Triangle => "Triangle", Tool::Diamond => "Diamond", Tool::Star => "Star", Tool::Eraser => "Eraser",
                                    }, t)
                                        .id("tool-properties").flex_1().min_h_0().overflow_y_scroll()
                                        .when(!matches!(self.tool, Tool::Crop | Tool::Text | Tool::Eraser) && selected_text.is_none() && selected_image.is_none(), |panel| panel.child(self.style_controls(cx)))
                                        .when(self.tool == Tool::Crop, |panel| panel.child(
                                            div().flex().flex_col().gap_3()
                                                .child(field_container("Aspect ratio", t).child(
                                                    div().relative().child(
                                                        div().id("crop-aspect").h(px(34.)).px_3().flex().items_center().rounded(px(7.)).border_1().border_color(t.border).bg(t.field).cursor_pointer()
                                                            .child(div().flex_1().child(crop_aspect_label(self.crop_aspect))).child("⌄")
                                                            .on_click(cx.listener(|s, _, _, cx| { s.crop_aspect_open = !s.crop_aspect_open; cx.notify(); })))
                                                        .when(self.crop_aspect_open, |menu| menu.child(deferred(
                                                            div().occlude().absolute().top(px(38.)).left_0().w_full().p_1().bg(t.raised).border_1().border_color(t.border).rounded(px(8.)).shadow_lg()
                                                                .children([(None, "Free"), (Some(1.), "1 : 1"), (Some(4./3.), "4 : 3"), (Some(1.5), "3 : 2"), (Some(16./9.), "16 : 9")].into_iter().enumerate().map(|(index, (aspect, label))|
                                                                    div().id(SharedString::from(format!("crop-aspect-{index}"))).h(px(34.)).px_2().flex().items_center().rounded(px(5.)).cursor_pointer().bg(if self.crop_aspect == aspect { t.hover } else { t.raised }).hover(|item| item.bg(t.hover)).child(label)
                                                                        .on_click(cx.listener(move |s, _, _, cx| { s.crop_aspect = aspect; s.crop_aspect_open = false; cx.notify(); }))))
                                                        )))
                                                    )
                                                )
                                                .when_some(self.crop_selection, |panel, selection| panel
                                                    .child(div().flex().gap_2()
                                                        .child(value_field("WIDTH", selection.width.round().to_string(), t).flex_1())
                                                        .child(value_field("HEIGHT", selection.height.round().to_string(), t).flex_1()))
                                                    .child(div().flex().gap_2()
                                                        .child(self.button("crop-clear", "Clear", false).on_click(cx.listener(|s, _, _, cx| { s.crop_selection = None; cx.notify(); })))
                                                        .child(self.button("crop-apply", "Apply crop", true).on_click(cx.listener(|s, _, _, cx| {
                                                            if let Some(selection) = s.crop_selection.take() { s.document.set_crop(selection); s.refresh(); }
                                                            cx.notify();
                                                        })))))
                                                .child(div().text_size(px(12.)).text_color(t.muted).child(if self.crop_selection.is_some() { "Hold Shift while dragging to keep this aspect ratio." } else { "Drag over the area you want to keep. Start from outside the canvas to crop to an edge. Hold Shift to lock the current aspect ratio." }))
                                        ))
                                        .when(self.tool == Tool::Eraser, |panel| panel.child(div().text_size(px(12.)).text_color(t.muted).child("Drag over an image to remove its background.")))
                                        .when(self.custom_style_open && self.tool != Tool::Text && selected_text.is_none(), |panel| panel.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(field("LINE WIDTH", stroke_input, t))
                                                .child(field(
                                                    "STROKE · HEX OR RGBA",
                                                    stroke_color_input.clone(),
                                                    t,
                                                ))
                                                .child(field(
                                                    "FILL · NONE, HEX OR RGBA",
                                                    fill_color_input.clone(),
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
                                        ))
                                        .when(self.tool == Tool::Text || selected_text.is_some(), |panel| {
                                            let style = selected_text
                                                .as_ref()
                                                .map(|(_, _, _, style)| style.clone())
                                                .unwrap_or_else(|| self.default_text_style.clone());
                                            panel
                                            .on_key_up(cx.listener(|s, _, _, cx| {
                                                if let Err(error) = s.apply_text_fields(cx) { s.status = error; }
                                            }))
                                            .child(div().text_size(px(12.)).text_color(t.muted).child(if selected_text.is_some() { "Text style" } else { "New text style" }))
                                            .child(self.text_picker("text-preset", &style, cx))
                                            .when(selected_text.is_none(), |panel| panel.child(field("New text size", text_size_input.clone(), t)))
                                            .when(selected_text.is_some(), |panel| panel
                                            .child(field("Text", text_input, t))
                                            .child(
                                                div().grid().grid_cols(2).flex_shrink_0()
                                                    .gap_2()
                                                    .child(div().flex().flex_col().gap_1()
                                                        .child(div().text_size(px(10.)).text_color(t.subtle).child("Font"))
                                                        .child(self.text_picker("text-font", &style, cx)))
                                                    .child(field("Size", text_size_input, t)),
                                            )
                                            .child(
                                                div().grid().grid_cols(5).gap_1().flex_shrink_0()
                                                    .child(self.button("text-bold", "B", style.bold).on_click(cx.listener(|s, _, _, cx| s.change_text_font(TextFontChange::Bold, cx))))
                                                    .child(self.button("text-italic", "I", style.italic).on_click(cx.listener(|s, _, _, cx| s.change_text_font(TextFontChange::Italic, cx))))
                                                    .child(self.button("text-left", "", style.align == captures_image::TextAlign::Left).child(editor_icon(EditorIcon::AlignLeft, icon_color)).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| style.align = captures_image::TextAlign::Left, cx))))
                                                    .child(self.button("text-center", "", style.align == captures_image::TextAlign::Center).child(editor_icon(EditorIcon::AlignCenter, icon_color)).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| style.align = captures_image::TextAlign::Center, cx))))
                                                    .child(self.button("text-right", "", style.align == captures_image::TextAlign::Right).child(editor_icon(EditorIcon::AlignRight, icon_color)).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| style.align = captures_image::TextAlign::Right, cx)))),
                                            )
                                            .child(div().flex().flex_wrap().gap_2().flex_shrink_0()
                                                    .child(self.button("text-auto", "Auto width", style.width.is_none()).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| style.width = None, cx))))
                                                    .child(self.button("text-fixed", "Fixed width", style.width.is_some()).on_click(cx.listener(|s, _, _, cx| {
                                                        let width = input_value(&s.text_wrap_input, cx).trim().parse::<f32>().unwrap_or(240.).clamp(20., 4000.);
                                                        s.edit_text_settings(|style| style.width = Some(width), cx)
                                                    })))
                                                    .child(field("WRAP WIDTH", text_wrap_input, t)),
                                            )
                                            .child(field("Text color", stroke_color_input, t).on_key_up(cx.listener(|s, _, _, cx| {
                                                if let Ok(color) = parse_color("Text color", input_value(&s.stroke_color_input, cx)) {
                                                    s.edit_selected(|layer| layer.color = color, cx);
                                                }
                                            })))
                                            .child(self.button("text-background", "Text background", style.background.is_some()).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| { style.background = if style.background.is_some() { None } else { Some([17,19,24,255]) }; style.outlined = false; }, cx))))
                                            .when(style.background.is_some(), |panel| panel.child(field("Background color", fill_color_input, t).on_key_up(cx.listener(|s, _, _, cx| {
                                                if let Ok(color) = parse_color("Background color", input_value(&s.fill_color_input, cx)) { s.edit_text_settings(|style| style.background = Some(color), cx); }
                                            }))))
                                            )
                                            .child(self.button("text-shadow", "Drop shadow", style.shadow.is_some()).on_click(cx.listener(|s, _, _, cx| s.edit_text_settings(|style| { style.shadow = if style.shadow.is_some() { None } else { Some(captures_image::TextShadow { color: [0, 0, 0, 115], blur: 6., offset: captures_image::Point { x: 0., y: 3. } }) } }, cx))))
                                            .when(style.shadow.is_some(), |panel| panel.child(
                                                div().flex().flex_wrap().gap_2()
                                                    .child(field("BLUR", shadow_blur_input, t))
                                                    .child(field("OFFSET X", shadow_x_input, t))
                                                    .child(field("OFFSET Y", shadow_y_input, t))
                                                    .child(field("COLOR", shadow_color_input, t))
                                                    .child(field("OPACITY %", shadow_opacity_input, t)),
                                            ))
                                            .child(
                                                div().text_size(px(12.)).text_color(t.muted).child(
                                                    if selected_text.is_some() { "Editing selected text layer." } else { "Click the image to place text." },
                                                ),
                                            )
                                        })
                                        .when_some(selected_image, |panel, (_, _, _, _, locked)| panel.child(
                                            div().flex().flex_col().gap_3()
                                                .child(div().grid().grid_cols(2).gap_2()
                                                    .child(self.image_number_field("Width", "image-width", image_width_input, ImageNumber::Width, locked, cx))
                                                    .child(self.image_number_field("Height", "image-height", image_height_input, ImageNumber::Height, locked, cx))
                                                    .child(self.image_number_field("X", "image-x", image_x_input, ImageNumber::X, locked, cx))
                                                    .child(self.image_number_field("Y", "image-y", image_y_input, ImageNumber::Y, locked, cx)))
                                                .child(div().text_size(px(12.)).text_color(t.muted).child(if locked { "Unlock this layer to change size and position." } else { "Width and height stay proportional to the image." }))
                                        )),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .h(px(if compact { 101. } else { 77. }))
                    .flex_shrink_0()
                    .relative()
                    .py_2()
                    .when(compact, |bar| bar.pb(px(32.)))
                    .px_3()
                    .flex()
                    .items_end()
                    .gap_3()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .when(self.export_open, |bar| {
                        bar.child(
                            div()
                                .absolute().bottom(px(76.)).left_3().right_3().p_3().rounded(px(12.)).border_1().border_color(t.border).bg(t.sunken).shadow_lg()
                                .occlude()
                                .flex()
                                .flex_wrap()
                                .items_end()
                                .gap_2()
                                .child(field_container("Output size", t).child(
                                    div().flex().items_center().gap_2()
                                        .child(self.export_select("output-size", match self.export_scale { 100 => "100", 75 => "75", 50 => "50", _ => "custom" }, &[("100", "Original"), ("75", "75%"), ("50", "50%"), ("custom", "Custom")], cx))
                                        .child(div().text_size(px(10.)).text_color(t.subtle).child(format!("{} × {}", self.export_width, self.export_height)))
                                ))
                                .when(self.export_scale == 0, |settings| settings
                                    .child(self.custom_export_number_field("Width × height", "export-width", export_width_input, true, cx))
                                    .child(div().pb(px(9.)).text_color(t.subtle).child("×"))
                                    .child(self.custom_export_number_field("", "export-height", export_height_input, false, cx))
                                    .child(self.icon_button("aspect-lock", if self.export_aspect_locked { EditorIcon::Lock } else { EditorIcon::Unlock })
                                        .on_click(cx.listener(|s, _, _, cx| { s.export_aspect_locked = !s.export_aspect_locked; cx.notify(); }))))
                                .child(field_container("Save quality", t).child(
                                    self.export_select("quality-mode", match self.export_quality_mode { ExportQualityMode::Preserve => "preserve", ExportQualityMode::Compress => "compress", ExportQualityMode::Maximum => "maximum" }, &[("preserve", "Preserve quality"), ("compress", "Compress"), ("maximum", "Maximum file size")], cx)
                                ))
                                .when(self.export_quality_mode == ExportQualityMode::Compress, |settings| settings.child(field_container("Quality", t).child(
                                    self.export_select("compression-quality", match self.quality { 55 => "55", 70 => "70", 85 => "85", 92 => "92", _ => "98" }, &[("55", "Tiny"), ("70", "Smaller"), ("85", "Balanced"), ("92", "High"), ("98", "Highest")], cx)
                                )))
                                .when(self.export_quality_mode == ExportQualityMode::Maximum, |settings| settings
                                    .child(field("Maximum file size", max_size_input, t).w(px(110.)).on_key_up(cx.listener(|s, _, _, cx| { if s.apply_export_fields(cx).is_ok() { s.invalidate_compression_preview(); } cx.notify(); })))
                                    .child(self.export_select("size-unit", match self.export_size_unit { FileSizeUnit::Kb => "kb", FileSizeUnit::Mb => "mb", FileSizeUnit::Gb => "gb" }, &[("kb", "KB"), ("mb", "MB"), ("gb", "GB")], cx)))
                                .child(field_container("Est. size", t).min_w(px(105.)).h(px(52.)).justify_between().child(div().h(px(32.)).flex().items_center().font_family("monospace").child(self.compression_preview.as_ref().map(|p| human_bytes(p.after_bytes)).unwrap_or_else(|| if self.compression_preview_pending { "Estimating…".into() } else { "—".into() }))))
                                .when(self.export_quality_mode != ExportQualityMode::Preserve && self.compression_compare_dismissed, |settings| settings.child(
                                    self.button("show-comparison", "Show before / after", false).on_click(cx.listener(|s, _, _, cx| { s.compression_compare_dismissed = false; cx.notify(); }))
                                )),
                        )
                    })
                    .child(
                        self.button("export-options", "", self.export_open).w(px(210.)).h(px(43.)).flex_shrink_0().flex_col().items_start().justify_start().py(px(5.)).gap(px(2.))
                            .child(div().w_full().flex().justify_between().child("Export settings").child("⌄"))
                            .child(div().text_size(px(9.)).text_color(t.subtle).child(format!("{} · {} × {}", self.format.extension().to_uppercase(), self.export_width, self.export_height)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_open = !this.export_open;
                                this.invalidate_compression_preview();
                                cx.notify()
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.))
                            .flex().flex_col().gap_2()
                            .child(
                                div().flex().items_center().gap_2().text_size(px(10.)).text_color(t.subtle)
                                    .child("Filename")
                                    .child(div().flex_1())
                                    .child(div().max_w(px(210.)).truncate().child(format!("Saving to {}", self.destination.parent().unwrap_or(Path::new(".")).display())))
                                    .child(self.button("destination", "Change…", false).h(px(16.)).px_0().border_0().text_size(px(10.)).text_color(t.subtle)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let parent = this.destination.parent().unwrap_or(Path::new("."));
                                let name = format!("Capture-edited.{}", this.format.extension());
                                let receiver = cx.prompt_for_new_path(parent, Some(&name));
                                cx.spawn_in(window, async move |this, cx| {
                                    if let Ok(Ok(Some(path))) = receiver.await {
                                        let _ = this.update(cx, |editor, cx| {
                                            editor.destination = path;
                                            editor.filename_input = None;
                                            editor.invalidate_compression_preview();
                                            editor.status = "Destination updated".into();
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            }))))
                            .child(div().h(px(36.)).flex().items_center().border_1().border_color(t.border).rounded(px(8.)).bg(t.field)
                                .child(div().flex_1().min_w_0().child(filename_input))
                                .child(self.button("format", format!(".{}  ⌄", self.format.extension()), false).h(px(34.)).border_0().border_l_1().text_color(t.subtle)
                                    .on_click(cx.listener(|s, _, _, cx| { s.format_open = !s.format_open; cx.notify(); })))),
                    )
                    .child(self.button("copy-image", "", false).h(px(36.)).gap_2().flex_shrink_0()
                        .child(editor_icon(EditorIcon::Copy, icon_color).size(px(14.))).child("Copy image")
                        .on_click(cx.listener(|this, _, _, cx| this.copy_image(cx))))
                    .child(div().w(px(250.)).min_w(px(100.)).text_size(px(10.)).text_color(t.subtle).text_center()
                        .when(compact, |status| status.absolute().bottom(px(6.)).left_3().w_auto().text_left())
                        .child(
                        if !matches!(self.status.as_str(), "Ready" | "Draft autosaved") { self.status.clone() }
                        else if self.make_copy { "Save creates a new file and leaves the original untouched.".into() }
                        else { "Save overwrites the chosen file.".into() }
                    ))
                    .child(
                        div().id("make-copy").h(px(36.)).flex_shrink_0().flex().items_center().gap_2().cursor_pointer()
                            .child(div().w(px(27.)).h(px(15.)).rounded_full().border_1().border_color(t.border).bg(if self.make_copy { t.accent } else { t.field }).px(px(2.)).flex().items_center()
                                .when(self.make_copy, |toggle| toggle.justify_end())
                                .child(div().size(px(9.)).rounded_full().bg(t.subtle)))
                            .child(div().text_size(px(10.)).text_color(t.muted).child("Save as new file"))
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.make_copy = !s.make_copy;
                                if let Some(source) = &s.source_path {
                                    s.destination = source.clone();
                                    if s.make_copy {
                                        s.destination.set_file_name(format!("{}-edited.{}", source.file_stem().unwrap_or_default().to_string_lossy(), s.format.extension()));
                                    }
                                    s.filename_input = None;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        self.button("save", "", true).h(px(36.)).w(px(106.)).flex_shrink_0().gap_2()
                            .bg(t.accent)
                            .text_color(rgb(0x17140a))
                            .child(editor_icon(EditorIcon::Save, "#17140a").size(px(14.))).child("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                    ),
            )
            .when(self.shapes_open || self.zoom_open || self.background_open || self.format_open || self.source_menu_open, |root| root.child(
                div().absolute().inset_0().on_mouse_down(MouseButton::Left, cx.listener(|s, _, _, cx| {
                    cx.stop_propagation(); s.shapes_open = false; s.zoom_open = false; s.background_open = false; s.format_open = false; s.source_menu_open = false; cx.notify();
                }))
            ))
            .when(self.source_menu_open, |root| root.child(
                div().occlude().absolute().right_3().top(px(155.)).w(px(170.)).p_2().rounded(px(10.)).border_1().border_color(t.border).bg(t.raised).shadow_lg().flex().flex_col()
                    .child(self.button("flatten", "Flatten", false).border_0().justify_start().on_click(cx.listener(|s, _, _, cx| {
                        match s.document.flatten_layers() { Ok(_) => s.refresh(), Err(e) => s.status = e }
                        s.source_menu_open = false; cx.notify();
                    })))
                    .child(self.button("draft", "Save draft", false).border_0().justify_start().on_click(cx.listener(|s, _, _, cx| { s.source_menu_open = false; s.save_draft(cx); })))
            ))
            .when(self.shapes_open, |root| root.child(
                div().occlude().absolute().left(px(56.)).top(px(148.)).p(px(6.)).w(px(154.)).flex().flex_wrap().gap(px(4.)).rounded(px(10.)).border_1().border_color(t.border).bg(t.raised).shadow_lg()
                    .children(shapes.into_iter().map(|(name, icon, tool)| {
                        self.button(name, "", self.tool == tool).size(px(44.)).px_0().border_0().bg(if self.tool == tool { t.accent } else { t.raised })
                            .child(editor_icon(icon, if self.tool == tool { "#17140a" } else { icon_color }).size(px(22.)))
                            .on_click(cx.listener(move |s, _, _, cx| { s.shapes_open = false; s.choose_tool(tool, cx); }))
                    }))
            ))
            .when(self.zoom_open, |root| root.child(
                div().occlude().absolute().right(px(132.)).top(px(46.)).w(px(100.)).p_2().rounded(px(10.)).border_1().border_color(t.border).bg(t.raised).shadow_lg()
                    .children([(0, "Fit"), (50, "50%"), (100, "100%"), (200, "200%")].into_iter().map(|(zoom, label)| {
                        self.button(label, label, if zoom == 0 { self.fit } else { !self.fit && self.zoom == zoom }).w_full().border_0().justify_start()
                            .on_click(cx.listener(move |s, _, _, cx| {
                                s.fit = zoom == 0; if zoom > 0 { s.zoom = zoom; } s.pan = point(px(0.), px(0.)); s.zoom_open = false; cx.notify();
                            }))
                    }))
            ))
            .when(self.background_open, |root| root.child(
                div().occlude().absolute().left(px(365.)).top(px(46.)).w(px(248.)).p_3().rounded(px(10.)).border_1().border_color(t.border).bg(t.raised).shadow_lg().flex().flex_col().gap_2()
                    .child("Background color")
                    .children([("Transparent", None), ("Off-white", Some([247,247,245,255])), ("White", Some([255,255,255,255])), ("Black", Some([0,0,0,255]))].into_iter().map(|(name, color)| {
                        self.button(name, name, self.document.background == color).on_click(cx.listener(move |s, _, _, cx| {
                            s.document.set_background(color); s.background_open = false; s.refresh(); cx.notify();
                        }))
                    }))
                    .child(div().pt_2().border_t_1().border_color(t.border).text_size(px(10.)).text_color(t.subtle).child("CUSTOM COLOR · HEX OR RGBA"))
                    .child(background_color_input)
                    .child(self.button("apply-background-color", "Use custom color", true).on_click(cx.listener(|s, _, _, cx| {
                        s.status = match s.apply_background_color(cx) { Ok(()) => "Background updated".into(), Err(error) => error }; cx.notify();
                    })))
            ))
            .when(self.format_open, |root| root.child(
                div().occlude().absolute().left(px(475.)).bottom(px(47.)).w(px(135.)).p_2().rounded(px(10.)).border_1().border_color(t.border).bg(t.raised).shadow_lg()
                    .children([(Format::Png, "PNG"), (Format::Jpeg, "JPEG"), (Format::Webp, "WebP")].into_iter().map(|(format, name)| {
                        self.button(name, name, self.format == format).w_full().border_0().justify_start().on_click(cx.listener(move |s, _, _, cx| {
                            s.format = format; s.format_open = false; s.invalidate_compression_preview(); cx.notify();
                        }))
                    }))
            ))
            .when_some(self.layer_menu, |root, id| root.child(self.layer_settings(id, window.viewport_size(), cx)))
    }
}

fn ensure_chrome_input(
    slot: &mut Option<Entity<crate::preferences::input::TextInput>>,
    value: impl ToString,
    placeholder: &'static str,
    cx: &mut Context<ScreenshotEditor>,
) -> Entity<crate::preferences::input::TextInput> {
    slot.get_or_insert_with(|| {
        let value = value.to_string();
        cx.new(|cx| crate::preferences::input::TextInput::new(value, placeholder, cx).chrome())
    })
    .clone()
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
        if input.read(cx).value() != value {
            input.update(cx, |input, cx| input.set_value(value, cx));
        }
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
    field_container(label, theme).child(input)
}

fn field_container(label: &'static str, theme: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_1()
        .min_w(px(72.))
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme.subtle)
                .child(label),
        )
}

fn value_field(label: &'static str, value: String, theme: Theme) -> Div {
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
        .child(
            div()
                .h(px(34.))
                .px_3()
                .flex()
                .items_center()
                .rounded(px(7.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.field)
                .text_size(px(12.))
                .child(value),
        )
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

fn parse_maximum_size(value: String, unit: FileSizeUnit) -> Result<Option<u64>, String> {
    let number = value
        .trim()
        .parse::<f64>()
        .map_err(|_| "Maximum file size must be a positive number".to_string())?;
    if !number.is_finite() || number <= 0. {
        return Err("Maximum file size must be greater than zero".into());
    }
    let multiplier = match unit {
        FileSizeUnit::Kb => 1_000.,
        FileSizeUnit::Mb => 1_000_000.,
        FileSizeUnit::Gb => 1_000_000_000.,
    };
    let bytes = number * multiplier;
    if bytes > u64::MAX as f64 {
        return Err("Maximum file size is too large".into());
    }
    Ok(Some(bytes.round() as u64))
}

fn proportional_height(width: u32, source_width: u32, source_height: u32) -> u32 {
    let denominator = u64::from(source_width.max(1));
    let height = (u64::from(width) * u64::from(source_height) + denominator / 2) / denominator;
    height.clamp(1, 16_384) as u32
}

fn proportional_image_size(
    width: f32,
    height: f32,
    requested: f32,
    width_changed: bool,
) -> (f32, f32) {
    let ratio = if width.is_finite() && height.is_finite() && width > 0. && height > 0. {
        width / height
    } else {
        1.
    };
    if width_changed {
        let next_width = requested.round().clamp(1., 16_384.).min(16_384. * ratio);
        (next_width, (next_width / ratio).round().clamp(1., 16_384.))
    } else {
        let next_height = requested.round().clamp(1., 16_384.).min(16_384. / ratio);
        (
            (next_height * ratio).round().clamp(1., 16_384.),
            next_height,
        )
    }
}

fn crop_aspect_label(aspect: Option<f32>) -> &'static str {
    match aspect {
        None => "Free",
        Some(1.) => "1 : 1",
        Some(value) if value == 4. / 3. => "4 : 3",
        Some(1.5) => "3 : 2",
        Some(_) => "16 : 9",
    }
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
        .px_3()
        .pb_3()
        .gap_3()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .h(px(48.))
                .flex_shrink_0()
                .mx(-px(12.))
                .px_3()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(t.border)
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title),
        )
}
/// GPUI 0.2.2's SVG decoder returns tiny-skia premultiplied RGBA, unlike
/// its raster decoders and RenderImage's straight-alpha BGRA contract.
pub(crate) fn svg_render_image(svg: String) -> Arc<RenderImage> {
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default())
        .expect("valid editor SVG");
    let size = tree.size().to_int_size();
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(size.width(), size.height()).expect("SVG dimensions");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    unpremultiply_svg_bgra(pixmap.data_mut());
    Arc::new(RenderImage::new([image::Frame::new(
        RgbaImage::from_raw(size.width(), size.height(), pixmap.take()).expect("SVG dimensions"),
    )]))
}

fn unpremultiply_svg_bgra(bytes: &mut [u8]) {
    for pixel in bytes.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
        pixel.swap(0, 2);
    }
}

fn render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = image.clone();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(bgra)]))
}

/// ScreenshotEditor.ts's boundedCropRect, with the document crop origin retained.
fn bounded_crop(start: Point, end: Point, bounds: Rect, aspect: Option<f32>) -> Rect {
    let start = Point {
        x: start.x.clamp(bounds.x, bounds.x + bounds.width),
        y: start.y.clamp(bounds.y, bounds.y + bounds.height),
    };
    let mut end = Point {
        x: end.x.clamp(bounds.x, bounds.x + bounds.width),
        y: end.y.clamp(bounds.y, bounds.y + bounds.height),
    };
    if let Some(aspect) = aspect.filter(|r| r.is_finite() && *r > 0.) {
        let dx = if end.x < start.x { -1. } else { 1. };
        let dy = if end.y < start.y { -1. } else { 1. };
        let mut width = (end.x - start.x)
            .abs()
            .max((end.y - start.y).abs() * aspect);
        width = width.min(if dx > 0. {
            bounds.x + bounds.width - start.x
        } else {
            start.x - bounds.x
        });
        let height = (width / aspect).min(if dy > 0. {
            bounds.y + bounds.height - start.y
        } else {
            start.y - bounds.y
        });
        end = Point {
            x: start.x + height * aspect * dx,
            y: start.y + height * dy,
        };
    }
    let rect = normalized_rect(start, end, false);
    Rect {
        x: rect.x.round(),
        y: rect.y.round(),
        width: rect.width.round().max(1.),
        height: rect.height.round().max(1.),
    }
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
        (Format::Png, None) => encode_png(
            &output,
            (spec.export_quality_mode != ExportQualityMode::Preserve).then_some(spec.quality),
        ),
        (Format::Jpeg, None) => encode_jpeg(&output, spec.quality),
        (Format::Webp, None) => encode_webp(
            &output,
            (spec.export_quality_mode != ExportQualityMode::Preserve).then_some(spec.quality),
        ),
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

fn follow_canvas_export_size(
    previous: (u32, u32),
    canvas: (u32, u32),
    output: (u32, u32),
    percent: u8,
) -> (u32, u32) {
    let scaled = |(width, height): (u32, u32)| {
        (
            (width * u32::from(percent) / 100).max(1),
            (height * u32::from(percent) / 100).max(1),
        )
    };
    if output == scaled(previous) {
        scaled(canvas)
    } else {
        output
    }
}

fn preset_output_size(canvas: (u32, u32), percent: u8) -> (u32, u32) {
    (
        (canvas.0 * u32::from(percent) / 100).max(1),
        (canvas.1 * u32::from(percent) / 100).max(1),
    )
}

fn comparison_clip(bounds: Bounds<Pixels>, split: f32) -> Bounds<Pixels> {
    let split = split.clamp(0., 1.);
    Bounds::new(
        point(bounds.left() + bounds.size.width * split, bounds.top()),
        size(bounds.size.width * (1. - split), bounds.size.height),
    )
}

fn comparison_hide_bounds(image: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(image.right() - px(64.), image.top() + px(12.)),
        size(px(52.), px(24.)),
    )
}

fn comparison_badge(
    text: &str,
    bounds: Bounds<Pixels>,
    t: Theme,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(quad(
        bounds,
        Corners::all(bounds.size.height / 2.),
        t.glass,
        Edges::all(px(1.)),
        t.glass_border,
        BorderStyle::Solid,
    ));
    let line = window.text_system().shape_line(
        text.to_owned().into(),
        px(11.),
        &[TextRun {
            len: text.len(),
            font: gpui::font(font()),
            color: t.glass_text.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let _ = line.paint(
        point(
            bounds.left() + (bounds.size.width - line.width) / 2.,
            bounds.top() + (bounds.size.height - px(16.)) / 2.,
        ),
        px(16.),
        window,
        cx,
    );
}

fn decode(path: &Path) -> anyhow::Result<RgbaImage> {
    Ok(image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .to_rgba8())
}
fn export_path(destination: &Path, name: &str, format: Format) -> Result<PathBuf, &'static str> {
    if matches!(name.trim(), "" | "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err("Enter a filename without directory separators");
    }
    Ok(destination.with_file_name(format!("{name}.{}", format.extension())))
}

fn write_export(path: &Path, bytes: &[u8], new_file: bool) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing parent"))?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    if new_file {
        tmp.persist_noclobber(path).map_err(|e| e.error)?;
    } else {
        tmp.persist(path).map_err(|e| e.error)?;
    }
    Ok(())
}
fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

fn edit_text_style_state(
    document: &mut Document,
    selected: Option<u64>,
    defaults: &mut captures_image::TextStyleSettings,
    edit: impl FnOnce(&mut captures_image::TextStyleSettings),
) -> Result<bool, &'static str> {
    let Some(id) = selected else {
        edit(defaults);
        return Ok(false);
    };
    let Some(mut layer) = document.layers.iter().find(|layer| layer.id == id).cloned() else {
        return Ok(false);
    };
    if layer.locked {
        return Err("Unlock this layer before editing it");
    }
    if !matches!(&layer.shape, Shape::Text { .. }) {
        return Ok(false);
    }
    let original = layer.clone();
    if let Shape::Text { style, .. } = &mut layer.shape {
        edit(style);
    }
    document.preview_layer(layer);
    document.commit_layer_preview(original);
    Ok(true)
}

fn update_text_effect_fields(
    style: &mut captures_image::TextStyleSettings,
    width: f32,
    blur: f32,
    offset_x: f32,
    offset_y: f32,
    shadow_color: [u8; 4],
) {
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

    fn text_document() -> (Document, u64) {
        let mut document = Document::new(RgbaImage::new(320, 200));
        let id = document.add(
            Shape::Text {
                origin: Point { x: 12., y: 18. },
                value: "Text".into(),
                font_size: 32.,
                font_data: Arc::from([]),
                style: Default::default(),
            },
            [255, 255, 255, 255],
            0.,
        );
        (document, id)
    }

    #[test]
    fn preplacement_text_settings_become_new_layer_style() {
        let mut document = Document::new(RgbaImage::new(320, 200));
        let mut defaults = captures_image::TextStyleSettings::default();
        assert!(
            !edit_text_style_state(&mut document, None, &mut defaults, |style| {
                style.bold = true;
                style.align = captures_image::TextAlign::Center;
                style.width = Some(280.);
            })
            .unwrap()
        );
        let id = document.add(
            Shape::Text {
                origin: Point { x: 5., y: 7. },
                value: "Placed".into(),
                font_size: 40.,
                font_data: Arc::from([]),
                style: defaults,
            },
            [255, 255, 255, 255],
            0.,
        );
        let Shape::Text { style, .. } = &document.layers.iter().find(|l| l.id == id).unwrap().shape
        else {
            panic!()
        };
        assert!(style.bold);
        assert_eq!(style.align, captures_image::TextAlign::Center);
        assert_eq!(style.width, Some(280.));
    }

    #[test]
    fn selected_text_style_edits_are_undoable_and_locked_text_is_unchanged() {
        let (mut document, id) = text_document();
        let mut defaults = captures_image::TextStyleSettings::default();
        assert!(
            edit_text_style_state(&mut document, Some(id), &mut defaults, |style| style
                .italic =
                true)
            .unwrap()
        );
        assert!(matches!(&document.layers[0].shape, Shape::Text { style, .. } if style.italic));
        assert!(document.undo());
        assert!(matches!(&document.layers[0].shape, Shape::Text { style, .. } if !style.italic));
        document.toggle_locked(id);
        assert!(
            edit_text_style_state(&mut document, Some(id), &mut defaults, |style| style.bold =
                true)
            .is_err()
        );
        assert!(matches!(&document.layers[0].shape, Shape::Text { style, .. } if !style.bold));
        assert!(
            !defaults.bold,
            "locked selected text must not redirect edits to defaults"
        );
    }

    #[test]
    fn unlocking_source_preserves_cropped_pixels_layers_and_single_undo() {
        let mut source = RgbaImage::from_pixel(12, 8, image::Rgba([31, 81, 142, 255]));
        source.put_pixel(6, 5, image::Rgba([255, 10, 79, 255]));
        let mut document = Document::new(source);
        let top = document.add(
            Shape::Rectangle(Rect {
                x: 3.,
                y: 2.,
                width: 4.,
                height: 3.,
            }),
            [230, 130, 40, 255],
            1.,
        );
        document.set_crop(Rect {
            x: 2.,
            y: 1.,
            width: 9.,
            height: 6.,
        });
        let before = document.render().unwrap();
        let id = document.unlock_source().unwrap();
        assert_eq!(document.layers[0].id, id);
        assert_eq!(document.layers[1].id, top);
        assert!(!document.layers[0].locked);
        assert!(!document.source_present);
        assert_eq!(document.render().unwrap(), before);
        assert!(document.undo());
        assert!(document.source_present);
        assert_eq!(document.layers.len(), 1);
        assert_eq!(document.render().unwrap(), before);
        document.toggle_source_visibility();
        let id = document.unlock_source().unwrap();
        assert!(!document.layers.iter().find(|l| l.id == id).unwrap().visible);
    }

    #[test]
    fn crop_presets_clamp_both_directions_and_keep_document_origin() {
        let bounds = Rect {
            x: 30.,
            y: 20.,
            width: 600.,
            height: 300.,
        };
        let start = Point { x: 130., y: 70. };
        assert_eq!(
            bounded_crop(start, Point { x: 900., y: 400. }, bounds, Some(2.)),
            Rect {
                x: 130.,
                y: 70.,
                width: 500.,
                height: 250.
            }
        );
        assert_eq!(
            bounded_crop(start, Point { x: -10., y: -20. }, bounds, Some(1.)),
            Rect {
                x: 80.,
                y: 20.,
                width: 50.,
                height: 50.
            }
        );
        assert_eq!(
            bounded_crop(start, Point { x: 401., y: 173. }, bounds, None),
            Rect {
                x: 130.,
                y: 70.,
                width: 271.,
                height: 103.
            }
        );
    }

    #[test]
    fn export_follows_cropped_canvas_at_percentage_scale_but_preserves_override() {
        assert_eq!(
            follow_canvas_export_size((960, 540), (554, 351), (960, 540), 100),
            (554, 351)
        );
        assert_eq!(
            follow_canvas_export_size((960, 540), (554, 351), (480, 270), 50),
            (277, 175)
        );
        assert_eq!(
            follow_canvas_export_size((960, 540), (554, 351), (800, 600), 100),
            (800, 600)
        );
    }

    #[test]
    fn image_numeric_updates_keep_aspect_and_round_pixels() {
        assert_eq!(
            proportional_image_size(400., 200., 333., true),
            (333., 167.)
        );
        assert_eq!(
            proportional_image_size(400., 200., 333., false),
            (666., 333.)
        );
        assert_eq!(proportional_image_size(0., 0., 42., true), (42., 42.));
    }

    #[test]
    fn image_aspect_constraints_clamp_the_derived_asymmetric_dimension() {
        assert_eq!(
            proportional_image_size(1., 4., 16_384., true),
            (4096., 16_384.)
        );
        assert_eq!(
            proportional_image_size(4., 1., 16_384., false),
            (16_384., 4096.)
        );
        assert_eq!(proportional_image_size(4., 1., -20., true), (1., 1.));
    }

    #[test]
    fn svg_conversion_preserves_asymmetric_colors_and_unpremultiplies_edges() {
        let mut pixels = [255, 59, 92, 255, 100, 25, 50, 128, 0, 0, 0, 0];
        unpremultiply_svg_bgra(&mut pixels);
        assert_eq!(pixels, [92, 59, 255, 255, 100, 50, 199, 128, 0, 0, 0, 0]);
        let image = svg_render_image(r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="1"><path d="M0 0h1v1H0Z" fill="#ff3b5c"/><path d="M1 0h1v1H1Z" fill="#c83264" fill-opacity="0.5"/></svg>"##.into());
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[92, 59, 255, 255]);
        assert_eq!(bytes[7], 128);
        assert!((i16::from(bytes[4]) - 100).abs() <= 1);
        assert!((i16::from(bytes[5]) - 50).abs() <= 1);
        assert!((i16::from(bytes[6]) - 200).abs() <= 1);
    }

    #[test]
    fn filename_field_keeps_dots_and_rejects_paths_or_empty_names() {
        let destination = Path::new("/captures/old.png");
        assert_eq!(
            export_path(destination, "screen.v2", Format::Jpeg).unwrap(),
            Path::new("/captures/screen.v2.jpg")
        );
        for invalid in [
            "",
            " ",
            ".",
            "..",
            "../source",
            "folder/image",
            "folder\\image",
            "bad\0name",
        ] {
            assert!(export_path(destination, invalid, Format::Png).is_err());
        }
    }

    #[test]
    fn save_as_new_refuses_existing_files_and_explicit_overwrite_replaces_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        write_export(&path, b"original", true).unwrap();
        assert!(write_export(&path, b"edited", true).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        write_export(&path, b"edited", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"edited");
    }

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
        write_export(&output, b"new bytes", true).unwrap();
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
        write_export(&destination, &bytes, true).unwrap();
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
        assert_eq!(proportional_height(width, 960, 600), 198);
        assert_eq!(proportional_height(600, 960, 540), 338);
        assert_eq!(proportional_height(599, 960, 540), 337);
        assert_eq!(proportional_height(16_384, 1, 16_384), 16_384);
    }

    #[test]
    fn output_presets_and_custom_dimensions_are_independent() {
        assert_eq!(preset_output_size((1001, 777), 100), (1001, 777));
        assert_eq!(preset_output_size((1001, 777), 75), (750, 582));
        assert_eq!(preset_output_size((1001, 777), 50), (500, 388));
        let custom = (
            parse_dimension("Width", "317".into()).unwrap(),
            parse_dimension("Height", "149".into()).unwrap(),
        );
        assert_eq!(custom, (317, 149));
    }

    #[test]
    fn quality_presets_and_size_units_produce_distinct_bytes() {
        assert_eq!(
            parse_maximum_size("1.5".into(), FileSizeUnit::Kb).unwrap(),
            Some(1_500)
        );
        assert_eq!(
            parse_maximum_size("1.5".into(), FileSizeUnit::Gb).unwrap(),
            Some(1_500_000_000)
        );
        let image = mock_artwork();
        let preserve = encode_png(&image, None).unwrap();
        let compressed = encode_png(&image, Some(55)).unwrap();
        assert_ne!(preserve, compressed);
    }

    #[test]
    fn comparison_clip_uses_the_full_canvas_coordinate_space() {
        let canvas = Bounds::new(point(px(37.), px(19.)), size(px(800.), px(450.)));
        let right = comparison_clip(canvas, 0.35);
        assert_eq!(right.origin, point(px(317.), px(19.)));
        assert_eq!(right.size, size(px(520.), px(450.)));
        assert_eq!(comparison_clip(canvas, -1.), canvas);
        assert_eq!(comparison_clip(canvas, 2.).size.width, px(0.));
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
        assert_eq!(
            parse_maximum_size("750".into(), FileSizeUnit::Kb).unwrap(),
            Some(750_000)
        );
        assert!(
            parse_maximum_size("large".into(), FileSizeUnit::Mb)
                .unwrap_err()
                .contains("positive number")
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
                export_quality_mode: ExportQualityMode::Compress,
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
