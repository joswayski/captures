//! Shipping screenshot-editor chrome shared by both native hosts: the header,
//! tool rail and Layers panel copy, tooltips, icon names and responsive rules
//! from `ScreenshotEditor.tsx` and `styles/editor-image.css`.
//!
//! Hosts own drawing and input. Nothing here edits a document, draft or output.

use crate::editor::{Element, Rect};

/// Header row height (`.screenshot-editor-header-main { min-height: 52px }`).
pub const HEADER_HEIGHT: f64 = 52.;
/// Left tool rail column width (`grid-template-columns: 56px …`).
pub const RAIL_WIDTH: f64 = 56.;
/// Rail button side (`.screenshot-tool-rail button { width: 38px }`).
pub const RAIL_BUTTON: f64 = 38.;
/// Header icon buttons and grouped controls (`min-height: 34px`).
pub const HEADER_CONTROL: f64 = 34.;
/// Zoom group buttons (`.screenshot-editor-zoom > button { width: 30px }`).
pub const ZOOM_BUTTON: f64 = 30.;
/// Canvas width/height fields (`.screenshot-canvas-dim .number-input`).
pub const CANVAS_FIELD: f64 = 74.;
/// Shapes flyout grid (`grid-template-columns: repeat(3, 44px)`).
pub const SHAPE_FLYOUT_COLUMNS: usize = 3;
pub const SHAPE_FLYOUT_BUTTON: f64 = 44.;
/// Below this window width shipping hides undo/redo and narrows zoom controls.
pub const COMPACT_WIDTH: f64 = 1040.;

/// Width-dependent header rules from the `max-width: 1040px` media query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderLayout {
    /// Undo/Redo buttons stay available through Cmd/Ctrl Z when hidden.
    pub show_history: bool,
    pub zoom_slider_width: f64,
    pub zoom_preset_width: f64,
}

#[must_use]
pub fn header_layout(window_width: f64) -> HeaderLayout {
    // `max-width: 1040px` includes 1040 itself.
    let compact = window_width <= COMPACT_WIDTH;
    HeaderLayout {
        show_history: !compact,
        zoom_slider_width: if compact { 72. } else { 92. },
        zoom_preset_width: if compact { 72. } else { 76. },
    }
}

/// Header copy. Tooltips are the shipping `title` attributes; labels are the
/// accessible names.
pub mod header {
    pub const CANVAS: &str = "Canvas";
    pub const CANVAS_WIDTH: &str = "Canvas width";
    pub const CANVAS_HEIGHT: &str = "Canvas height";
    pub const TRIM: &str = "Trim edges";
    pub const TRIM_TOOLTIP: &str = "Shrink the canvas to the edges of visible layers";
    pub const UNDO: &str = "Undo";
    pub const REDO: &str = "Redo";
    pub const ZOOM_GROUP: &str = "Canvas zoom controls";
    pub const FIT: &str = "Fit canvas";
    pub const FIT_TOOLTIP: &str = "Fit canvas to window";
    pub const ZOOM_OUT: &str = "Zoom out";
    pub const ZOOM_IN: &str = "Zoom in";
    pub const ZOOM_SLIDER: &str = "Canvas zoom";
    pub const ZOOM_SLIDER_TOOLTIP: &str = "Drag to zoom · Pinch or Command/Ctrl + scroll also work";
    pub const ZOOM_PRESET: &str = "Canvas zoom preset";
    pub const ZOOM_PRESET_TOOLTIP: &str = "Zoom presets";
    pub const ZOOM_PRESETS: [f64; 3] = [50., 100., 200.];
    pub const ADD_IMAGES: &str = "Add images";
    pub const RECENTER: &str = "Recenter";
    /// Native drafts are explicit (shipping autosaves), so both hosts keep
    /// these actions in one compact header menu.
    pub const DRAFT_MENU: &str = "Draft actions";
    pub const SAVE_DRAFT: &str = "Save draft";
    pub const DISCARD_EDITS: &str = "Discard edits…";
    pub const DRAFT_RESTORED: &str = "Restored unsaved edits from last time.";
    pub const DRAFT_DISCARD: &str = "Discard";
    pub const DRAFT_DISMISS: &str = "Dismiss";
    pub const DRAFT_DISMISS_LABEL: &str = "Dismiss restored-edits notice";
}

/// Shipping zoom value label: integers as-is, otherwise one decimal with
/// `toFixed`'s ties-away rounding (12.25 → `12.3%`).
#[must_use]
pub fn zoom_label(percent: f64) -> String {
    if percent.fract() == 0. {
        format!("{percent:.0}%")
    } else {
        format!("{:.1}%", (percent * 10.).round() / 10.)
    }
}

