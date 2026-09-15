//! Screenshot editor GPUI surface.
//!
//! The chrome deliberately follows the shared React editor rather than the
//! Windows experiment.  Pixel/document behavior is delegated to the portable,
//! tested `captures-windows-native` model.

use captures_windows_native::{
    draft::{DraftIdentity, DraftStore},
    editor::{BlendMode, Document, Shape, Tool},
    encoder::{encode_jpeg, encode_png, encode_webp},
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

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Webp,
}

pub struct ScreenshotEditor {
    launch: Launch,
    theme: Theme,
    focus: Option<FocusHandle>,
    text: Option<Entity<crate::preferences::input::TextInput>>,
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
        Ok(Self {
            theme: Theme::new(launch.light),
            focus: None,
            text: None,
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
            format: Format::Png,
            quality: 92,
            status: "Ready".into(),
        })
    }

    fn refresh(&mut self) {
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
                    self.selected = Some(self.document.add(
                        Shape::Text {
                            origin: p,
                            value,
                            font_size: 32.,
                            font_data: self.text_font.clone().unwrap(),
                        },
                        ACCENT,
                        0.,
                    ));
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
            Tool::Select => self
                .document
                .hit_test(p, 8.)
                .and_then(|id| {
                    self.document
                        .layers
                        .iter()
                        .find(|l| l.id == id && !l.locked)
                        .cloned()
                })
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
        let Some(p) = self.document_point(ev.position) else {
            return;
        };
        match &mut self.drag {
            Some(Drag::Draw { start, points }) => {
                points.push(p);
                if let Some(shape) =
                    gesture_shape(self.tool, *start, p, points.clone(), ev.modifiers.shift)
                {
                    let mut preview = self.document.clone();
                    preview.add(shape, ACCENT, 6.);
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
            None => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.document_point(ev.position);
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Move { original, .. } => {
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
                        self.selected = Some(self.document.add(shape, ACCENT, 6.));
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

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let store = DraftStore::new(&self.launch.profile.join("drafts"));
        self.status = match store.save(&self.identity, self.source_path.as_deref(), &self.document)
        {
            Ok(()) => "Draft saved".into(),
            Err(e) => format!("Draft failed: {e}"),
        };
        cx.notify();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let image = match self.document.render() {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Export failed: {e}");
                cx.notify();
                return;
            }
        };
        let (ext, bytes) = match self.format {
            Format::Png => ("png", encode_png(&image, Some(self.quality))),
            Format::Jpeg => ("jpg", encode_jpeg(&image, self.quality)),
            Format::Webp => ("webp", encode_webp(&image, Some(self.quality))),
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
        let path = self.launch.profile.join("captures").join(format!(
            "capture-{}.{}",
            uuid::Uuid::new_v4(),
            ext
        ));
        self.status = match atomic_write(&path, &bytes) {
            Ok(()) => format!("Saved {} ({} KB)", path.display(), bytes.len() / 1024),
            Err(e) => format!("Save failed: {e}"),
        };
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
        glyph: &'static str,
        tool: Tool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.button(name, glyph, self.tool == tool)
            .w(px(42.))
            .h(px(42.))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_tool(tool, cx)))
    }
}

impl Render for ScreenshotEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let text_input = self
            .text
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    crate::preferences::input::TextInput::new("Text", "Annotation text", cx)
                })
            })
            .clone();
        let rendered = self.rendered.clone();
        let recorded_bounds = self.canvas_bounds.clone();
        let paint_bounds = recorded_bounds.clone();
        let image_size = size(
            self.document.canvas_width as f32,
            self.document.canvas_height as f32,
        );
        let fit = self.fit;
        let zoom = self.zoom;
        let tools = [
            ("Select", "↖", Tool::Select),
            ("Crop", "⌗", Tool::Crop),
            ("Text", "T", Tool::Text),
            ("Pen", "✎", Tool::Pen),
            ("Arrow", "↗", Tool::Arrow),
            ("Rectangle", "□", Tool::Rectangle),
            ("Ellipse", "○", Tool::Ellipse),
            ("Eraser", "⌫", Tool::Eraser),
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
        div()
            .track_focus(&focus)
            .key_context("ScreenshotEditor")
            .on_key_down(cx.listener(Self::key_down))
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
                    .h(px(58.))
                    .px_5()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Screenshot editor"),
                    )
                    .child(div().text_sm().text_color(t.muted).child(format!(
                        "{} × {}",
                        self.document.canvas_width, self.document.canvas_height
                    )))
                    .child(div().flex_1())
                    .child(
                        self.button("undo", "Undo", false)
                            .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                    )
                    .child(
                        self.button("redo", "Redo", false)
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
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(64.))
                            .py_3()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
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
                            .p_6()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.canvas)
                            .child(
                                div()
                                    .id("canvas")
                                    .size_full()
                                    .rounded(px(8.))
                                    .bg(t.raised)
                                    .shadow_lg()
                                    .cursor_crosshair()
                                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                                    .on_mouse_move(cx.listener(Self::mouse_move))
                                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
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
                                                            + (bounds.size.width - size.width) / 2.,
                                                        bounds.origin.y
                                                            + (bounds.size.height - size.height)
                                                                / 2.,
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
                                            },
                                        )
                                        .size_full(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .child(
                                section("Inspector", t)
                                    .when(self.tool == Tool::Text, |panel| {
                                        panel.child(text_input).child(
                                            div().text_size(px(12.)).text_color(t.muted).child(
                                                "Enter text, then click the image to place it.",
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
                                                                s.document.toggle_visibility(id);
                                                                s.refresh();
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                .child(self.button("lock", "Lock", false).on_click(
                                                    cx.listener(move |s, _, _, cx| {
                                                        s.document.toggle_locked(id);
                                                        s.refresh();
                                                        cx.notify();
                                                    }),
                                                ))
                                                .child(
                                                    self.button("duplicate", "Duplicate", false)
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
                                                    self.button("raise", "Raise", false).on_click(
                                                        cx.listener(move |s, _, _, cx| {
                                                            s.document.move_layer(id, 1);
                                                            s.refresh();
                                                            cx.notify();
                                                        }),
                                                    ),
                                                )
                                                .child(
                                                    self.button("lower", "Lower", false).on_click(
                                                        cx.listener(move |s, _, _, cx| {
                                                            s.document.move_layer(id, -1);
                                                            s.refresh();
                                                            cx.notify();
                                                        }),
                                                    ),
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
                                                                s.document
                                                                    .set_layer_rotation(id, angle);
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
                                                                        l.opacity.saturating_sub(26)
                                                                    });
                                                                s.document
                                                                    .set_layer_opacity(id, opacity);
                                                                s.refresh();
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    self.button("blend", "Multiply", false)
                                                        .on_click(cx.listener(
                                                            move |s, _, _, cx| {
                                                                s.document.set_layer_blend_mode(
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
                                        .gap_2()
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
                    .h(px(92.))
                    .px_5()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.subtle)
                                    .child("FILE NAME & DESTINATION"),
                            )
                            .child(div().text_sm().child(self.status.clone())),
                    )
                    .child(
                        self.button("png", "PNG", self.format == Format::Png)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Png;
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("jpeg", "JPEG", self.format == Format::Jpeg)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Jpeg;
                                cx.notify()
                            })),
                    )
                    .child(
                        self.button("webp", "WebP", self.format == Format::Webp)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.format = Format::Webp;
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
        _ => None,
    }
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
}
