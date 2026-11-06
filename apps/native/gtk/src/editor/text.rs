//! The Text tool: shipping's inline textarea over the canvas.
//!
//! A real `GtkTextView` owns typing, IME, selection and AT-SPI, and draws the
//! layer while it is edited, as shipping's textarea does: the session omits
//! the layer from its preview while a text input is active. Typing is
//! coalesced into at most one in-flight `UpdateTextInput`; only Finish
//! publishes document history. Escape and leaving the field commit, as in
//! shipping (there is no cancel).
use std::cell::Cell;

use captures_app::editor::{Element, Point, TextElement};
use captures_app::editor_session::{Request, TextCreate, TextInputTarget};
use captures_app::editor_text::{self, InlineEditorLayout};
use gtk::prelude::*;
use gtk::{gdk, glib};

use super::{Job, State};

/// The named treatment new labels start with, as shipping.
const NEW_TEXT_PRESET: &str = "rounded-box";

/// One inline text transaction.
pub(super) struct Inline {
    id: String,
    /// Begin was accepted; the session holds a transient layer.
    started: bool,
    /// The text the session last accepted.
    accepted: String,
    /// A request is with the worker.
    in_flight: bool,
    /// Finish was requested (always a commit, as shipping).
    finishing: bool,
    /// The geometry the field draws at, in document units.
    layout: Option<InlineEditorLayout>,
    element: Option<TextElement>,
}

/// The text field and its per-layer stylesheet.
pub(super) struct TextField {
    pub(super) view: gtk::TextView,
    css: gtk::CssProvider,
    /// Programmatic buffer writes must not read as typing.
    loading: Cell<bool>,
}

impl TextField {
    pub(super) fn new() -> Self {
        let view = gtk::TextView::new();
        view.add_css_class("inline-text");
        view.set_accepts_tab(false);
        // Clip to the Rounded Box plate radius.
        view.set_overflow(gtk::Overflow::Hidden);
        view.set_visible(false);
        view.set_halign(gtk::Align::Start);
        view.set_valign(gtk::Align::Start);
        view.update_property(&[gtk::accessible::Property::Label(
            captures_app::editor_chrome::text_format::INLINE_LABEL,
        )]);
        let css = gtk::CssProvider::new();
        view.style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        TextField {
            view,
            css,
            loading: Cell::new(false),
        }
    }

    fn text(&self) -> String {
        let buffer = self.view.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    fn set_text(&self, text: &str) {
        self.loading.set(true);
        self.view.buffer().set_text(text);
        self.loading.set(false);
    }
}

/// Register the bundled editor fonts with fontconfig so the field draws the
/// layer in the session's faces. Call before GTK creates its font map.
pub fn register_fonts() {
    #[link(name = "fontconfig")]
    unsafe extern "C" {
        fn FcConfigAppFontAddFile(config: *mut std::ffi::c_void, file: *const u8) -> i32;
    }
    let Some(cache) = glib::user_cache_dir()
        .to_str()
        .map(std::path::PathBuf::from)
    else {
        return;
    };
    let directory = cache.join("captures-gtk").join("fonts");
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    for (id, bytes) in captures_app::editor_fonts::bundled().files {
        let path = directory.join(format!("{id}.ttf"));
        let current = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
        if !current && std::fs::write(&path, &bytes).is_err() {
            continue;
        }
        let Ok(file) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else {
            continue;
        };
        // SAFETY: a null config is fontconfig's current configuration, and
        // `file` is a NUL-terminated path that outlives the call.
        unsafe {
            FcConfigAppFontAddFile(std::ptr::null_mut(), file.as_ptr().cast());
        }
    }
}

impl State {
    pub(super) fn text_active(&self) -> bool {
        self.inline.borrow().is_some()
    }

    /// A Text click: edit the text layer under the pointer, or place a new one.
    pub(super) fn begin_text(&self, point: Point) {
        if self.text_active() {
            self.finish_text();
            return;
        }
        let guard = self.presented.borrow();
        let Some(presented) = guard.as_ref() else {
            return;
        };
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let hit = presented
            .document
            .hit_test(point, 8. / scale)
            .ok()
            .flatten()
            .and_then(|element| match element {
                Element::Text(text) if !text.base.locked => Some(text.base.id.clone()),
                _ => None,
            });
        let target = match hit {
            Some(id) => TextInputTarget::Existing { id },
            None => TextInputTarget::New {
                create: TextCreate {
                    point,
                    text: String::new(),
                    font_size: self.text_size.get(),
                    font_family: "sans".into(),
                    // Shipping places new text in the drawing color.
                    color: presented.style.color.clone(),
                    style_preset: Some(NEW_TEXT_PRESET.into()),
                    drop_shadow: presented.style.drop_shadow,
                    drop_shadow_style: presented.style.drop_shadow_style.clone(),
                },
            },
        };
        drop(guard);
        self.start_text(target);
    }