/// Shipping `isCanvasMostlyOffscreen`: Recenter fades in when free pan leaves
/// no overlap or only a sliver (under 48×48 points or 4% of the canvas).
#[must_use]
pub fn canvas_mostly_offscreen(viewport: Rect, canvas: Rect) -> bool {
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| (a1.min(b1) - a0.max(b0)).max(0.);
    let area = overlap(
        canvas.x,
        canvas.x + canvas.width,
        viewport.x,
        viewport.x + viewport.width,
    ) * overlap(
        canvas.y,
        canvas.y + canvas.height,
        viewport.y,
        viewport.y + viewport.height,
    );
    if area <= 0. {
        return true;
    }
    area < (48_f64 * 48.).min((canvas.width * canvas.height).max(1.) * 0.04)
}

/// One left-rail tool, in shipping order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailTool {
    /// Host tool key; also the keyboard shortcut in lower case where present.
    pub key: &'static str,
    /// Shared icon name for [`crate::icons::paths`].
    pub icon: &'static str,
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
}

impl RailTool {
    /// Accessible name and `title`, e.g. `Select & move (V)`.
    #[must_use]
    pub fn name(&self) -> String {
        item_name(self.label, self.shortcut)
    }
}

pub const SHAPES_KEY: &str = "shapes";
pub const SHAPES_LABEL: &str = "Shapes";

/// Shipping `RAIL_TOOL_ITEMS` with the grouped Shapes button before Arrow.
/// The glass hover tip shows only `label`; `name()` is the accessible name.
pub const RAIL_TOOLS: [RailTool; 7] = [
    RailTool {
        key: "v",
        icon: "select",
        label: "Select & move",
        shortcut: Some("V"),
    },
    RailTool {
        key: "c",
        icon: "crop",
        label: "Crop",
        shortcut: Some("C"),
    },
    RailTool {
        key: "t",
        icon: "text",
        label: "Text",
        shortcut: Some("T"),
    },
    RailTool {
        key: SHAPES_KEY,
        icon: "shapes",
        label: SHAPES_LABEL,
        shortcut: None,
    },
    RailTool {
        key: "a",
        icon: "arrow",
        label: "Arrow",
        shortcut: Some("A"),
    },
    RailTool {
        key: "p",
        icon: "pen",
        label: "Freehand",
        shortcut: Some("P"),
    },
    RailTool {
        key: "b",
        icon: "remove-bg",
        label: "Eraser",
        shortcut: Some("B"),
    },
];

/// Shipping `SHAPE_GROUP_ITEMS`; `key` doubles as the icon name.
pub const SHAPE_TOOLS: [RailTool; 6] = [
    RailTool {
        key: "rectangle",
        icon: "rectangle",
        label: "Rectangle",
        shortcut: Some("R"),
    },
    RailTool {
        key: "ellipse",
        icon: "ellipse",
        label: "Ellipse",
        shortcut: Some("O"),
    },
    RailTool {
        key: "line",
        icon: "line",
        label: "Line",
        shortcut: Some("L"),
    },
    RailTool {
        key: "triangle",
        icon: "triangle",
        label: "Triangle",
        shortcut: None,
    },
    RailTool {
        key: "diamond",
        icon: "diamond",
        label: "Diamond",
        shortcut: Some("D"),
    },
    RailTool {
        key: "star",
        icon: "star",
        label: "Star",
        shortcut: Some("S"),
    },
];

fn item_name(label: &str, shortcut: Option<&str>) -> String {
    shortcut.map_or_else(|| label.to_owned(), |key| format!("{label} ({key})"))
}

/// The Shapes button `title`: `Shapes · Rectangle (R)` for the active or last
/// grouped shape. Unknown keys fall back to the first shape, like shipping.
#[must_use]
pub fn shapes_tooltip(current: &str) -> String {
    let item = SHAPE_TOOLS
        .iter()
        .find(|item| item.key == current)
        .unwrap_or(&SHAPE_TOOLS[0]);
    format!("{SHAPES_LABEL} · {}", item.name())
}

/// Properties heading for a tool (`toolLabel`): the rail/flyout label, or
/// shipping's `Properties` fallback. Background tools share `Eraser`.
#[must_use]
pub fn tool_label(key: &str) -> &'static str {
    match key {
        "wand" | "erase" | "restore" => "Eraser",
        "pen" | "freehand" => "Freehand",
        _ => RAIL_TOOLS
            .iter()
            .chain(SHAPE_TOOLS.iter())
            .find(|item| item.key == key && item.key != SHAPES_KEY)
            .map_or("Properties", |item| item.label),
    }
}

