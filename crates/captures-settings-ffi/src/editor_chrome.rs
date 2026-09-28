//! Shipping screenshot-editor chrome for AppKit, from `captures-app::editor_chrome`
//! (shared with the wgpu host): header, tool rail and Layers copy, icon names and
//! the responsive header rules. Requests are rare: one `copy` per editor window,
//! then small queries on resize, viewport changes or tool changes.
use super::region::{response, text};
use captures_app::{editor::Rect, editor_chrome as chrome};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum ChromeRequest {
    Copy,
    HeaderLayout {
        width: f64,
    },
    ShapesTooltip {
        current: String,
    },
    ToolLabel {
        key: String,
    },
    ZoomLabel {
        percent: f64,
    },
    CanvasOffscreen {
        viewport: [f64; 4],
        canvas: [f64; 4],
    },
    DrawToolPreview {
        tool: String,
        stroke_width: f64,
        stroke_enabled: bool,
    },
    BrushPreview {
        size: f64,
        softness: f64,
    },
    WandLoupePosition {
        cursor: [f64; 2],
        viewport: [f64; 2],
    },
    TextFace {
        family: String,
        /// The draft's font name for `family`; a draft's own font is not bundled.
        #[serde(default)]
        name: Option<String>,
        bold: bool,
        italic: bool,
    },
    /// An existing text layer (`element`) or the layer a Text click would
    /// create (`create`), for the host-drawn inline editor.
    InlineTextLayout {
        #[serde(default)]
        element: Option<Box<captures_app::editor::TextElement>>,
        #[serde(default)]
        create: Option<captures_app::editor_session::TextCreate>,
    },
}

fn tool(item: &chrome::RailTool) -> Value {
    json!({
        "key": item.key,
        "icon": item.icon,
        "label": item.label,
        "shortcut": item.shortcut,
        "name": item.name(),
    })
}

fn rect([x, y, width, height]: [f64; 4]) -> Result<Rect, String> {
    if [x, y, width, height].into_iter().all(f64::is_finite) && width >= 0. && height >= 0. {
        Ok(Rect {
            x,
            y,
            width,
            height,
        })
    } else {
        Err("invalid rect".into())
    }
}

