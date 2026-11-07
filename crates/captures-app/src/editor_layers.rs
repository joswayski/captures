//! Shared Layers-panel logic for the native screenshot editors: blend mode
//! options, the layer settings popover copy, aspect-preserving image sizes and
//! the live row thumbnails (shipping `.screenshot-layer-preview`).

use std::collections::BTreeMap;
use std::sync::Arc;

use image::RgbaImage;

use crate::editor::{Document, Element, ImageElement, ImageOrientation};

/// Shipping `LAYER_BLEND_MODE_OPTIONS`: canvas composite value and label.
pub const BLEND_MODES: [(&str, &str); 6] = [
    ("source-over", "Normal"),
    ("multiply", "Multiply"),
    ("screen", "Screen"),
    ("overlay", "Overlay"),
    ("darken", "Darken"),
    ("lighten", "Lighten"),
];

/// The label for a stored blend mode; unknown values read as Normal.
#[must_use]
pub fn blend_label(value: &str) -> &'static str {
    BLEND_MODES
        .iter()
        .find(|(key, _)| *key == value)
        .map_or("Normal", |(_, label)| label)
}

/// Shipping `.screenshot-layer-menu-panel` copy.
pub mod menu {
    pub const APPEARANCE: &str = "Appearance";
    pub const BLEND_MODE: &str = "Blend mode";
    pub const OPACITY: &str = "Opacity";
    pub const OPACITY_LABEL: &str = "Layer opacity";
    pub const TRANSFORM: &str = "Transform";
    pub const ARRANGE: &str = "Arrange";
    pub const COMBINE: &str = "Combine";
    pub const ROTATE_LEFT: &str = "Rotate left";
    pub const ROTATE_RIGHT: &str = "Rotate right";
    pub const FLIP_HORIZONTAL: &str = "Flip horizontal";
    pub const FLIP_VERTICAL: &str = "Flip vertical";
    pub const ROTATE_LEFT_TIP: &str = "Rotate this image layer 90° counterclockwise";
    pub const ROTATE_RIGHT_TIP: &str = "Rotate this image layer 90° clockwise";
    pub const FLIP_HORIZONTAL_TIP: &str = "Mirror this image layer from left to right";
    pub const FLIP_VERTICAL_TIP: &str = "Mirror this image layer from top to bottom";
    pub const BRING_FRONT: &str = "Bring to front";
    pub const SEND_BACK: &str = "Send to back";
    pub const BRING_FRONT_TIP: &str = "Move this layer above every other layer";
    pub const SEND_BACK_TIP: &str = "Move this layer below every other layer";
    pub const MERGE_DOWN: &str = "Merge down";
    pub const MERGE_VISIBLE: &str = "Merge visible";
    pub const FLATTEN: &str = "Flatten image";
    pub const MERGE_DOWN_TIP: &str =
        "Rasterize this layer together with the unlocked layer directly under it";
    pub const MERGE_VISIBLE_TIP: &str =
        "Rasterize every visible layer into one image; hidden layers stay";
    pub const FLATTEN_TIP: &str = "Bake the canvas background and visible layers into one locked background layer; discard hidden layers";
    pub const DUPLICATE: &str = "Duplicate";
    pub const DELETE: &str = "Delete";
    pub const DUPLICATE_TIP: &str = "Duplicate this layer (Command/Ctrl+D)";
    pub const DELETE_TIP: &str = "Delete this layer";
    pub const RENAME_LABEL: &str = "Rename layer";

    /// Shipping `Layer settings for {name}` popover and trigger name.
    #[must_use]
    pub fn settings_label(name: &str) -> String {
        format!("Layer settings for {name}")
    }
}

/// Shipping inspector geometry copy (`.screenshot-number-pair`).
pub mod geometry {
    pub const WIDTH: &str = "Width";
    pub const HEIGHT: &str = "Height";
    pub const WIDTH_LABEL: &str = "Layer width";
    pub const HEIGHT_LABEL: &str = "Layer height";
    pub const X_LABEL: &str = "Layer X";
    pub const Y_LABEL: &str = "Layer Y";
    pub const MAX_SIZE: f64 = 16_384.;
    pub const PROPORTIONAL: &str = "Width and height stay proportional to the image.";
    pub const LOCKED: &str = "Unlock this layer to change size and position.";
    pub const KEEPS_ASPECT: &str = "Keeps the image aspect ratio";
}