/// Layers panel copy.
pub mod layers {
    pub const TITLE: &str = "Layers";
    pub const ADD: &str = "Add image layer";
    pub const HIDE: &str = "Hide layer";
    pub const SHOW: &str = "Show layer";
    pub const LOCK: &str = "Lock layer";
    pub const UNLOCK: &str = "Unlock layer";
    pub const MENU: &str = "Layer settings and actions";
    pub const DRAG: &str = "Drag to reorder";
    pub const LOCKED: &str = "Layer is locked";
    pub const RENAME: &str = "Double-click to rename";
}

/// Shipping `elementLabel`: the Properties heading for a selected layer.
#[must_use]
pub fn element_label(element: &Element) -> String {
    match element {
        Element::Image(image) => image.name.clone(),
        Element::Text(_) => "Text".into(),
        Element::Path(_) => "Freehand drawing".into(),
        Element::Shape(shape) => {
            let mut chars = shape.shape.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

/// Shipping `elementLayerName`: text layers show their first line (up to 42
/// characters, after trimming), otherwise [`element_label`].
#[must_use]
pub fn layer_name(element: &Element) -> String {
    if let Element::Text(text) = element {
        let first = text.text.trim().split('\n').next().unwrap_or_default();
        let name: String = first.chars().take(42).collect();
        return if name.is_empty() { "Text".into() } else { name };
    }
    element_label(element)
}

/// Shipping `elementKindLabel`, the muted second line of a layer row.
#[must_use]
pub fn layer_kind(element: &Element) -> &'static str {
    match element {
        Element::Image(image) if image.source == "background" => {
            if image.base.locked {
                "Locked background"
            } else {
                "Background"
            }
        }
        Element::Image(_) => "Image",
        Element::Text(_) => "Text",
        Element::Path(_) => "Drawing",
        Element::Shape(_) => "Shape",
    }
}

/// Row accessible names: `Hide Name` / `Show Name`, `Lock Name` / `Unlock Name`.
#[must_use]
pub fn visibility_label(element: &Element) -> String {
    let verb = if element.base().visible {
        "Hide"
    } else {
        "Show"
    };
    format!("{verb} {}", layer_name(element))
}

#[must_use]
pub fn lock_label(element: &Element) -> String {
    let verb = if element.base().locked {
        "Unlock"
    } else {
        "Lock"
    };
    format!("{verb} {}", layer_name(element))
}

#[must_use]
pub fn menu_label(element: &Element) -> String {
    format!("Layer settings for {}", layer_name(element))
}

/// Icon shown in a layer row's preview well when the host has no thumbnail.
#[must_use]
pub fn layer_icon(element: &Element) -> &'static str {
    match element {
        Element::Image(_) => "image",
        Element::Text(_) => "text",
        Element::Path(_) => "pen",
        Element::Shape(shape) => match shape.shape.as_str() {
            "ellipse" => "ellipse",
            "line" => "line",
            "arrow" => "arrow",
            "triangle" => "triangle",
            "diamond" => "diamond",
            "star" => "star",
            _ => "rectangle",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn element(value: serde_json::Value) -> Element {
        serde_json::from_value(value).unwrap()
    }

    fn base(kind: &str, id: &str) -> serde_json::Value {
        json!({"kind": kind, "id": id, "x": 0., "y": 0., "locked": false, "visible": true,
               "opacity": 100., "blendMode": "source-over"})
    }

    #[test]
    fn header_hides_history_and_narrows_zoom_at_the_shipping_breakpoint() {
        assert_eq!(
            header_layout(1041.),
            HeaderLayout {
                show_history: true,
                zoom_slider_width: 92.,
                zoom_preset_width: 76.
            }
        );
        for width in [1040., 900., 760.] {
            assert_eq!(
                header_layout(width),
                HeaderLayout {
                    show_history: false,
                    zoom_slider_width: 72.,
                    zoom_preset_width: 72.
                }
            );
        }
    }

    #[test]
    fn rail_and_flyout_use_shipping_labels_order_and_names() {
        let names: Vec<_> = RAIL_TOOLS.iter().map(RailTool::name).collect();
        assert_eq!(
            names,
            [
                "Select & move (V)",
                "Crop (C)",
                "Text (T)",
                "Shapes",
                "Arrow (A)",
                "Freehand (P)",
                "Eraser (B)"
            ]
        );
        let shapes: Vec<_> = SHAPE_TOOLS.iter().map(RailTool::name).collect();
        assert_eq!(
            shapes,
            [
                "Rectangle (R)",
                "Ellipse (O)",
                "Line (L)",
                "Triangle",
                "Diamond (D)",
                "Star (S)"
            ]
        );
        assert_eq!(shapes_tooltip("triangle"), "Shapes · Triangle");
        assert_eq!(shapes_tooltip("unknown"), "Shapes · Rectangle (R)");
        for item in RAIL_TOOLS.iter().chain(SHAPE_TOOLS.iter()) {
            assert!(
                crate::icons::polylines(item.icon).is_some_and(|lines| !lines.is_empty()),
                "{} icon",
                item.icon
            );
        }
        assert_eq!(tool_label("c"), "Crop");
        assert_eq!(tool_label("star"), "Star");
        assert_eq!(tool_label("restore"), "Eraser");
        assert_eq!(tool_label(SHAPES_KEY), "Properties");
    }

    #[test]
    fn zoom_labels_and_offscreen_rule_match_shipping() {
        assert_eq!(zoom_label(100.), "100%");
        assert_eq!(zoom_label(98.94), "98.9%");
        assert_eq!(zoom_label(12.25), "12.3%");
        let viewport = Rect {
            x: 0.,
            y: 0.,
            width: 400.,
            height: 300.,
        };
        let canvas = |x, y| Rect {
            x,
            y,
            width: 200.,
            height: 100.,
        };
        assert!(!canvas_mostly_offscreen(viewport, canvas(100., 100.)));
        assert!(canvas_mostly_offscreen(viewport, canvas(500., 100.)));
        // 4% of 20,000 is 800 square points: a 5×100 sliver is lost, 10×100 is not.
        assert!(canvas_mostly_offscreen(viewport, canvas(395., 100.)));
        assert!(!canvas_mostly_offscreen(viewport, canvas(390., 100.)));
    }

    #[test]
    fn layer_rows_follow_shipping_names_and_kinds() {
        let mut image = base("image", "bg");
        image.as_object_mut().unwrap().extend(
            json!({"source": "background", "src": "", "name": "Screenshot", "width": 1.,
                   "height": 1., "naturalWidth": 1., "naturalHeight": 1.})
            .as_object()
            .unwrap()
            .clone(),
        );
        let background = element(image.clone());
        assert_eq!(layer_name(&background), "Screenshot");
        assert_eq!(layer_kind(&background), "Background");
        assert_eq!(layer_icon(&background), "image");
        image["locked"] = json!(true);
        assert_eq!(layer_kind(&element(image.clone())), "Locked background");
        assert_eq!(lock_label(&element(image.clone())), "Unlock Screenshot");
        image["source"] = json!("imported");
        assert_eq!(layer_kind(&element(image)), "Image");

        let mut shape = base("shape", "s");
        shape.as_object_mut().unwrap().extend(
            json!({"shape": "rectangle", "endX": 1., "endY": 1., "controls": [],
                   "style": {"color": "#fff", "fill": null, "strokeWidth": 1.}})
            .as_object()
            .unwrap()
            .clone(),
        );
        let shape = element(shape);
        assert_eq!(
            (layer_name(&shape), layer_kind(&shape)),
            ("Rectangle".into(), "Shape")
        );
        assert_eq!(visibility_label(&shape), "Hide Rectangle");
        assert_eq!(menu_label(&shape), "Layer settings for Rectangle");

        let mut path = base("path", "p");
        path.as_object_mut().unwrap().extend(
            json!({"points": [], "style": {"color": "#fff", "fill": null, "strokeWidth": 1.}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let path = element(path);
        assert_eq!(
            (layer_name(&path), layer_kind(&path)),
            ("Freehand drawing".into(), "Drawing")
        );
    }

    #[test]
    fn text_layers_show_their_trimmed_first_line_up_to_42_characters() {
        let text = |value: &str| {
            let mut text = base("text", "t");
            text.as_object_mut().unwrap().extend(
                json!({"text": value, "fontSize": 20., "width": 10., "fontFamily": "sans",
                       "bold": false, "italic": false, "align": "left", "color": "#fff",
                       "background": null, "outlined": false, "roundedBackground": false})
                .as_object()
                .unwrap()
                .clone(),
            );
            element(text)
        };
        assert_eq!(layer_name(&text("  Hello\nworld ")), "Hello");
        assert_eq!(layer_name(&text("   ")), "Text");
        assert_eq!(layer_name(&text(&"é".repeat(60))).chars().count(), 42);
        assert_eq!(element_label(&text("Hello")), "Text");
        assert_eq!(layer_kind(&text("Hello")), "Text");
    }
}