fn copy() -> Value {
    use captures_app::editor_layers::geometry as g;
    use chrome::{colors as c, header as h, layers as l};
    json!({
        "metrics": {
            "header_height": chrome::HEADER_HEIGHT,
            "rail_width": chrome::RAIL_WIDTH,
            "rail_button": chrome::RAIL_BUTTON,
            "header_control": chrome::HEADER_CONTROL,
            "zoom_button": chrome::ZOOM_BUTTON,
            "canvas_field": chrome::CANVAS_FIELD,
            "shape_flyout_columns": chrome::SHAPE_FLYOUT_COLUMNS,
            "shape_flyout_button": chrome::SHAPE_FLYOUT_BUTTON,
            "compact_width": chrome::COMPACT_WIDTH,
            "draft_autosave_ms": captures_app::editor_session::DRAFT_AUTOSAVE_MS,
        },
        "rail": chrome::RAIL_TOOLS.iter().map(tool).collect::<Vec<_>>(),
        "shapes": chrome::SHAPE_TOOLS.iter().map(tool).collect::<Vec<_>>(),
        "header": {
            "canvas": h::CANVAS, "canvas_width": h::CANVAS_WIDTH,
            "canvas_height": h::CANVAS_HEIGHT, "trim": h::TRIM, "trim_tooltip": h::TRIM_TOOLTIP,
            "undo": h::UNDO, "redo": h::REDO, "zoom_group": h::ZOOM_GROUP,
            "fit": h::FIT, "fit_tooltip": h::FIT_TOOLTIP,
            "zoom_out": h::ZOOM_OUT, "zoom_in": h::ZOOM_IN,
            "zoom_slider": h::ZOOM_SLIDER, "zoom_slider_tooltip": h::ZOOM_SLIDER_TOOLTIP,
            "zoom_preset": h::ZOOM_PRESET, "zoom_preset_tooltip": h::ZOOM_PRESET_TOOLTIP,
            "zoom_presets": h::ZOOM_PRESETS,
            "add_images": h::ADD_IMAGES, "recenter": h::RECENTER,
            "draft_restored": h::DRAFT_RESTORED,
            "draft_discard": h::DRAFT_DISCARD, "draft_dismiss": h::DRAFT_DISMISS,
            "draft_dismiss_label": h::DRAFT_DISMISS_LABEL,
        },
        "colors": {
            "swatches": c::SWATCHES,
            "default_canvas_background": c::DEFAULT_CANVAS_BACKGROUND,
            "tile": c::TILE, "custom_inset": c::CUSTOM_INSET,
            "cell": c::CELL, "compact_cell": c::COMPACT_CELL, "menu_width": c::MENU_WIDTH,
            "background": c::BACKGROUND, "background_tooltip": c::BACKGROUND_TOOLTIP,
            "canvas_background": c::CANVAS_BACKGROUND, "solid_background": c::SOLID_BACKGROUND,
            "custom_color": c::CUSTOM_COLOR, "stroke_color": c::STROKE_COLOR,
            "fill_color": c::FILL_COLOR, "shadow_color": c::SHADOW_COLOR,
            "text_color": c::TEXT_COLOR, "color": c::COLOR,
        },
        "eraser": eraser(),
        "brush_cursor": brush_cursor(),
        "text_format": text_format(),
        "layers": {
            "title": l::TITLE, "add": l::ADD, "hide": l::HIDE, "show": l::SHOW,
            "lock": l::LOCK, "unlock": l::UNLOCK, "menu": l::MENU, "drag": l::DRAG,
            "locked": l::LOCKED, "rename": l::RENAME,
        },
        "layer_menu": layer_menu(),
        "layer_geometry": {
            "width": g::WIDTH, "height": g::HEIGHT, "width_label": g::WIDTH_LABEL,
            "height_label": g::HEIGHT_LABEL, "x_label": g::X_LABEL, "y_label": g::Y_LABEL,
            "max_size": g::MAX_SIZE, "proportional": g::PROPORTIONAL, "locked": g::LOCKED,
            "keeps_aspect": g::KEEPS_ASPECT,
        },
        "trim": trim(),
        "snap": snap(),
        "wand_loupe": wand_loupe(),
        "draw_preview": {
            "viewbox": chrome::draw_preview::VIEWBOX, "height": chrome::draw_preview::HEIGHT,
            "checker": chrome::draw_preview::CHECKER,
        },
        "blend_modes": captures_app::editor_layers::BLEND_MODES
            .iter()
            .map(|(value, label)| json!({"value": value, "label": label}))
            .collect::<Vec<_>>(),
    })
}

/// Eraser slider ranges, marks and hints (`editor_chrome::eraser`).
fn eraser() -> Value {
    use chrome::eraser as e;
    let marks = |marks: &[(f64, &str)]| {
        marks
            .iter()
            .map(|(value, label)| json!({"value": value, "label": label}))
            .collect::<Vec<_>>()
    };
    json!({
        "intro": e::INTRO,
        "tolerance": e::TOLERANCE, "tolerance_label": e::TOLERANCE_LABEL,
        "tolerance_range": [e::TOLERANCE_RANGE.0, e::TOLERANCE_RANGE.1],
        "tolerance_marks": marks(&e::TOLERANCE_MARKS),
        "contiguous": e::CONTIGUOUS,
        "wand_contiguous_hint": e::WAND_CONTIGUOUS_HINT,
        "wand_everywhere_hint": e::WAND_EVERYWHERE_HINT,
        "size": e::SIZE, "size_label": e::SIZE_LABEL,
        "size_range": [e::SIZE_RANGE.0, e::SIZE_RANGE.1], "size_marks": marks(&e::SIZE_MARKS),
        "softness": e::SOFTNESS, "softness_label": e::SOFTNESS_LABEL,
        "softness_range": [e::SOFTNESS_RANGE.0, e::SOFTNESS_RANGE.1],
        "softness_marks": marks(&e::SOFTNESS_MARKS),
        "erase_hint": e::ERASE_HINT, "restore_hint": e::RESTORE_HINT,
    })
}

