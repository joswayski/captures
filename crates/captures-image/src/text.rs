//! Explicit-font, single-line shaping and CPU rasterization for editor labels.
//!
//! Hosts supply font bytes; this module never scans installed fonts. Paragraph
//! wrapping, plates, shadows, document edits and native input belong to the
//! editor integration, not this primitive. The legacy `Shape::Text` is unchanged.

use std::{collections::HashMap, sync::Arc};

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, SwashImage, Weight, Wrap, fontdb,
};
use image::{Pixel, Rgba, RgbaImage};
use swash::zeno::{Format, Join, Mask, Origin, Stroke, Vector};

use crate::Bounds;

const MAX_LINE_BYTES: usize = 4096;
const MAX_EXTENT: f32 = 16_384.;
const MAX_PIXELS: u64 = 16_777_216;

pub struct TextStyle<'a> {
    /// A family name embedded in one of the supplied fonts, not an OS generic.
    pub family: &'a str,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 4],
}

pub struct TextLine {
    /// Logical advance, including spaces; distinct from painted ink bounds.
    pub advance: f32,
    /// Baseline in a line box whose height is 1.25 times the type size.
    pub baseline: f32,
    /// Pixel-aligned ink placement relative to the line box's top-left.
    pub bounds: Bounds,
    /// Straight-alpha RGBA. Whitespace-only lines have a zero-size bitmap.
    pub pixels: RgbaImage,
}

/// Reuse within a serialized editor worker. Fonts and shaping scratch persist;
/// glyph images are retained only for the duration of each operation.
/// Font bytes must come from a trusted font source. Output budgets do not sandbox
/// the font parser or bound allocations inside the third-party glyph rasterizer.
pub struct TextRenderer {
    fonts: FontSystem,
    raster: SwashCache,
}