fn swaps_axes(orientation: Option<ImageOrientation>) -> bool {
    matches!(
        orientation,
        Some(
            ImageOrientation::Rotate90
                | ImageOrientation::Rotate270
                | ImageOrientation::Transpose
                | ImageOrientation::Transverse
        )
    )
}

fn oriented_natural(image: &ImageElement) -> (f64, f64) {
    let (width, height) = (image.natural_width.max(1.), image.natural_height.max(1.));
    if swaps_axes(image.orientation) {
        (height, width)
    } else {
        (width, height)
    }
}

/// Shipping `imageSizeAtWidth`: whole pixels, at least 1, natural aspect.
#[must_use]
pub fn image_size_at_width(image: &ImageElement, width: f64) -> (f64, f64) {
    let width = width.round().max(1.);
    let (natural_width, natural_height) = oriented_natural(image);
    (
        width,
        (width * natural_height / natural_width).round().max(1.),
    )
}

/// Shipping `imageSizeAtHeight`: whole pixels, at least 1, natural aspect.
#[must_use]
pub fn image_size_at_height(image: &ImageElement, height: f64) -> (f64, f64) {
    let height = height.round().max(1.);
    let (natural_width, natural_height) = oriented_natural(image);
    (
        (height * natural_width / natural_height).round().max(1.),
        height,
    )
}

/// Whether Bring to front (`front`) or Send to back would move this layer:
/// shipping disables them for locked layers and the layer already there.
#[must_use]
pub fn can_arrange(document: &Document, id: &str, front: bool) -> bool {
    let Some(element) = document
        .elements
        .iter()
        .find(|element| element.base().id == id)
    else {
        return false;
    };
    let edge = if front {
        document.elements.last()
    } else {
        document.elements.first()
    };
    !element.base().locked && edge.is_some_and(|edge| edge.base().id != id)
}

/// Longest side of a rendered row thumbnail in pixels (shipping's 32 px
/// preview box at 2x).
pub const THUMBNAIL_SIZE: u32 = 64;

/// Shipping AnnotationLayerPreview's logical canvas, with 3 px containment
/// padding. Native hosts receive this whole canvas at 2x, not a tight crop.
pub const ANNOTATION_PREVIEW_SIZE: (u32, u32) = (46, 34);

/// One layer's live thumbnail, cached until the layer changes.
#[derive(Clone, Debug)]
pub struct LayerThumbnail {
    element: Element,
    pub image: Arc<RgbaImage>,
    /// `data:image/png;base64,…` for hosts that receive the snapshot as JSON.
    pub data_url: String,
}

/// Row thumbnails by layer ID. [`ThumbnailCache::refresh`] re-renders only
/// layers whose element changed and drops layers that no longer exist.
#[derive(Clone, Debug, Default)]
pub struct ThumbnailCache {
    entries: BTreeMap<String, LayerThumbnail>,
}

impl ThumbnailCache {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&LayerThumbnail> {
        self.entries.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &LayerThumbnail)> {
        self.entries.iter().map(|(id, entry)| (id.as_str(), entry))
    }

    /// Render missing or stale thumbnails with `render`, which composites a
    /// one-element document with the given geometry scale. The document's
    /// dimensions are output pixels. A layer that fails to render keeps no
    /// thumbnail; hosts fall back to the kind icon.
    pub fn refresh(
        &mut self,
        document: &Document,
        mut render: impl FnMut(&Document, f64) -> Result<RgbaImage, String>,
    ) {
        self.entries.retain(|id, _| {
            document
                .elements
                .iter()
                .any(|element| &element.base().id == id)
        });
        for element in &document.elements {
            let id = &element.base().id;
            if self
                .entries
                .get(id)
                .is_some_and(|entry| thumbnail_key_matches(&entry.element, element))
            {
                continue;
            }
            match render_thumbnail(element, &mut render) {
                Some(image) => {
                    let data_url = png_data_url(&image).unwrap_or_default();
                    self.entries.insert(
                        id.clone(),
                        LayerThumbnail {
                            element: element.clone(),
                            image: Arc::new(image),
                            data_url,
                        },
                    );
                }
                None => {
                    self.entries.remove(id);
                }
            }
        }
    }
}

/// Visibility, lock and blend do not change shipping's preview. Image rows use
/// the source image without layer opacity; annotation canvases keep opacity.
fn thumbnail_key_matches(cached: &Element, current: &Element) -> bool {
    normalized(cached) == normalized(current)
}