/// Erase/Restore ring paint (`editor_chrome::brush_cursor`).
fn brush_cursor() -> Value {
    use chrome::brush_cursor as b;
    json!({
        "ring_width": b::RING_WIDTH, "ring_rgba": b::RING_RGBA,
        "erase_fill_rgba": b::ERASE_FILL_RGBA,
        "restore_fill_accent_alpha": b::RESTORE_FILL_ACCENT_ALPHA,
        "restore_dash": b::RESTORE_DASH,
        "halo_width": b::HALO_WIDTH, "halo_rgba": b::HALO_RGBA,
        "inset_width": b::INSET_WIDTH, "inset_rgba": b::INSET_RGBA,
    })
}

/// Bold/Italic and alignment buttons (`editor_chrome::text_format`).
fn text_format() -> Value {
    use chrome::text_format as t;
    json!({
        "inline_label": t::INLINE_LABEL, "bold": t::BOLD, "italic": t::ITALIC,
        "align": t::ALIGN
            .iter()
            .map(|(value, label, icon)| json!({"value": value, "label": label, "icon": icon}))
            .collect::<Vec<_>>(),
        "columns": t::COLUMNS, "button_height": t::BUTTON_HEIGHT, "icon": t::ICON,
    })
}

/// Trim edges hover preview paint (`.screenshot-canvas-trim-*`).
fn trim() -> Value {
    use captures_app::editor_canvas as c;
    json!({
        "rgb": c::TRIM_RGB, "region_alpha": c::TRIM_REGION_ALPHA,
        "keep_alpha": c::TRIM_KEEP_ALPHA, "keep_width": c::TRIM_KEEP_WIDTH,
        "keep_radius": c::TRIM_KEEP_RADIUS, "keep_glow_alpha": c::TRIM_KEEP_GLOW_ALPHA,
        "edge_bar": c::TRIM_EDGE_BAR, "bloom": c::TRIM_BLOOM, "bloom_stops": c::TRIM_BLOOM_STOPS,
    })
}

/// Image-drop snap and Expand canvas edge paint (`.screenshot-drop-snap-*`,
/// `.screenshot-canvas-expand-*`).
fn snap() -> Value {
    use captures_app::editor_canvas as c;
    json!({
        "bloom": c::SNAP_BLOOM, "bloom_fraction": c::SNAP_BLOOM_FRACTION,
        "bloom_overhang": c::SNAP_BLOOM_OVERHANG, "bloom_stops": c::SNAP_BLOOM_STOPS,
        "edge_bar": c::SNAP_EDGE_BAR,
    })
}

/// Remove-background wand loupe metrics (`WandColorLoupe`).
fn wand_loupe() -> Value {
    use captures_app::editor_image_background as w;
    let (dark, light, cell) = w::WAND_LOUPE_CHECKER;
    json!({
        "size": w::WAND_LOUPE_SIZE, "offset": w::WAND_LOUPE_OFFSET,
        "margin": w::WAND_LOUPE_MARGIN, "extent": w::WAND_LOUPE_SAMPLE_EXTENT,
        "checker_dark": dark, "checker_light": light, "checker_cell": cell,
        "grid_alpha": w::WAND_LOUPE_GRID_ALPHA, "meta_gap": w::WAND_LOUPE_META_GAP,
    })
}

fn layer_menu() -> Value {
    use captures_app::editor_layers::menu as m;
    json!({
        "appearance": m::APPEARANCE, "blend_mode": m::BLEND_MODE, "opacity": m::OPACITY,
        "opacity_label": m::OPACITY_LABEL, "transform": m::TRANSFORM, "arrange": m::ARRANGE,
        "combine": m::COMBINE, "rotate_left": m::ROTATE_LEFT, "rotate_right": m::ROTATE_RIGHT,
        "flip_horizontal": m::FLIP_HORIZONTAL, "flip_vertical": m::FLIP_VERTICAL,
        "rotate_left_tip": m::ROTATE_LEFT_TIP, "rotate_right_tip": m::ROTATE_RIGHT_TIP,
        "flip_horizontal_tip": m::FLIP_HORIZONTAL_TIP, "flip_vertical_tip": m::FLIP_VERTICAL_TIP,
        "bring_front": m::BRING_FRONT, "send_back": m::SEND_BACK,
        "bring_front_tip": m::BRING_FRONT_TIP, "send_back_tip": m::SEND_BACK_TIP,
        "merge_down": m::MERGE_DOWN, "merge_visible": m::MERGE_VISIBLE, "flatten": m::FLATTEN,
        "merge_down_tip": m::MERGE_DOWN_TIP, "merge_visible_tip": m::MERGE_VISIBLE_TIP,
        "flatten_tip": m::FLATTEN_TIP, "duplicate": m::DUPLICATE, "delete": m::DELETE,
        "duplicate_tip": m::DUPLICATE_TIP, "delete_tip": m::DELETE_TIP,
        "rename_label": m::RENAME_LABEL,
    })
}