impl TextRenderer {
    pub fn new(fonts: impl IntoIterator<Item = Arc<[u8]>>) -> Result<Self, String> {
        let mut db = fontdb::Database::new();
        for bytes in fonts {
            let before = db.faces().count();
            db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
            if db.faces().count() == before {
                return Err("Invalid or unsupported text font.".into());
            }
        }
        if db.faces().next().is_none() {
            return Err("Text requires explicit font bytes.".into());
        }
        Ok(Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            raster: SwashCache::new(),
        })
    }

    pub fn render_line(&mut self, text: &str, style: &TextStyle<'_>) -> Result<TextLine, String> {
        let result = self.render_line_inner(text, style, None);
        self.raster.image_cache.clear();
        result
    }

    /// Stroke actual glyph contours with round joins and no filled interior.
    /// The centered width is in raster pixels; advances and baseline stay unchanged.
    /// Color/bitmap glyphs are explicit errors rather than dilated or substituted.
    pub fn render_outline_line(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        width: f32,
    ) -> Result<TextLine, String> {
        if !width.is_finite() || width <= 0. || width > 512. {
            return Err("Text outline width must be greater than zero and at most 512.".into());
        }
        let result = self.render_line_inner(text, style, Some(width));
        self.raster.image_cache.clear();
        self.raster.outline_command_cache.clear();
        result
    }

    /// Shape without allocating glyph bitmaps; wrapping measures many candidate lines.
    /// Advances may exceed the raster extent limit so callers can break long tokens.
    pub fn measure_line(&mut self, text: &str, style: &TextStyle<'_>) -> Result<f32, String> {
        let buffer = self.shape_line(text, style)?;
        Ok(buffer.layout_runs().next().expect("validated line").line_w)
    }

    fn shape_line(&mut self, text: &str, style: &TextStyle<'_>) -> Result<Buffer, String> {
        if text.len() > MAX_LINE_BYTES
            || text.contains([
                '\r', '\n', '\u{000b}', '\u{000c}', '\u{0085}', '\u{2028}', '\u{2029}',
            ])
        {
            return Err("Text must be one line of at most 4096 UTF-8 bytes.".into());
        }
        if !style.size.is_finite() || style.size <= 0. || style.size > 512. {
            return Err("Text size must be greater than zero and at most 512.".into());
        }
        if !self
            .fonts
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == style.family))
        {
            return Err("Requested text family was not supplied.".into());
        }
        let attrs = Attrs::new()
            .family(Family::Name(style.family))
            .weight(if style.bold {
                Weight::BOLD
            } else {
                Weight::NORMAL
            })
            .style(if style.italic {
                Style::Italic
            } else {
                Style::Normal
            });
        let mut buffer = Buffer::new_empty(Metrics::new(style.size, style.size * 1.25));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs, Shaping::Advanced, Some(Align::Left));
        buffer.shape_until_scroll(&mut self.fonts, false);
        let run = buffer
            .layout_runs()
            .next()
            .ok_or("Text layout produced no line.")?;
        if !run.line_w.is_finite() || run.line_w < 0. {
            return Err("Text line has an invalid advance.".into());
        }
        if run.glyphs.iter().any(|glyph| glyph.glyph_id == 0) {
            return Err("Supplied fonts cannot shape every text glyph.".into());
        }
        Ok(buffer)
    }

    fn outline_image(&mut self, key: CacheKey, width: f32) -> Result<Option<SwashImage>, String> {
        let Some(fill) = self.raster.get_image(&mut self.fonts, key) else {
            return Ok(None); // Nonpainting glyph, such as a space.
        };
        if fill.content != SwashContent::Mask
            || !matches!(fill.source, swash::scale::Source::Outline)
        {
            return Err("Text outlines require monochrome scalable glyphs.".into());
        }
        // cosmic-text retains the shaped face/weight, hinting and synthetic italic
        // in these paths. Match Swash's baseline-up placement and subpixel offset.
        let commands = self
            .raster
            .get_outline_commands(&mut self.fonts, key)
            .ok_or("Text glyph has no scalable outline.")?;
        let mut offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
        if key.flags.contains(CacheKeyFlags::PIXEL_FONT) {
            offset = Vector::new(offset.x.round(), offset.y.round());
        }
        let mut stroke = Stroke::new(width);
        stroke.join(Join::Round);
        let mut mask = Mask::new(commands);
        mask.format(Format::Alpha)
            .origin(Origin::BottomLeft)
            .style(stroke)
            .offset(offset)
            .render_offset(offset);
        let mut within_budget = false;
        mask.inspect(|_, width, height| {
            within_budget = width <= MAX_EXTENT as u32
                && height <= MAX_EXTENT as u32
                && u64::from(width) * u64::from(height) <= MAX_PIXELS;
        });
        if !within_budget {
            return Err("Text outline exceeds the raster bounds limit.".into());
        }
        let (data, placement) = mask.render();
        Ok(Some(SwashImage {
            source: swash::scale::Source::Outline,
            content: SwashContent::Mask,
            placement,
            data,
        }))
    }

    fn render_line_inner(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        outline_width: Option<f32>,
    ) -> Result<TextLine, String> {
        let buffer = self.shape_line(text, style)?;
        let run = buffer.layout_runs().next().expect("validated line");
        if run.line_w > MAX_EXTENT {
            return Err("Text line exceeds the raster bounds limit.".into());
        }
        // Operation-local: different widths and filled/outlined calls cannot reuse
        // one another's masks. Commands and fill images are cleared by the caller.
        let mut outlines = HashMap::new();
        let mut placements = Vec::new();
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        let mut glyph_pixels = 0_u64;
        for glyph in run.glyphs {
            let physical = glyph.physical((0., run.line_y), 1.);
            let image = if let Some(width) = outline_width {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    outlines.entry(physical.cache_key)
                {
                    entry.insert(self.outline_image(physical.cache_key, width)?);
                }
                &outlines[&physical.cache_key]
            } else {
                self.raster.get_image(&mut self.fonts, physical.cache_key)
            };
            let Some(image) = image else {
                continue; // Spaces and other nonpainting glyphs still contribute advance.
            };
            if image.content == SwashContent::SubpixelMask {
                return Err("Subpixel text masks are unsupported.".into());
            }
            glyph_pixels += u64::from(image.placement.width) * u64::from(image.placement.height);
            if glyph_pixels > MAX_PIXELS {
                return Err("Text glyphs exceed the raster pixel budget.".into());
            }
            if image.placement.width == 0 || image.placement.height == 0 {
                continue;
            }
            let x = physical
                .x
                .checked_add(image.placement.left)
                .ok_or("Text bounds overflow.")?;
            let y = physical
                .y
                .checked_sub(image.placement.top)
                .ok_or("Text bounds overflow.")?;
            let right = x
                .checked_add(
                    i32::try_from(image.placement.width).map_err(|_| "Text bounds overflow.")?,
                )
                .ok_or("Text bounds overflow.")?;
            let bottom = y
                .checked_add(
                    i32::try_from(image.placement.height).map_err(|_| "Text bounds overflow.")?,
                )
                .ok_or("Text bounds overflow.")?;
            bounds = Some(match bounds {
                Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(right), b.max(bottom)),
                None => (x, y, right, bottom),
            });
            placements.push((physical.cache_key, x, y));
        }
        let (left, top, right, bottom) = bounds.unwrap_or((0, 0, 0, 0));
        let width = i64::from(right) - i64::from(left);
        let height = i64::from(bottom) - i64::from(top);
        if width > MAX_EXTENT as i64
            || height > MAX_EXTENT as i64
            || width * height > MAX_PIXELS as i64
        {
            return Err("Text ink exceeds the raster bounds limit.".into());
        }
        let mut pixels = RgbaImage::new(width as u32, height as u32);
        for (key, x, y) in placements {
            let image = if outline_width.is_some() {
                &outlines[&key]
            } else {
                &self.raster.image_cache[&key]
            }
            .as_ref()
            .expect("rasterized above");
            for row in 0..image.placement.height {
                for column in 0..image.placement.width {
                    let index = (row * image.placement.width + column) as usize;
                    let mut rgba = match image.content {
                        SwashContent::Mask => [
                            style.color[0],
                            style.color[1],
                            style.color[2],
                            image.data[index],
                        ],
                        SwashContent::Color => image.data[index * 4..index * 4 + 4]
                            .try_into()
                            .expect("RGBA glyph"),
                        SwashContent::SubpixelMask => unreachable!("rejected above"),
                    };
                    // Swash COLR layers are flattened into premultiplied color;
                    // its embedded PNG glyphs are straight RGBA. Normalize only
                    // outlines before applying external opacity. Unpremultiplying
                    // cannot recover the alpha precision lost by COLR's /256 blitter.
                    if matches!(image.source, swash::scale::Source::ColorOutline(_)) {
                        let alpha = u32::from(rgba[3]);
                        for channel in &mut rgba[..3] {
                            *channel = if alpha == 0 {
                                0
                            } else {
                                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
                            };
                        }
                    }
                    pixels
                        .get_pixel_mut((x - left) as u32 + column, (y - top) as u32 + row)
                        .blend(&Rgba(rgba));
                }
            }
        }
        // Opacity belongs to the line, not to individual overlapping glyphs.
        for pixel in pixels.pixels_mut() {
            pixel[3] = ((u16::from(pixel[3]) * u16::from(style.color[3]) + 127) / 255) as u8;
        }
        Ok(TextLine {
            advance: run.line_w,
            baseline: run.line_y,
            bounds: Bounds {
                x: left as f32,
                y: top as f32,
                width: width as f32,
                height: height as f32,
            },
            pixels,
        })
    }
}
