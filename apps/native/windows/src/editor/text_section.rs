//! The Text sections of Tool properties (shipping `ScreenshotEditor.tsx`):
//! a selected label's Text style picker, textarea, Font and Size, format
//! buttons, colors and Text background; and the Text tool's New text style
//! and New text size. Label edits go to the session as `EditText` (typing
//! and size as `Request::Live`, one undo step per field); the new-text
//! defaults stay window-local like shipping's `defaultTextStyle` and
//! `defaultFontSize`.
use captures_app::editor::{Element, OptionalNullable, TextElement};
use captures_app::editor_chrome::text_format;
use captures_app::editor_session::{Request, TextPatch};
use captures_app::editor_text::{self, TEXT_STYLE_PRESETS};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;

use super::menu::MenuKind;
use super::props::{Num, Prop, Section, TextChange};
use super::{Editor, Layer};
use crate::gfx::{self, Style, rect};

/// `.screenshot-text-style-preview { width: 72px; min-height: 24px }`, centred
/// in the 82 px column.
const PREVIEW_W: f32 = 72.;
const PREVIEW_H: f32 = 24.;
pub const PREVIEW_COLUMN: f32 = 82.;
/// The trigger's 14 px chevron column.
const CHEVRON: f32 = 14.;

impl Editor {
    /// The selected layer when it is a label.
    pub fn selected_text(&self) -> Option<&TextElement> {
        let id = self.selected.as_deref()?;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Text(text) if text.base.id == id => Some(text),
                _ => None,
            })
    }

    /// Shipping `defaultFontSize`, starting at the session's text size.
    pub fn new_text_size(&self) -> f64 {
        self.text_size
            .or_else(|| self.presented.as_ref().map(|p| p.text_size))
            .unwrap_or(32.)
    }

    /// The picker's preset: the label's treatment, or the new-text default.
    pub fn style_preset(&self) -> &'static str {
        match (self.section(), self.selected_text()) {
            (Some(Section::Text(_)), Some(t)) => editor_text::text_style_preset_id(
                &t.font_family,
                t.background.is_some(),
                t.outlined,
                t.rounded_background,
            ),
            _ => self.text_preset,
        }
    }

    /// The picker's `label` prop.
    pub fn style_label(&self) -> &'static str {
        if self.section() == Some(Section::TextDefaults) {
            "New text style"
        } else {
            "Text style"
        }
    }

    pub fn number_value(&self, num: Num) -> f64 {
        match num {
            Num::TextSize => self.selected_text().map_or(0., |t| t.font_size),
            Num::NewTextSize => self.new_text_size(),
            Num::ShadowX => self.values().map_or(0., |v| v.shadow_x.round()),
            Num::ShadowY => self.values().map_or(0., |v| v.shadow_y.round()),
            Num::CanvasWidth | Num::CanvasHeight => self
                .document_size()
                .map_or(0., |(w, h)| if num == Num::CanvasWidth { w } else { h }),
            num => self.geometry_value(num),
        }
    }

    pub(super) fn set_number(&mut self, num: Num, value: f64) {
        match num {
            Num::TextSize | Num::NewTextSize => {
                if value != self.number_value(num) {
                    self.apply_text(TextChange::Size(value));
                }
            }
            Num::CanvasWidth | Num::CanvasHeight => self.resize_canvas(num, value),
            Num::ShadowX | Num::ShadowY => {
                if value != self.number_value(num) {
                    self.apply(super::props::Change::Offset(num == Num::ShadowX, value));
                }
            }
            num => self.set_geometry(num, value),
        }
    }

    /// Whether a format button is on.
    pub fn format_on(&self, index: usize) -> bool {
        let Some(t) = self.selected_text() else {
            return false;
        };
        match index {
            0 => t.bold,
            1 => t.italic,
            i => text_format::ALIGN
                .get(i - 2)
                .is_some_and(|a| t.align == a.0),
        }
    }

    pub fn format_name(index: usize) -> &'static str {
        match index {
            0 => text_format::BOLD,
            1 => text_format::ITALIC,
            i => text_format::ALIGN[i - 2].1,
        }
    }

    pub(super) fn activate_format(&mut self, index: usize) {
        let change = match index {
            0 => TextChange::Bold,
            1 => TextChange::Italic,
            i => TextChange::Align(text_format::ALIGN[i - 2].0),
        };
        self.apply_text(change);
        self.events
            .push(super::Event::Toggled(super::Target::Prop(Prop::Format(
                index,
            ))));
        if index >= 2 {
            for other in 2..2 + text_format::ALIGN.len() {
                if other != index {
                    self.events
                        .push(super::Event::Toggled(super::Target::Prop(Prop::Format(
                            other,
                        ))));
                }
            }
        }
    }

    /// Apply a Text-section edit to the selected label, or to the new-text
    /// defaults.
    pub(super) fn apply_text(&mut self, change: TextChange) {
        match self.section() {
            Some(Section::Text(id)) => {
                let Some(text) = self.selected_text().cloned() else {
                    return;
                };
                let (live, patch) = match change {
                    TextChange::Preset(preset_id) => {
                        // Shipping also makes it the new-text default.
                        self.text_preset = preset_id;
                        let Some(preset) = TEXT_STYLE_PRESETS.iter().find(|p| p.id == preset_id)
                        else {
                            return;
                        };
                        // A preset with a plate keeps an existing plate color.
                        let background = match preset.background {
                            Some(color) => OptionalNullable::Value(
                                text.background.clone().unwrap_or_else(|| color.to_owned()),
                            ),
                            None => OptionalNullable::Null,
                        };
                        (
                            None,
                            TextPatch {
                                font_family: Some(preset.font_family.to_owned()),
                                background,
                                outlined: Some(preset.outlined),
                                rounded_background: Some(preset.rounded_background),
                                ..Default::default()
                            },
                        )
                    }
                    TextChange::Content(value) => (
                        Some("text"),
                        TextPatch {
                            text: Some(value),
                            ..Default::default()
                        },
                    ),
                    TextChange::Family(family) => (
                        None,
                        TextPatch {
                            // Only Nunito keeps the rounded plate.
                            rounded_background: (family != "rounded").then_some(false),
                            font_family: Some(family),
                            ..Default::default()
                        },
                    ),
                    TextChange::Size(size) => (
                        Some("size"),
                        TextPatch {
                            font_size: Some(size.max(8.)),
                            ..Default::default()
                        },
                    ),
                    TextChange::Bold => (
                        None,
                        TextPatch {
                            bold: Some(!text.bold),
                            ..Default::default()
                        },
                    ),
                    TextChange::Italic => (
                        None,
                        TextPatch {
                            italic: Some(!text.italic),
                            ..Default::default()
                        },
                    ),
                    TextChange::Align(align) => (
                        None,
                        TextPatch {
                            align: Some(align.to_owned()),
                            ..Default::default()
                        },
                    ),
                };
                self.edit_text(&id, patch, live);
            }
            Some(Section::TextDefaults) => match change {
                TextChange::Preset(preset_id) => self.text_preset = preset_id,
                TextChange::Size(size) => self.text_size = Some(size),
                _ => return,
            },
            _ => return,
        }
        self.props_changed();
    }

    /// Send a label edit; show it at once, the session's reply confirms it.
    pub(super) fn edit_text(&mut self, id: &str, patch: TextPatch, live: Option<&str>) {
        if let Some(Element::Text(text)) = self
            .presented
            .as_mut()
            .and_then(|p| p.document.elements.iter_mut().find(|e| e.base().id == id))
        {
            preview_text(text, &patch);
        }
        let request = Request::EditText {
            id: id.to_owned(),
            patch,
        };
        match live {
            // Shipping commits every Size step and every keystroke of the
            // Text field as its own undo step (`updateSelected`); a unique
            // key keeps them apart while `submit_live` still queues only the
            // latest behind a busy worker.
            Some(key) => {
                self.edit_seq += 1;
                let key = format!("{id}:{key}:{}", self.edit_seq);
                self.submit_live(Request::Live {
                    key,
                    request: Box::new(request),
                });
            }
            None => self.submit(request),
        }
    }

    // ---- drawing ----

    /// `.screenshot-text-style-preview` for a preset, its box at `x`
    /// (the 82 px column's left) centred in a row `h` tall at `y`.
    pub(super) fn draw_style_preview(
        &self,
        dc: &ID2D1DeviceContext,
        preset_id: &str,
        (x, y, h): (f32, f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let Some(preset) = TEXT_STYLE_PRESETS.iter().find(|p| p.id == preset_id) else {
            return Ok(());
        };
        let b = rect(
            x + (PREVIEW_COLUMN - PREVIEW_W) / 2.,
            y + ((h - PREVIEW_H) / 2.).round(),
            x + (PREVIEW_COLUMN + PREVIEW_W) / 2.,
            y + ((h - PREVIEW_H) / 2.).round() + PREVIEW_H,
        );
        let boxed = preset.background.is_some();
        if boxed {
            let radius = if preset.rounded_background {
                t.n("r-lg")
            } else {
                0.
            };
            gfx::fill_round(dc, b, radius, t.color("solid"))?;
        }
        let ink = if boxed {
            t.color("solid-ink")
        } else {
            t.color("text")
        };
        let layout = match preset.font_family {
            "rounded" => self.bundled_layout("rounded", 17., DWRITE_FONT_WEIGHT_BOLD, "Text"),
            _ => None,
        };
        let layout = match layout {
            Some(layout) => layout,
            None => {
                let style = if preset.font_family == "mono" {
                    Style::PreviewMono
                } else {
                    Style::Preview
                };
                self.g.text(t, style, "Text", 1e4)?.layout
            }
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        let origin = (
            b.left + (PREVIEW_W - m.width) / 2.,
            b.top + (PREVIEW_H - m.height) / 2.,
        );
        if preset.outlined {
            // `color: transparent; -webkit-text-stroke: 1.2px var(--text)`.
            return gfx::outline_text(dc, &self.g.factory, &layout, origin, 1.2, ink);
        }
        unsafe {
            dc.DrawTextLayout(
                windows_numerics::Vector2 {
                    X: origin.0,
                    Y: origin.1,
                },
                &layout,
                &gfx::brush(dc, ink)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        Ok(())
    }

    /// `.screenshot-text-style-trigger`: preview, label and chevron.
    pub(super) fn draw_style_trigger(
        &self,
        dc: &ID2D1DeviceContext,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let open = self.menu_open(MenuKind::Style);
        let lit = layer == Layer::Lit;
        let (fill, border, ink) = if lit {
            (c("control-hover"), c("border-strong"), c("text"))
        } else {
            (c("surface-field"), c("control-border"), c("text-muted"))
        };
        gfx::fill_round(dc, all, radius, fill)?;
        // `[aria-expanded="true"]` keeps the accent border over hover.
        gfx::border(
            dc,
            all,
            radius,
            1.,
            if open { c("theme-accent") } else { border },
        )?;
        let s4 = t.n("s-4");
        let preset = self.style_preset();
        self.draw_style_preview(dc, preset, (s4, 0., h))?;
        let label = TEXT_STYLE_PRESETS
            .iter()
            .find(|p| p.id == preset)
            .map_or("", |p| p.label);
        let text = self.g.text(t, Style::Button, label, 1e4)?;
        let x = s4 + PREVIEW_COLUMN + s4;
        self.g
            .ellipsis(&text, (w - x - s4 - CHEVRON - s4).max(1.))?;
        gfx::text(dc, &text, x, ((h - text.height) / 2.).round(), ink)?;
        // `rotate(180deg)` while open.
        let (cx, cy) = (w - s4 - CHEVRON, (h - CHEVRON) / 2.);
        let s = CHEVRON / 24.;
        let p = |x: f32, y: f32| {
            let y = if open { 24. - y } else { y };
            (cx + x * s, cy + y * s)
        };
        super::polyline(
            dc,
            &self.g,
            &[p(6., 9.), p(12., 15.), p(18., 9.)],
            false,
            ink,
            1.9,
        )
    }

    /// The Font `<select>`, drawn as Captures' `.custom-select-trigger`
    /// inside the property field metrics.
    /// The Font or Aspect ratio select's trigger.
    pub(super) fn draw_select(
        &self,
        dc: &ID2D1DeviceContext,
        kind: MenuKind,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let open = self.menu_open(kind);
        let focused = self.focus == Some(super::Target::Prop(kind.trigger()));
        if layer == Layer::Lit {
            if open || focused {
                return Ok(());
            }
            return gfx::border(dc, all, radius, 1., c("border-strong"));
        }
        gfx::fill_round(dc, all, radius, c("surface-field"))?;
        let border = if open || focused {
            c("theme-accent")
        } else {
            c("control-border")
        };
        gfx::border(dc, all, radius, 1., border)?;
        let label = self
            .menu_selected(kind)
            .and_then(|i| self.menu_items(kind).into_iter().nth(i))
            .unwrap_or_default();
        let s4 = t.n("s-4");
        let text = self.g.text(t, Style::Option, &label, 1e4)?;
        self.g.ellipsis(&text, (w - 2. * s4 - 14.).max(1.))?;
        gfx::text(dc, &text, s4, ((h - text.height) / 2.).round(), c("text"))?;
        gfx::icon(
            dc,
            &self.g.factory,
            "chevron-down",
            &[],
            (w - s4 - 10., (h - 10.) / 2., 10.),
            1.8,
            t.color_alpha("text-subtle", 0.8),
        )
    }

    /// `.screenshot-format-buttons button`: B, *I* or an alignment icon.
    pub(super) fn draw_format(
        &self,
        dc: &ID2D1DeviceContext,
        index: usize,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-sm");
        let active = self.format_on(index);
        let ink = if active {
            if layer == Layer::Lit {
                return Ok(());
            }
            gfx::fill_round(dc, all, radius, c("theme-accent"))?;
            gfx::border(dc, all, radius, 1., c("theme-accent"))?;
            c("theme-accent-ink")
        } else if layer == Layer::Lit {
            gfx::fill_round(dc, all, radius, c("control-hover"))?;
            gfx::border(dc, all, radius, 1., c("border-subtle"))?;
            c("text")
        } else {
            gfx::fill_round(dc, all, radius, c("control"))?;
            gfx::border(dc, all, radius, 1., c("border-subtle"))?;
            c("text-muted")
        };
        match index {
            0 | 1 => {
                let style = if index == 0 {
                    Style::Strong
                } else {
                    Style::Italic
                };
                let glyph = if index == 0 { "B" } else { "I" };
                let text = self.g.text(t, style, glyph, 1e4)?;
                gfx::text(
                    dc,
                    &text,
                    ((w - text.width) / 2.).round(),
                    ((h - text.height) / 2.).round(),
                    ink,
                )
            }
            i => {
                let size = text_format::ICON as f32;
                gfx::icon(
                    dc,
                    &self.g.factory,
                    text_format::ALIGN[i - 2].2,
                    &[],
                    ((w - size) / 2., (h - size) / 2., size),
                    1.8,
                    ink,
                )
            }
        }
    }

    /// A bundled editor family's layout (the previews' Nunito).
    fn bundled_layout(
        &self,
        key: &str,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        text: &str,
    ) -> Option<IDWriteTextLayout> {
        let family = self.presented.as_ref()?.font_families.get(key)?.clone();
        super::text::bundled_text_layout(self.g.dwrite(), &family, size, weight, text)
    }
}

/// Apply a label patch to the shown element before the session confirms it.
fn preview_text(t: &mut TextElement, patch: &TextPatch) {
    if let Some(text) = &patch.text {
        t.text.clone_from(text);
    }
    if let Some(size) = patch.font_size {
        t.font_size = size;
    }
    if let Some(family) = &patch.font_family {
        t.font_family.clone_from(family);
    }
    if let Some(bold) = patch.bold {
        t.bold = bold;
    }
    if let Some(italic) = patch.italic {
        t.italic = italic;
    }
    if let Some(align) = &patch.align {
        t.align.clone_from(align);
    }
    if let Some(color) = &patch.color {
        t.color.clone_from(color);
    }
    match &patch.background {
        OptionalNullable::Value(color) => t.background = Some(color.clone()),
        OptionalNullable::Null => t.background = None,
        _ => {}
    }
    if let Some(rounded) = patch.rounded_background {
        t.rounded_background = rounded;
    }
    if let Some(outlined) = patch.outlined {
        t.outlined = outlined;
    }
    if let Some(shadow) = patch.drop_shadow {
        t.drop_shadow = Some(shadow);
    }
}