fn normalized(element: &Element) -> Element {
    let mut element = element.clone();
    let base = element.base_mut();
    base.visible = true;
    base.locked = false;
    "source-over".clone_into(&mut base.blend_mode);
    if let Element::Image(image) = &mut element {
        image.base.opacity = 100.;
        image.name.clear();
    }
    element
}

fn render_thumbnail(
    element: &Element,
    render: &mut impl FnMut(&Document, f64) -> Result<RgbaImage, String>,
) -> Option<RgbaImage> {
    let bounds = element.painted_bounds().ok()?;
    if !(bounds.width.is_finite() && bounds.height.is_finite())
        || bounds.width <= 0.
        || bounds.height <= 0.
    {
        return None;
    }
    let mut element = normalized(element);
    if !matches!(element, Element::Image(_)) {
        let (width, height) = ANNOTATION_PREVIEW_SIZE;
        let scale = ((f64::from(width) - 6.) / bounds.width.max(1.))
            .min((f64::from(height) - 6.) / bounds.height.max(1.));
        // Compute the transform from authored bounds BEFORE widening strokes,
        // exactly as previewTransformForBounds + withPreviewStrokeFloor do.
        let style = match &mut element {
            Element::Shape(shape) => Some(&mut shape.style),
            Element::Path(path) => Some(&mut path.style),
            _ => None,
        };
        if let Some(style) = style {
            style.stroke_width = style.stroke_width.max(1.35 / scale);
        }
        element
            .translate_layer(
                (f64::from(width) / scale - bounds.width) / 2. - bounds.x,
                (f64::from(height) / scale - bounds.height) / 2. - bounds.y,
            )
            .ok()?;
        return render(
            &Document {
                width: f64::from(width * 2),
                height: f64::from(height * 2),
                background: None,
                elements: vec![element],
                extra: serde_json::Map::new(),
            },
            scale * 2.,
        )
        .ok();
    }
    element.translate_layer(-bounds.x, -bounds.y).ok()?;
    let document = Document {
        width: bounds.width.ceil().max(1.),
        height: bounds.height.ceil().max(1.),
        background: None,
        elements: vec![element],
        extra: serde_json::Map::new(),
    };
    let frame = render(&document, 1.).ok()?;
    let (width, height) = frame.dimensions();
    let scale = (f64::from(THUMBNAIL_SIZE) / f64::from(width.max(height))).min(1.);
    let target = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    Some(if scale < 1. {
        image::imageops::resize(
            &frame,
            target(width),
            target(height),
            image::imageops::FilterType::Triangle,
        )
    } else {
        frame
    })
}