fn handle(request: ChromeRequest) -> Result<Value, String> {
    Ok(match request {
        ChromeRequest::Copy => copy(),
        ChromeRequest::HeaderLayout { width } => {
            if !width.is_finite() {
                return Err("invalid width".into());
            }
            let layout = chrome::header_layout(width);
            json!({
                "show_history": layout.show_history,
                "zoom_slider_width": layout.zoom_slider_width,
                "zoom_preset_width": layout.zoom_preset_width,
            })
        }
        ChromeRequest::ShapesTooltip { current } => json!(chrome::shapes_tooltip(&current)),
        ChromeRequest::ToolLabel { key } => json!(chrome::tool_label(&key)),
        ChromeRequest::ZoomLabel { percent } => {
            if !percent.is_finite() {
                return Err("invalid percent".into());
            }
            json!(chrome::zoom_label(percent))
        }
        ChromeRequest::CanvasOffscreen { viewport, canvas } => {
            json!(chrome::canvas_mostly_offscreen(
                rect(viewport)?,
                rect(canvas)?
            ))
        }
        ChromeRequest::DrawToolPreview {
            tool,
            stroke_width,
            stroke_enabled,
        } => {
            if !stroke_width.is_finite() {
                return Err("invalid stroke width".into());
            }
            json!(chrome::draw_preview::stroke(
                &tool,
                stroke_width,
                stroke_enabled
            ))
        }
        ChromeRequest::BrushPreview { size, softness } => {
            if !size.is_finite() || !softness.is_finite() {
                return Err("invalid brush".into());
            }
            json!(chrome::draw_preview::brush(size, softness))
        }
        ChromeRequest::InlineTextLayout { element, create } => {
            let element = match (element, create) {
                (Some(element), None) => *element,
                (None, Some(create)) => {
                    captures_app::editor_session::new_text_element(String::new(), &create)?
                }
                _ => return Err("Pass exactly one of element or create.".into()),
            };
            let layout = captures_app::editor_text::inline_editor_layout(&element)?;
            json!({"element": element, "layout": layout})
        }
        ChromeRequest::TextFace {
            family,
            name,
            bold,
            italic,
        } => {
            use captures_app::editor_fonts as fonts;
            if name.is_some_and(|name| !fonts::is_bundled_family(&family, &name)) {
                Value::Null
            } else {
                json!(fonts::bundled_face_base64(&family, bold, italic))
            }
        }
        ChromeRequest::WandLoupePosition { cursor, viewport } => {
            if !cursor
                .iter()
                .chain(&viewport)
                .all(|value| value.is_finite())
            {
                return Err("invalid loupe position".into());
            }
            json!(captures_app::editor_image_background::wand_loupe_position(
                captures_app::editor::Point {
                    x: cursor[0],
                    y: cursor[1],
                },
                viewport[0],
                viewport[1],
            ))
        }
    })
}

