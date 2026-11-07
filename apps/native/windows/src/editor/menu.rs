//! Popups over the editor: the Text style picker's menu (shipping
//! `TextStylePicker`: `role="menu"` of `menuitemradio` presets with a
//! preview, placed by its `positionMenu`) and the Font `<select>`'s option
//! list, drawn as Captures' `.custom-select-listbox` and placed and keyed by
//! the shared `controls::select` model the Preferences selects use.
//!
//! A popup is one retained surface plus a row-sized "lit" surface that moves
//! on the compositor: pointer and keyboard highlight repaint only that row.
//! Focus stays on the trigger, as in shipping, and the popup closes on
//! choice, Escape, Tab, an outside press or when its trigger goes away.
use captures_app::controls::select;
use captures_app::editor_chrome::{self, header};
use captures_app::editor_text::{self, TEXT_STYLE_PRESETS};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::*;

use super::props::{Prop, TextChange};
use super::{Editor, Event, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// The Text style (or New text style) picker.
    Style,
    /// The selected label's Font select.
    Font,
    /// The Crop section's Aspect ratio select.
    Aspect,
    /// The header's Canvas zoom preset select.
    Preset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Menu {
    pub kind: MenuKind,
    /// The highlighted row (pointer or keyboard).
    pub active: Option<usize>,
    /// The panel in client DIPs, border included.
    pub rect: Rect,
}

/// `.screenshot-text-style-menu button { min-height: 38px }` and `gap: 2px`.
const STYLE_ROW: f32 = 38.;
const STYLE_GAP: f32 = 2.;
/// `positionMenu`: `gap = 5`, `viewportPadding = 8`.
const STYLE_MENU_GAP: f32 = 5.;
const VIEWPORT_PADDING: f32 = 8.;
/// Room for `--shadow-lg` around the panel.
pub const MENU_BLEED: f32 = 36.;

impl MenuKind {
    /// A `.custom-select-listbox` (`role="listbox"`), not a menu.
    pub fn is_select(self) -> bool {
        matches!(self, MenuKind::Font | MenuKind::Aspect | MenuKind::Preset)
    }

    /// The trigger that owns the popup.
    pub fn trigger(self) -> Prop {
        match self {
            MenuKind::Style => Prop::StyleTrigger,
            MenuKind::Font => Prop::Font,
            MenuKind::Aspect => Prop::Aspect,
            MenuKind::Preset => Prop::Preset,
        }
    }
}

impl Editor {
    /// Option labels, in order.
    pub fn menu_items(&self, kind: MenuKind) -> Vec<String> {
        match kind {
            MenuKind::Style => TEXT_STYLE_PRESETS
                .iter()
                .map(|p| p.label.to_owned())
                .collect(),
            MenuKind::Font => self
                .font_options()
                .into_iter()
                .map(|(_, label)| label)
                .collect(),
            MenuKind::Aspect => super::crop::ASPECTS
                .iter()
                .map(|(label, _)| (*label).to_owned())
                .collect(),
            MenuKind::Preset => self
                .preset_options()
                .into_iter()
                .map(|(_, label)| label)
                .collect(),
        }
    }

    /// The zoom preset `<option>`s: Fit (`None`), a manual zoom that isn't a
    /// preset, then `SCREENSHOT_ZOOM_OPTIONS`.
    pub fn preset_options(&self) -> Vec<(Option<f64>, String)> {
        let mut options = vec![(None, "Fit".to_owned())];
        let zoom = self.viewport.zoom_percent;
        if !self.fit_mode() && !header::ZOOM_PRESETS.contains(&zoom) {
            options.push((Some(zoom), editor_chrome::zoom_label(zoom)));
        }
        options.extend(
            header::ZOOM_PRESETS
                .iter()
                .map(|&p| (Some(p), format!("{p}%"))),
        );
        options
    }

    /// The Font select's `(family key, label)` options.
    pub fn font_options(&self) -> Vec<(String, String)> {
        self.presented
            .as_ref()
            .map(|p| editor_text::font_family_options(&p.font_families))
            .unwrap_or_default()
    }

    /// The chosen option's index.
    pub fn menu_selected(&self, kind: MenuKind) -> Option<usize> {
        match kind {
            MenuKind::Style => {
                let id = self.style_preset();
                TEXT_STYLE_PRESETS.iter().position(|p| p.id == id)
            }
            MenuKind::Font => {
                let family = &self.selected_text()?.font_family;
                self.font_options()
                    .iter()
                    .position(|(key, _)| key == family)
            }
            MenuKind::Aspect => Some(self.crop_aspect),
            MenuKind::Preset => {
                let value = (!self.fit_mode()).then_some(self.viewport.zoom_percent);
                self.preset_options().iter().position(|(v, _)| *v == value)
            }
        }
    }

    pub fn menu_open(&self, kind: MenuKind) -> bool {
        self.menu.is_some_and(|m| m.kind == kind)
    }

    /// Open a popup at its selection, or close it if it is open.
    pub(super) fn toggle_menu(&mut self, kind: MenuKind) {
        if self.menu_open(kind) {
            self.close_menu();
        } else {
            self.open_menu(kind, self.menu_selected(kind));
        }
    }

    pub(super) fn open_menu(&mut self, kind: MenuKind, active: Option<usize>) {
        self.close_menu();
        if self.menu_items(kind).is_empty() {
            return;
        }
        self.menu = Some(Menu {
            kind,
            active,
            rect: Rect::default(),
        });
        self.place_menu();
        let panel = self.menu_node;
        self.nodes[panel].dirty = true;
        self.set_visible(panel, true);
        // `ui-pop-in`, as a fade on the compositor.
        self.nodes[panel].opacity = 0.;
        self.fade(panel, 1.);
        self.prop_dirty(kind.trigger());
        self.events
            .push(Event::Expanded(Target::Prop(kind.trigger())));
        self.events.push(Event::Structure);
        if let Some(active) = active {
            self.events.push(Event::MenuActive(active));
        }
        self.need = true;
    }

    /// The open popup's kind and highlighted row.
    pub fn open_menu_state(&self) -> Option<(MenuKind, Option<usize>)> {
        self.menu.map(|m| (m.kind, m.active))
    }

    /// The open popup's rect in client DIPs.
    pub fn menu_rect(&self) -> Option<crate::scene::Rect> {
        self.menu.map(|m| m.rect)
    }

    /// UI Automation `Expand` and `Collapse`.
    pub fn set_menu(&mut self, kind: MenuKind, open: bool) {
        if open && !self.menu_open(kind) {
            self.open_menu(kind, self.menu_selected(kind));
        } else if !open && self.menu_open(kind) {
            self.close_menu();
        }
    }

    /// Close the popup; it unmounts at once, like shipping.
    pub(super) fn close_menu(&mut self) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let panel = self.menu_node;
        self.motions.retain(|m| m.node != panel);
        self.nodes[panel].opacity = 1.;
        self.set_visible(panel, false);
        self.set_visible(self.menu_lit, false);
        self.prop_dirty(menu.kind.trigger());
        self.events
            .push(Event::Expanded(Target::Prop(menu.kind.trigger())));
        self.events.push(Event::Structure);
        self.need = true;
    }

    /// Row `index`'s rect inside the panel.
    fn menu_row(&self, kind: MenuKind, index: usize, width: f32) -> Rect {
        let s2 = self.t.n("s-2");
        match kind {
            MenuKind::Style => Rect::new(
                1. + s2,
                1. + s2 + index as f32 * (STYLE_ROW + STYLE_GAP),
                width - 2. - 2. * s2,
                STYLE_ROW,
            ),
            MenuKind::Font | MenuKind::Aspect | MenuKind::Preset => {
                let h = self.option_h();
                Rect::new(1. + s2, 1. + s2 + index as f32 * h, width - 2. - 2. * s2, h)
            }
        }
    }

    /// `.custom-select-option`: `--h-sm` tall at least.
    fn option_h(&self) -> f32 {
        let n = |name: &str| self.t.n(name);
        self.g
            .text(&self.t, Style::Option, "Ag", 1e4)
            .map_or(n("h-sm"), |t| {
                (t.height + 2. + 2. * n("s-3")).max(n("h-sm"))
            })
    }

    /// Place the open popup against its trigger; closes it when the trigger
    /// is gone.
    pub(super) fn place_menu(&mut self) {
        let Some(menu) = self.menu else {
            return;
        };
        let trigger_prop = menu.kind.trigger();
        if !self.prop_exists(trigger_prop) {
            self.close_menu();
            return;
        }
        let trigger = self.target_rect(Target::Prop(trigger_prop));
        let count = self.menu_items(menu.kind).len();
        let s2 = self.t.n("s-2");
        let r = match menu.kind {
            MenuKind::Style => {
                // Shipping `positionMenu`.
                let h = 2.
                    + 2. * s2
                    + count as f32 * STYLE_ROW
                    + count.saturating_sub(1) as f32 * STYLE_GAP;
                let below = self.h - (trigger.y + trigger.h) - VIEWPORT_PADDING;
                let above = trigger.y - VIEWPORT_PADDING;
                let top = if below < h && above > below {
                    trigger.y - h - STYLE_MENU_GAP
                } else {
                    trigger.y + trigger.h + STYLE_MENU_GAP
                };
                let top = top
                    .max(VIEWPORT_PADDING)
                    .min((self.h - h - VIEWPORT_PADDING).max(VIEWPORT_PADDING));
                let left = trigger
                    .x
                    .max(VIEWPORT_PADDING)
                    .min((self.w - trigger.w - VIEWPORT_PADDING).max(VIEWPORT_PADDING));
                Rect::new(left, top, trigger.w, h)
            }
            MenuKind::Font | MenuKind::Aspect | MenuKind::Preset => {
                let (s4, s6) = (self.t.n("s-4"), self.t.n("s-6"));
                let label_w = self
                    .menu_items(menu.kind)
                    .iter()
                    .filter_map(|l| self.g.text(&self.t, Style::Button, l, 1e4).ok())
                    .map(|t| t.width)
                    .fold(0., f32::max);
                let check_w = self
                    .g
                    .text(&self.t, Style::Option, "✓", 1e4)
                    .map_or(10., |t| t.width);
                let content_w = (label_w + s6 + check_w + 2. * s4 + 2. * s2 + 2.).ceil();
                let content_h = count as f32 * self.option_h() + 2. * s2;
                let layout = select::layout(
                    select::Rect {
                        left: f64::from(trigger.x),
                        top: f64::from(trigger.y),
                        width: f64::from(trigger.w),
                        height: f64::from(trigger.h),
                    },
                    f64::from(content_w),
                    f64::from(content_h + 2.),
                    f64::from(self.w),
                    f64::from(self.h),
                    count,
                );
                Rect::new(
                    layout.left as f32,
                    layout.top as f32,
                    layout.width as f32,
                    (layout.max_height as f32).min(content_h + 2.),
                )
            }
        };
        if let Some(open) = self.menu.as_mut() {
            open.rect = r;
        }
        self.set_rect(self.menu_node, r);
        self.place_menu_lit();
    }

    fn place_menu_lit(&mut self) {
        let Some(menu) = self.menu else {
            self.set_visible(self.menu_lit, false);
            return;
        };
        match menu.active {
            Some(active) => {
                let row = self.menu_row(menu.kind, active, menu.rect.w);
                let lit = self.menu_lit;
                if self.nodes[lit].rect != row {
                    self.nodes[lit].dirty = true;
                }
                self.set_rect(lit, row);
                self.set_visible(lit, true);
            }
            None => self.set_visible(self.menu_lit, false),
        }
    }

    /// Highlight a row; only the lit row repaints.
    pub(super) fn set_menu_active(&mut self, active: Option<usize>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if menu.active == active {
            return;
        }
        menu.active = active;
        self.nodes[self.menu_lit].dirty = true;
        self.place_menu_lit();
        if let Some(active) = active {
            self.events.push(Event::MenuActive(active));
        }
        self.need = true;
    }

    /// The row under a client point.
    pub fn menu_item_at(&self, x: f32, y: f32) -> Option<usize> {
        let menu = self.menu?;
        let (lx, ly) = (x - menu.rect.x, y - menu.rect.y);
        (0..self.menu_items(menu.kind).len())
            .find(|&i| self.menu_row(menu.kind, i, menu.rect.w).contains(lx, ly))
    }

    /// A row's rect in client DIPs.
    pub fn menu_item_rect(&self, index: usize) -> Rect {
        let Some(menu) = self.menu else {
            return Rect::default();
        };
        let r = self.menu_row(menu.kind, index, menu.rect.w);
        Rect::new(menu.rect.x + r.x, menu.rect.y + r.y, r.w, r.h)
    }

    /// Pick a row: apply it and close, focus staying on the trigger.
    pub fn choose_menu(&mut self, index: usize) {
        let Some(menu) = self.menu else {
            return;
        };
        self.close_menu();
        match menu.kind {
            MenuKind::Style => {
                if let Some(preset) = TEXT_STYLE_PRESETS.get(index) {
                    self.apply_text(TextChange::Preset(preset.id));
                }
            }
            MenuKind::Font => {
                if let Some((key, _)) = self.font_options().into_iter().nth(index) {
                    self.apply_text(TextChange::Family(key));
                }
            }
            MenuKind::Aspect => {
                if index < super::crop::ASPECTS.len() {
                    self.crop_aspect = index;
                }
            }
            // `activateFitZoom()` or `setManualZoom(Number(value))`.
            MenuKind::Preset => match self.preset_options().get(index) {
                Some((None, _)) => self.activate(Target::Fit),
                Some(&(Some(percent), _)) => self.zoom_to(percent, None),
                None => {}
            },
        }
        self.events
            .push(Event::Value(Target::Prop(menu.kind.trigger())));
    }

    /// Alt+Down or Alt+Up (a system key) on a focused select: opens it, or
    /// closes an open one keeping its choice, like the Preferences selects
    /// and Windows combo boxes. The Text style picker is a button.
    pub(super) fn menu_alt_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let Some(Target::Prop(prop @ (Prop::Font | Prop::Aspect | Prop::Preset))) = self.focus
        else {
            return false;
        };
        if !matches!(vk, VK_DOWN | VK_UP) {
            return false;
        }
        if self.menu.is_some_and(|m| m.kind.trigger() == prop) {
            return self.menu_key(VK_ESCAPE);
        }
        self.menu_key(vk)
    }

    /// Keys for a focused popup trigger; true when consumed. Both popups use
    /// the shared select keys while open; a closed Font select also opens
    /// on the arrows, a closed Text style picker only on Enter and Space
    /// (a button).
    pub(super) fn menu_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let Some(Target::Prop(prop)) = self.focus else {
            return false;
        };
        let kind = match prop {
            Prop::StyleTrigger => MenuKind::Style,
            Prop::Font => MenuKind::Font,
            Prop::Aspect => MenuKind::Aspect,
            Prop::Preset => MenuKind::Preset,
            _ => return false,
        };
        let key = match vk {
            VK_DOWN => select::Key::ArrowDown,
            VK_UP => select::Key::ArrowUp,
            VK_HOME => select::Key::Home,
            VK_END => select::Key::End,
            VK_RETURN => select::Key::Enter,
            VK_SPACE => select::Key::Space,
            VK_ESCAPE => select::Key::Escape,
            VK_TAB => {
                self.close_menu();
                return false;
            }
            _ => return false,
        };
        let open = self.menu.filter(|m| m.kind == kind);
        if open.is_none() && kind == MenuKind::Style {
            if matches!(key, select::Key::Enter | select::Key::Space) {
                self.open_menu(kind, self.menu_selected(kind));
                return true;
            }
            return false;
        }
        let count = self.menu_items(kind).len();
        let disabled = vec![false; count];
        let selected = self.menu_selected(kind).unwrap_or(0);
        let state = open.map_or_else(select::State::default, |m| select::State {
            open: true,
            active: m.active.unwrap_or(selected),
        });
        let outcome = select::key(state, &disabled, selected, key);
        if !outcome.handled {
            return false;
        }
        if let Some(index) = outcome.chosen {
            if open.is_none() {
                self.open_menu(kind, Some(index));
            }
            self.choose_menu(index);
        } else if outcome.state.open && open.is_none() {
            self.open_menu(kind, Some(outcome.state.active));
        } else if !outcome.state.open {
            self.close_menu();
        } else {
            self.set_menu_active(Some(outcome.state.active));
        }
        true
    }

    // ---- drawing ----

    /// `.screenshot-editor-zoom-presets`: the value in mono `--text-xs` at
    /// `padding-left: var(--s-4)` and the select's chevron; no hover fill or
    /// outline in shipping (`outline: none`).
    pub(super) fn draw_preset(
        &self,
        dc: &ID2D1DeviceContext,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        if lit {
            return Ok(());
        }
        let t = &self.t;
        let text = self.g.text(t, Style::MonoSmall, &self.zoom_text(), 1e4)?;
        gfx::text(
            dc,
            &text,
            t.n("s-4"),
            ((h - text.height) / 2.).round(),
            t.color("text"),
        )?;
        gfx::icon(
            dc,
            &self.g.factory,
            "chevron-down",
            &[],
            (w - 16., (h - 10.) / 2., 10.),
            1.8,
            t.color_alpha("text-subtle", 0.6),
        )
    }

    /// The panel and every row at rest.
    pub(super) fn draw_menu(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let Some(menu) = self.menu else {
            return Ok(());
        };
        let t = &self.t;
        let all = rect(0., 0., w, h);
        // `.screenshot-text-style-menu` (`--r-xl`) and the listbox (`--r-lg`).
        let radius = match menu.kind {
            MenuKind::Style => t.n("r-xl"),
            MenuKind::Font | MenuKind::Aspect | MenuKind::Preset => t.n("r-lg"),
        };
        gfx::shadow(dc, all, radius, t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, all, radius, t.color("surface-overlay"))?;
        gfx::border(dc, all, radius, 1., t.color("border"))?;
        for index in 0..self.menu_items(menu.kind).len() {
            let r = self.menu_row(menu.kind, index, w);
            self.draw_menu_row(dc, menu.kind, index, r, false)?;
        }
        Ok(())
    }

    /// The highlighted row, opaque over the resting one.
    pub(super) fn draw_menu_lit(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let Some(Menu {
            kind,
            active: Some(active),
            ..
        }) = self.menu
        else {
            return Ok(());
        };
        gfx::fill(dc, rect(0., 0., w, h), self.t.color("surface-overlay"))?;
        self.draw_menu_row(dc, kind, active, Rect::new(0., 0., w, h), true)
    }

    fn draw_menu_row(
        &self,
        dc: &ID2D1DeviceContext,
        kind: MenuKind,
        index: usize,
        r: Rect,
        lit: bool,
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let selected = self.menu_selected(kind) == Some(index);
        let items = self.menu_items(kind);
        let Some(label) = items.get(index) else {
            return Ok(());
        };
        let b = rect(r.x, r.y, r.x + r.w, r.y + r.h);
        match kind {
            MenuKind::Style => {
                let radius = t.n("r-md");
                // `.active` wins over `:hover` (it comes later).
                let ink = if selected {
                    gfx::fill_round(dc, b, radius, c("surface-selected"))?;
                    gfx::border(dc, b, radius, 1., c("theme-accent"))?;
                    c("text")
                } else if lit {
                    gfx::fill_round(dc, b, radius, c("control-hover"))?;
                    gfx::border(dc, b, radius, 1., c("border-strong"))?;
                    c("text")
                } else {
                    c("text-muted")
                };
                let s4 = t.n("s-4");
                self.draw_style_preview(dc, TEXT_STYLE_PRESETS[index].id, (r.x + s4, r.y, r.h))?;
                let text = self.g.text(t, Style::Button, label, 1e4)?;
                self.g.ellipsis(&text, (r.w - 2. * s4 - 82. - s4).max(1.))?;
                gfx::text(
                    dc,
                    &text,
                    r.x + s4 + 82. + s4,
                    r.y + ((r.h - text.height) / 2.).round(),
                    ink,
                )?;
            }
            MenuKind::Font | MenuKind::Aspect | MenuKind::Preset => {
                if lit {
                    gfx::fill_round(dc, b, t.n("r-sm"), c("surface-hover"))?;
                }
                let ink = if lit || selected {
                    c("text")
                } else {
                    c("text-muted")
                };
                let style = if selected {
                    Style::Button
                } else {
                    Style::Option
                };
                let pad = t.n("s-4");
                let text = self.g.text(t, style, label, 1e4)?;
                gfx::text(dc, &text, r.x + pad, r.y + (r.h - text.height) / 2., ink)?;
                if selected {
                    let check = self.g.text(t, Style::Option, "✓", 1e4)?;
                    let mark = if t.appearance == "dark" {
                        "theme-accent"
                    } else {
                        "theme-accent-readable"
                    };
                    gfx::text(
                        dc,
                        &check,
                        r.x + r.w - pad - check.width,
                        r.y + (r.h - check.height) / 2.,
                        c(mark),
                    )?;
                }
            }
        }
        Ok(())
    }
}