fn png_data_url(image: &RgbaImage) -> Option<String> {
    let png = captures_history::encode_png(image).ok()?;
    Some(format!("data:image/png;base64,{}", base64(&png)))
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document(value: serde_json::Value) -> Document {
        serde_json::from_value(value).unwrap()
    }

    fn image(id: &str, locked: bool) -> serde_json::Value {
        json!({"kind": "image", "id": id, "x": 0., "y": 0., "locked": locked, "visible": true,
               "opacity": 100., "blendMode": "source-over", "source": "imported", "src": id,
               "name": id, "sourceArtifactId": null, "width": 40., "height": 20.,
               "naturalWidth": 400., "naturalHeight": 200.})
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
    }

    #[test]
    fn image_sizes_keep_the_oriented_natural_aspect() {
        let doc = document(json!({"width": 100., "height": 100., "background": null,
                                  "elements": [image("a", false)]}));
        let Element::Image(mut element) = doc.elements[0].clone() else {
            unreachable!()
        };
        assert_eq!(image_size_at_width(&element, 800.), (800., 400.));
        assert_eq!(image_size_at_height(&element, 50.4), (100., 50.));
        assert_eq!(image_size_at_width(&element, 0.), (1., 1.));
        element.orientation = Some(ImageOrientation::Rotate90);
        assert_eq!(image_size_at_width(&element, 200.), (200., 400.));
    }

    #[test]
    fn arrange_is_disabled_for_locked_layers_and_the_edge_layer() {
        let doc = document(json!({"width": 100., "height": 100., "background": null,
            "elements": [image("back", true), image("middle", false), image("front", false)]}));
        assert!(can_arrange(&doc, "middle", true));
        assert!(can_arrange(&doc, "middle", false));
        assert!(!can_arrange(&doc, "front", true));
        assert!(can_arrange(&doc, "front", false));
        assert!(!can_arrange(&doc, "back", true));
        assert!(!can_arrange(&doc, "missing", true));
        assert_eq!(blend_label("screen"), "Screen");
        assert_eq!(blend_label("bogus"), "Normal");
    }

    #[test]
    fn annotation_previews_keep_readable_strokes_and_authored_opacity_without_editing_layers() {
        // Asymmetric document coordinates catch scaling without translation.
        // Both paths and shapes must retain 1.35 logical px at 2x resolution.
        for kind in ["shape", "path"] {
            let element = json!({"kind": kind, "id": "a", "x": 37., "y": 19.,
                "visible": true, "locked": false, "opacity": 100., "blendMode": "source-over",
                "shape": "line", "endX": 437., "endY": 19., "controls": [],
                "points": [{"x": 37., "y": 19.}, {"x": 437., "y": 19.}],
                "style": {"color": "#ff3b5c", "fill": null, "strokeWidth": 1.,
                    "strokeEnabled": true, "dropShadow": false}});
            let mut doc = document(json!({"width": 500., "height": 100.,
                "background": null, "elements": [element]}));
            let original = doc.clone();
            let mut cache = ThumbnailCache::default();
            let render = |doc: &Document, scale| {
                crate::editor_render::render_thumbnail_frame(doc, &BTreeMap::new(), None, scale)
            };
            let coverage = |image: &RgbaImage| {
                (0..image.height())
                    .map(|y| f64::from(image.get_pixel(46, y)[3]) / 255.)
                    .sum::<f64>()
            };
            cache.refresh(&doc, render);
            let full = cache.get("a").unwrap().image.clone();
            assert_eq!(full.dimensions(), (92, 68));
            assert!(
                (coverage(&full) - 2.7).abs() < 0.12,
                "{kind}: {}",
                coverage(&full)
            );
            assert_eq!(
                doc, original,
                "thumbnail widening must not edit the document"
            );
            assert!(
                full.enumerate_pixels()
                    .all(|(x, y, p)| (x > 0 && x < 91 && y > 0 && y < 67) || p[3] == 0)
            );

            doc.elements[0].base_mut().opacity = 25.;
            cache.refresh(&doc, render);
            let quarter = cache.get("a").unwrap().image.clone();
            assert!((coverage(&quarter) - 0.675).abs() < 0.08);
            assert!(quarter.pixels().all(|p| p[3] <= 64));
            doc.elements[0].base_mut().visible = false;
            cache.refresh(&doc, render);
            assert_eq!(
                *cache.get("a").unwrap().image,
                *quarter,
                "host visibility fade must not hide or double-dim the annotation"
            );

            // A thick authored stroke must not be replaced by the minimum.
            doc.elements[0].base_mut().opacity = 100.;
            match &mut doc.elements[0] {
                Element::Shape(s) => s.style.stroke_width = 40.,
                Element::Path(p) => p.style.stroke_width = 40.,
                _ => unreachable!(),
            }
            cache.refresh(&doc, render);
            assert!(coverage(&cache.get("a").unwrap().image) > 6.);
        }
    }

    #[test]
    fn thumbnails_render_each_layer_alone_and_rerender_only_on_change() {
        let mut doc = document(json!({"width": 100., "height": 100., "background": "#fff",
            "elements": [image("a", false), image("b", false)]}));
        let mut cache = ThumbnailCache::default();
        let renders = std::cell::RefCell::new(Vec::new());
        let render = |document: &Document, scale: f64| {
            assert_eq!(scale, 1.);
            renders
                .borrow_mut()
                .push((document.width, document.height, document.elements.len()));
            assert!(document.background.is_none());
            assert_eq!(document.elements[0].base().x, 0.);
            Ok(RgbaImage::new(
                document.width as u32 * 4,
                document.height as u32 * 4,
            ))
        };
        cache.refresh(&doc, render);
        assert_eq!(*renders.borrow(), [(40., 20., 1), (40., 20., 1)]);
        let thumbnail = cache.get("a").unwrap();
        assert_eq!(thumbnail.image.dimensions(), (64, 32));
        assert!(thumbnail.data_url.starts_with("data:image/png;base64,"));
        // Visibility and opacity do not change the preview.
        doc.elements[0].base_mut().visible = false;
        doc.elements[0].base_mut().opacity = 10.;
        renders.borrow_mut().clear();
        cache.refresh(&doc, render);
        assert!(renders.borrow().is_empty());
        doc.elements[1].base_mut().x = 30.;
        doc.elements.remove(0);
        cache.refresh(&doc, render);
        assert_eq!(renders.borrow().len(), 1);
        assert!(cache.get("a").is_none());
        assert_eq!(cache.iter().count(), 1);
    }
}