    /// Double-click with Select edits an existing label.
    pub(super) fn edit_text_at(&self, point: Point) -> bool {
        let id = self.presented.borrow().as_ref().and_then(|presented| {
            let scale = self.zoom_percent().unwrap_or(100.) / 100.;
            match presented
                .document
                .hit_test(point, 4. / scale)
                .ok()
                .flatten()
            {
                Some(Element::Text(text)) if !text.base.locked => Some(text.base.id.clone()),
                _ => None,
            }
        });
        let Some(id) = id else { return false };
        self.start_text(TextInputTarget::Existing { id });
        true
    }

    fn start_text(&self, target: TextInputTarget) {
        // Until Begin is accepted, draw the existing layer or the unfitted
        // layer a click creates, so the field appears at once.
        let element = match &target {
            TextInputTarget::Existing { id } => self.text_element(id),
            TextInputTarget::New { create } => {
                captures_app::editor_session::new_text_element(String::new(), create).ok()
            }
        };
        let Some(element) = element else { return };
        let counter = self.input_counter.get() + 1;
        self.input_counter.set(counter);
        let id = format!("gtk-text-{counter}");
        *self.inline.borrow_mut() = Some(Inline {
            id: id.clone(),
            started: false,
            accepted: element.text.clone(),
            in_flight: true,
            finishing: false,
            layout: editor_text::inline_editor_layout(&element).ok(),
            element: Some(element.clone()),
        });
        self.text_field.set_text(&element.text);
        self.style_text_field();
        self.text_field.view.set_visible(true);
        self.text_field.view.grab_focus();
        let _ = self.jobs.send(Job::Apply(Request::BeginTextInput {
            input_id: id,
            target,
        }));
        self.busy.set(true);
    }

    fn text_element(&self, id: &str) -> Option<TextElement> {
        self.presented.borrow().as_ref().and_then(|presented| {
            presented
                .document
                .elements
                .iter()
                .find_map(|element| match element {
                    Element::Text(text) if text.base.id == id => Some(text.clone()),
                    _ => None,
                })
        })
    }

    /// Typing: send the newest buffer when the worker is free.
    pub(super) fn text_changed(&self) {
        self.fit_text_field();
        if !self.text_field.loading.get() {
            self.drain_text();
        }
    }

    /// Size the field to the layer frame, growing with auto-width text. The
    /// overlay positions it but allocates its size request.
    fn fit_text_field(&self) {
        let slot = self.inline.borrow();
        let Some(layout) = slot.as_ref().and_then(|inline| inline.layout.as_ref()) else {
            return;
        };
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let view = &self.text_field.view;
        // `measure` includes the size request; clear it to read the content.
        view.set_size_request(-1, -1);
        let mut width = (layout.frame.width * scale).ceil().max(48.) as i32;
        if layout.auto_width {
            let (_, natural, _, _) = view.measure(gtk::Orientation::Horizontal, -1);
            width = width.max(natural);
        }
        let (_, natural, _, _) = view.measure(gtk::Orientation::Vertical, width);
        let height = ((layout.frame.height * scale).ceil().max(28.) as i32).max(natural);
        drop(slot);
        view.set_size_request(width, height);
        self.text_overlay.queue_allocate();
    }

    pub(super) fn finish_text(&self) {
        if let Some(inline) = self.inline.borrow_mut().as_mut() {
            inline.finishing = true;
        }
        self.drain_text();
    }

    fn drain_text(&self) {
        let text = self.text_field.text();
        let mut slot = self.inline.borrow_mut();
        let Some(inline) = slot.as_mut() else { return };
        if inline.in_flight {
            return;
        }
        let request = if !inline.started {
            // Begin failed: nothing reached the document; drop the field.
            drop(slot);
            self.close_text();
            return;
        } else if text != inline.accepted {
            Request::UpdateTextInput {
                input_id: inline.id.clone(),
                text,
            }
        } else if inline.finishing {
            Request::FinishTextInput {
                input_id: inline.id.clone(),
                commit: true,
            }
        } else {
            return;
        };
        inline.in_flight = true;
        drop(slot);
        self.busy.set(true);
        let _ = self.jobs.send(Job::Apply(request));
    }