/// Screenshot-editor chrome copy and policy. `request_json` is one of
/// `{"operation":"copy"}`, `header_layout {width}`, `shapes_tooltip {current}`,
/// `tool_label {key}`, `zoom_label {percent}`,
/// `canvas_offscreen {viewport:[x,y,w,h], canvas:[x,y,w,h]}`,
/// `draw_tool_preview {tool, stroke_width, stroke_enabled}`,
/// `brush_preview {size, softness}`,
/// `wand_loupe_position {cursor:[x,y], viewport:[w,h]}` or
/// `text_face {family, name?, bold, italic}` (base64 font bytes, or null for a
/// family this build does not bundle) or `inline_text_layout {element | create}`
/// (`{element, layout}` for the inline text editor). Returns the owned
/// `{ok,result}` / `{ok,error}` envelope; free with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_chrome_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<ChromeRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        handle(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures_settings_free_v1;
    use std::ffi::{CStr, CString};

    fn call(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: input is a live NUL-terminated string for this call.
        let output = unsafe { captures_editor_chrome_v1(input.as_ptr()) };
        // SAFETY: output is an owned response string freed exactly once below.
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(output) };
        value
    }

    #[test]
    fn header_declares_editor_chrome_abi() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("char *captures_editor_chrome_v1(const char *request_json);"));
    }

    #[test]
    fn copy_carries_rail_shapes_header_and_layers() {
        let copy = call(json!({"operation": "copy"}));
        assert_eq!(copy["ok"], true);
        let result = &copy["result"];
        assert_eq!(result["rail"][6]["label"], "Eraser");
        assert_eq!(result["blend_modes"][0]["value"], "source-over");
        assert_eq!(result["blend_modes"][5]["label"], "Lighten");
        assert_eq!(result["layer_menu"]["bring_front"], "Bring to front");
        assert_eq!(result["layer_geometry"]["x_label"], "Layer X");
        assert_eq!(result["rail"][6]["name"], "Eraser (B)");
        assert_eq!(result["rail"][3]["shortcut"], Value::Null);
        assert_eq!(result["shapes"][3]["name"], "Triangle");
        assert_eq!(result["header"]["fit_tooltip"], "Fit canvas to window");
        assert_eq!(result["header"]["zoom_presets"], json!([50., 100., 200.]));
        assert_eq!(result["layers"]["menu"], "Layer settings and actions");
        assert_eq!(result["metrics"]["compact_width"], 1040.);
        assert_eq!(result["metrics"]["draft_autosave_ms"], 700);
        assert_eq!(result["header"]["draft_discard"], "Discard");
        assert!(result["header"].get("save_draft").is_none());
        let colors = &result["colors"];
        assert_eq!(colors["swatches"], json!(chrome::colors::SWATCHES));
        assert_eq!(colors["swatches"][0], "#ff3b5c");
        assert_eq!(colors["default_canvas_background"], "#f7f7f5");
        assert_eq!(colors["solid_background"], "Solid background");
        assert_eq!(colors["custom_color"], "Custom color");
        assert_eq!(colors["compact_cell"], 36.);
        assert_eq!(colors["menu_width"], 248.);
        assert_eq!(colors["text_color"], "Text color");
        assert_eq!(colors["color"], "Color");
    }

    #[test]
    fn copy_carries_eraser_sliders_and_text_format_buttons() {
        let result = &call(json!({"operation": "copy"}))["result"];
        let eraser = &result["eraser"];
        assert_eq!(eraser["tolerance_range"], json!([0., 120.]));
        assert_eq!(
            eraser["tolerance_marks"][1],
            json!({"value": 36., "label": "36"})
        );
        assert_eq!(eraser["softness_marks"][0]["label"], "Hard");
        assert_eq!(eraser["softness_marks"][2]["label"], "Soft");
        assert_eq!(eraser["size_label"], "Brush size");
        assert_eq!(
            eraser["wand_everywhere_hint"],
            "Click a color to remove it everywhere in the layer."
        );
        assert_eq!(eraser["restore_hint"], "Paint to put back what you erased.");
        let ring = &result["brush_cursor"];
        assert_eq!(ring["ring_width"], 1.5);
        assert_eq!(ring["ring_rgba"], json!([1., 1., 1., 0.92]));
        assert_eq!(ring["halo_rgba"][3], 0.55);
        assert_eq!(ring["restore_fill_accent_alpha"], 0.08);
        let format = &result["text_format"];
        assert_eq!(format["bold"], "Bold");
        assert_eq!(format["inline_label"], "Edit text on canvas");
        assert_eq!(
            format["align"][2],
            json!({"value": "right", "label": "Align right", "icon": "align-right"})
        );
        assert_eq!(format["columns"], 5);
    }

    #[test]
    fn draw_previews_trim_paint_and_wand_loupe_round_trip() {
        let result = &call(json!({"operation": "copy"}))["result"];
        assert_eq!(result["trim"]["rgb"], json!([255, 92, 106]));
        assert_eq!(result["trim"]["keep_width"], 1.5);
        assert_eq!(result["snap"]["bloom"], 96.);
        assert_eq!(result["snap"]["edge_bar"], 5.);
        assert_eq!(result["snap"]["bloom_stops"][0], json!([0., 0.55]));
        assert_eq!(result["wand_loupe"]["size"], 84.);
        assert_eq!(result["wand_loupe"]["extent"], 11);
        assert_eq!(result["draw_preview"]["height"], 88.);
        let stroke = call(
            json!({"operation": "draw_tool_preview", "tool": "rectangle",
            "stroke_width": 8., "stroke_enabled": true}),
        );
        assert_eq!(stroke["result"]["label"], "Stroke preview");
        assert_eq!(stroke["result"]["shapes"][0]["kind"], "rounded_rect");
        assert_eq!(stroke["result"]["shapes"][0]["radius"], 6.);
        let brush = call(json!({"operation": "brush_preview", "size": 120., "softness": 0.}));
        assert_eq!(brush["result"]["brush"]["radius"], 30.);
        assert_eq!(brush["result"]["label"], "Brush preview");
        let position = call(json!({"operation": "wand_loupe_position",
            "cursor": [40., 50.], "viewport": [800., 600.]}));
        assert_eq!(position["result"], json!({"x": 58., "y": 68.}));
        assert_eq!(
            call(json!({"operation": "brush_preview", "size": null, "softness": 0.}))["ok"],
            false
        );
    }

    #[test]
    fn queries_round_trip_and_reject_invalid_input() {
        let narrow = call(json!({"operation": "header_layout", "width": 900.}));
        assert_eq!(
            narrow["result"],
            json!({"show_history": false, "zoom_slider_width": 72., "zoom_preset_width": 72.})
        );
        assert_eq!(
            call(json!({"operation": "shapes_tooltip", "current": "star"}))["result"],
            "Shapes · Star (S)"
        );
        assert_eq!(
            call(json!({"operation": "tool_label", "key": "wand"}))["result"],
            "Eraser"
        );
        assert_eq!(
            call(json!({"operation": "zoom_label", "percent": 12.25}))["result"],
            "12.3%"
        );
        assert_eq!(
            call(json!({"operation": "canvas_offscreen",
                        "viewport": [0., 0., 400., 300.], "canvas": [500., 0., 100., 100.]}))["result"],
            true
        );
        assert_eq!(
            call(json!({"operation": "canvas_offscreen",
                        "viewport": [0., 0., 400., 300.], "canvas": [0., 0., -1., 1.]}))["ok"],
            false
        );
        assert_eq!(call(json!({"operation": "nope"}))["ok"], false);
        let created = &call(json!({"operation": "inline_text_layout", "create": {
            "point": {"x": 200., "y": 80.}, "text": "", "fontSize": 20., "fontFamily": "sans",
            "color": "#2d9cff", "stylePreset": "rounded-box"}}))["result"];
        assert_eq!(created["element"]["fontFamily"], "rounded");
        assert_eq!(created["element"]["x"], 120.);
        assert!(created["layout"]["plate_radius"].as_f64().unwrap() > 0.);
        let existing = &call(json!({"operation": "inline_text_layout",
            "element": created["element"].clone()}))["result"];
        assert_eq!(existing["layout"], created["layout"]);
        assert_eq!(
            call(json!({"operation": "inline_text_layout"}))["ok"],
            false
        );
        let face = call(json!({"operation": "text_face", "family": "rounded",
                               "bold": true, "italic": false}));
        assert!(
            face["result"]
                .as_str()
                .is_some_and(|encoded| encoded.len() > 1000)
        );
        assert!(
            call(json!({"operation": "text_face", "family": "Draft Font",
                        "bold": false, "italic": false}))["result"]
                .is_null()
        );
        assert!(
            call(json!({"operation": "text_face", "family": "sans",
                        "name": "Captures Shaping Test", "bold": false, "italic": false}))["result"]
                .is_null(),
            "a draft's own font under a bundled key is not the bundled face"
        );
        // SAFETY: Null input is reported as an error envelope.
        let null = unsafe { captures_editor_chrome_v1(std::ptr::null()) };
        // SAFETY: owned response, read then freed once.
        let value: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(null) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(null) };
        assert_eq!(value["ok"], false);
    }
}