    /// A worker result arrived; the presented snapshot is already current.
    pub(super) fn received_text(&self) {
        let active = self
            .presented
            .borrow()
            .as_ref()
            .and_then(|presented| presented.active_input.clone());
        let mut slot = self.inline.borrow_mut();
        let Some(inline) = slot.as_mut() else { return };
        match active {
            Some(active) if active.input_id == inline.id => {
                inline.started = true;
                inline.in_flight = false;
                drop(slot);
                let element = self.text_element(&active.layer_id);
                let mut slot = self.inline.borrow_mut();
                let Some(inline) = slot.as_mut() else { return };
                if let Some(element) = &element {
                    inline.accepted = element.text.clone();
                }
                if active.layout.is_some() {
                    inline.layout = active.layout;
                }
                inline.element = element;
                drop(slot);
                *self.selected.borrow_mut() = Some(active.layer_id);
                self.style_text_field();
                self.drain_text();
            }
            Some(_) => {}
            // Finished (or an unrelated edit ended the input): the committed
            // layer is in the canvas pixels again.
            None => {
                drop(slot);
                self.close_text();
            }
        }
    }

    /// A worker request failed: keep the typing and allow a retry.
    pub(super) fn text_failed(&self) {
        let started = {
            let mut slot = self.inline.borrow_mut();
            let Some(inline) = slot.as_mut() else { return };
            inline.in_flight = false;
            inline.finishing = false;
            inline.started
        };
        if !started {
            self.close_text();
        }
    }

    fn close_text(&self) {
        self.inline.borrow_mut().take();
        let field = &self.text_field.view;
        let focused = field.has_focus();
        field.set_visible(false);
        self.text_field.set_text("");
        if focused {
            self.canvas.grab_focus();
        }
        self.canvas.queue_draw();
    }

    /// Place and style the field from the layer and the viewport.
    pub(super) fn style_text_field(&self) {
        let slot = self.inline.borrow();
        let Some(inline) = slot.as_ref() else { return };
        let (Some(layout), Some(element)) = (&inline.layout, &inline.element) else {
            return;
        };
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let family = self
            .presented
            .borrow()
            .as_ref()
            .and_then(|p| p.font_families.get(&element.font_family).cloned())
            .unwrap_or_else(|| "Liberation Sans".into());
        let px = |value: f64| value * scale;
        let plate = element
            .background
            .as_deref()
            .filter(|color| !color.is_empty())
            .unwrap_or("transparent");
        let css = format!(
            "textview.inline-text, textview.inline-text text {{ background: {plate}; \
             color: {color}; caret-color: {color}; }}\n\
             textview.inline-text {{ border-radius: {radius}px; font-family: \"{family}\"; \
             font-size: {size}px; line-height: {line}px; font-weight: {weight}; \
             font-style: {style}; }}",
            color = element.color,
            radius = px(layout.plate_radius),
            size = px(element.font_size),
            line = px(layout.line_height),
            weight = if element.bold { 700 } else { 400 },
            style = if element.italic { "italic" } else { "normal" },
        );
        self.text_field.css.load_from_data(&css);
        let view = &self.text_field.view;
        let [top, right, bottom, left] = layout.padding.map(|value| px(value).round() as i32);
        view.set_top_margin(top);
        view.set_right_margin(right);
        view.set_bottom_margin(bottom);
        view.set_left_margin(left);
        view.set_justification(match element.align.as_str() {
            "center" => gtk::Justification::Center,
            "right" => gtk::Justification::Right,
            _ => gtk::Justification::Left,
        });
        view.set_wrap_mode(if layout.auto_width {
            gtk::WrapMode::None
        } else {
            gtk::WrapMode::WordChar
        });
        drop(slot);
        self.fit_text_field();
    }

    /// The field's rectangle in canvas coordinates (`get-child-position`).
    pub(super) fn text_field_rect(&self) -> Option<gdk::Rectangle> {
        let slot = self.inline.borrow();
        let layout = slot.as_ref()?.layout.as_ref()?;
        let (x, y) = self.to_screen(Point {
            x: layout.frame.x,
            y: layout.frame.y,
        })?;
        let (width, height) = self.text_field.view.size_request();
        Some(gdk::Rectangle::new(
            x.round() as i32,
            y.round() as i32,
            width,
            height,
        ))
    }
}
